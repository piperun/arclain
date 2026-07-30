//! The image-asset lifecycle: read-once, decode-once, texture ownership,
//! corrupt-entry eviction, and the cache-ready restart handshake.
//!
//! Driven against an instrumented [`ImageBytes`] source rather than a real
//! `ContentCache`. That is deliberate on two counts:
//!
//! - What this file pins is the *store's* lifecycle -- how many reads a
//!   given sequence of requests performs, which thread they run on, and how
//!   a fetch completing mid-load is serialized. A counting source states
//!   those directly; a real cache only let them be inferred from index
//!   traffic.
//! - Where the bytes actually come from is `arclain_app`'s business now
//!   (see `crates/app/tests/display_images.rs` for the namespace and
//!   fetch-once behaviour, and `plugin_session_facade_test.rs` for this
//!   frontend's routing into it). This frontend holds no cache handle to
//!   build a fixture from, and building one anyway would test a
//!   composition production no longer has.
//!
//! A side effect worth naming: with no cache on disk there is no
//! free-space probe, so nothing here depends on the machine's headroom.

use arclain_ui::core::tabs::TabId;
use arclain_ui::shared::image_assets::{ImageAssetState, ImageAssetStore, ImageBytes, ImageOwner};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::ThreadId;
use std::time::{Duration, Instant};

/// A byte source that records every read and eviction, and which thread it
/// happened on.
#[derive(Default)]
struct CountingImageBytes {
    entries: Mutex<HashMap<String, Vec<u8>>>,
    gets: AtomicUsize,
    removes: AtomicUsize,
    get_threads: Mutex<Vec<ThreadId>>,
    remove_threads: Mutex<Vec<ThreadId>>,
}

impl CountingImageBytes {
    fn insert(&self, key: &str, bytes: Vec<u8>) {
        self.entries.lock().insert(key.to_string(), bytes);
    }

    fn get_count(&self) -> usize {
        self.gets.load(Ordering::SeqCst)
    }

    fn remove_count(&self) -> usize {
        self.removes.load(Ordering::SeqCst)
    }

    fn get_threads(&self) -> Vec<ThreadId> {
        self.get_threads.lock().clone()
    }

    fn remove_threads(&self) -> Vec<ThreadId> {
        self.remove_threads.lock().clone()
    }
}

impl ImageBytes for CountingImageBytes {
    fn get(&self, key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        self.gets.fetch_add(1, Ordering::SeqCst);
        self.get_threads.lock().push(std::thread::current().id());
        Ok(self.entries.lock().get(key).cloned())
    }

    fn fetch(&self, _plugin_id: Option<&str>, _key: &str, _url: &str) -> anyhow::Result<()> {
        Ok(())
    }

    fn remove(&self, key: &str) {
        self.removes.fetch_add(1, Ordering::SeqCst);
        self.remove_threads.lock().push(std::thread::current().id());
        self.entries.lock().remove(key);
    }

    fn can_store(&self, _key: &str) -> bool {
        true
    }
}

/// A source whose *first* read parks until released, so a test can make a
/// cache-ready notification arrive while a stale load is still in flight.
struct HeldFirstReadImageBytes {
    entries: Mutex<HashMap<String, Vec<u8>>>,
    gets: AtomicUsize,
    first_get_entered: Sender<()>,
    release_first_get: Mutex<Receiver<()>>,
}

impl ImageBytes for HeldFirstReadImageBytes {
    fn get(&self, key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let snapshot = self.entries.lock().get(key).cloned();
        if self.gets.fetch_add(1, Ordering::SeqCst) == 0 {
            self.first_get_entered
                .send(())
                .expect("signal held image read");
            self.release_first_get
                .lock()
                .recv()
                .expect("release held image read");
        }
        Ok(snapshot)
    }

    fn fetch(&self, _plugin_id: Option<&str>, _key: &str, _url: &str) -> anyhow::Result<()> {
        Ok(())
    }

    fn remove(&self, key: &str) {
        self.entries.lock().remove(key);
    }

    fn can_store(&self, _key: &str) -> bool {
        true
    }
}

struct StoreFixture {
    store: ImageAssetStore,
    source: Arc<CountingImageBytes>,
}

fn fixture() -> StoreFixture {
    let source = Arc::new(CountingImageBytes::default());
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("create runtime"));
    let store = ImageAssetStore::from_source(source.clone(), runtime);
    StoreFixture { store, source }
}

fn png_1x1() -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([12, 34, 56, 255]));
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .expect("encode PNG");
    bytes.into_inner()
}

fn wait_until(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !predicate() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for image worker"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn owner_identity_distinguishes_surface_kind_and_origin_tab() {
    let page = ImageOwner::plugin_page("plugin", "details", TabId(1));
    let same_page_other_tab = ImageOwner::plugin_page("plugin", "details", TabId(2));
    let dialog = ImageOwner::plugin_dialog("plugin", "details", TabId(1));
    let panel = ImageOwner::plugin_panel("plugin", "details", TabId(1));

    assert_ne!(page, same_page_other_tab);
    assert_ne!(page, dialog);
    assert_ne!(page, panel);
    assert_ne!(dialog, panel);
}

#[test]
fn uploaded_asset_is_read_and_decoded_once_and_releases_cpu_pixels() {
    let fixture = fixture();
    fixture.source.insert("cover", png_1x1());
    let owner = ImageOwner::plugin_page("plugin", "page", TabId(1));
    let ctx = eframe::egui::Context::default();
    let ui_caller_thread = std::thread::current().id();

    assert_eq!(
        fixture.store.request(owner.clone(), "cover", ctx.clone()),
        ImageAssetState::Loading
    );
    wait_until(|| fixture.store.is_decoded("cover"));
    assert_eq!(fixture.source.get_count(), 1);
    assert!(
        fixture
            .source
            .get_threads()
            .iter()
            .all(|thread_id| *thread_id != ui_caller_thread),
        "cache reads must not run on the UI caller thread"
    );
    assert_eq!(fixture.store.state("cover"), Some(ImageAssetState::Decoded));
    assert!(
        fixture.store.get_texture(&owner, "cover").is_none(),
        "decoded pixels must only upload through the UI-facing upload_ready API"
    );

    let first = fixture
        .store
        .upload_ready("cover", &ctx)
        .expect("decoded texture uploads");
    assert_eq!(fixture.store.cpu_byte_len("cover"), 0);
    assert_eq!(
        fixture.store.request(owner.clone(), "cover", ctx.clone()),
        ImageAssetState::Uploaded
    );
    let second = fixture
        .store
        .get_texture(&owner, "cover")
        .expect("owner sees uploaded texture");
    assert_eq!(first.id(), second.id());
    assert_eq!(
        fixture.source.get_count(),
        1,
        "uploaded texture re-read disk cache"
    );

    fixture.store.release_owner(&owner);
    assert!(!fixture.store.contains("cover"));
}

#[test]
fn texture_survives_until_every_owner_releases_it() {
    let fixture = fixture();
    fixture.source.insert("shared", png_1x1());
    let page = ImageOwner::plugin_page("plugin", "page", TabId(1));
    let lightbox = ImageOwner::Lightbox {
        tab: TabId(7),
        plugin_id: None,
    };
    let ctx = eframe::egui::Context::default();

    fixture.store.request(page.clone(), "shared", ctx.clone());
    fixture
        .store
        .request(lightbox.clone(), "shared", ctx.clone());
    wait_until(|| fixture.store.is_decoded("shared"));
    let texture = fixture
        .store
        .upload_ready("shared", &ctx)
        .expect("decoded texture uploads");

    fixture.store.release_owner(&page);
    assert!(fixture.store.contains("shared"));
    assert_eq!(
        fixture
            .store
            .get_texture(&lightbox, "shared")
            .expect("second owner keeps texture")
            .id(),
        texture.id()
    );

    fixture.store.release_owner(&lightbox);
    assert!(!fixture.store.contains("shared"));
    assert_eq!(fixture.source.get_count(), 1);
}

#[test]
fn retaining_live_owners_evicts_abandoned_origins_only() {
    let fixture = fixture();
    fixture.source.insert("retained", png_1x1());
    let page = ImageOwner::plugin_page("plugin", "page", TabId(1));
    let settings = ImageOwner::plugin_settings("plugin");
    let ctx = eframe::egui::Context::default();

    fixture.store.request(page.clone(), "retained", ctx.clone());
    fixture
        .store
        .request(settings.clone(), "retained", ctx.clone());
    wait_until(|| fixture.store.is_decoded("retained"));
    let _ = fixture
        .store
        .upload_ready("retained", &ctx)
        .expect("decoded texture uploads");

    fixture.store.retain_owners(&HashSet::from([page.clone()]));
    assert!(fixture.store.contains("retained"));
    assert!(fixture.store.get_texture(&settings, "retained").is_none());
    assert!(fixture.store.get_texture(&page, "retained").is_some());

    fixture.store.retain_owners(&HashSet::new());
    assert!(!fixture.store.contains("retained"));
}

#[test]
fn corrupt_cached_image_is_removed_off_thread() {
    let fixture = fixture();
    fixture.source.insert("corrupt", b"not an image".to_vec());
    let owner = ImageOwner::plugin_settings("plugin");
    let ctx = eframe::egui::Context::default();
    let ui_caller_thread = std::thread::current().id();

    fixture.store.request(owner, "corrupt", ctx);
    wait_until(|| {
        matches!(
            fixture.store.state("corrupt"),
            Some(ImageAssetState::Failed(_))
        )
    });

    assert_eq!(fixture.source.get_count(), 1);
    assert_eq!(fixture.source.remove_count(), 1);
    assert!(
        fixture
            .source
            .get_threads()
            .iter()
            .all(|thread_id| *thread_id != ui_caller_thread),
        "corrupt cache reads must not run on the UI caller thread"
    );
    assert!(
        fixture
            .source
            .remove_threads()
            .iter()
            .all(|thread_id| *thread_id != ui_caller_thread),
        "corrupt cache removal must not run on the UI caller thread"
    );
    assert_eq!(fixture.store.cpu_byte_len("corrupt"), 0);
}

#[test]
fn cache_ready_notification_retries_a_miss_without_render_polling() {
    let fixture = fixture();
    let owner = ImageOwner::plugin_page("plugin", "page", TabId(1));
    let ctx = eframe::egui::Context::default();

    fixture.store.request(owner, "late", ctx.clone());
    wait_until(|| {
        matches!(
            fixture.store.state("late"),
            Some(ImageAssetState::Failed(_))
        )
    });
    assert_eq!(fixture.source.get_count(), 1);
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(
        fixture.source.get_count(),
        1,
        "failed asset polled the cache without a cache-ready transition"
    );

    fixture.source.insert("late", png_1x1());
    fixture.store.cache_ready("late", ctx);
    wait_until(|| fixture.store.is_decoded("late"));

    assert_eq!(fixture.source.get_count(), 2);
}

#[test]
fn cache_ready_during_loading_serializes_a_restart_after_the_stale_miss() {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let source = Arc::new(HeldFirstReadImageBytes {
        entries: Mutex::new(HashMap::new()),
        gets: AtomicUsize::new(0),
        first_get_entered: entered_tx,
        release_first_get: Mutex::new(release_rx),
    });
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("create runtime"));
    let store = ImageAssetStore::from_source(source.clone(), runtime);
    let owner = ImageOwner::plugin_page("plugin", "page", TabId(1));
    let ctx = eframe::egui::Context::default();

    assert_eq!(
        store.request(owner, "racing", ctx.clone()),
        ImageAssetState::Loading
    );
    entered_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("initial read reaches the held source");
    source
        .entries
        .lock()
        .insert("racing".to_string(), png_1x1());
    store.cache_ready("racing", ctx);
    release_tx.send(()).expect("release stale cache miss");

    wait_until(|| !matches!(store.state("racing"), Some(ImageAssetState::Loading)));
    assert!(
        store.is_decoded("racing"),
        "cache-ready notification was lost while the previous generation was loading"
    );
    assert_eq!(
        source.gets.load(Ordering::SeqCst),
        2,
        "the stale load must finish before exactly one cache-ready restart"
    );
}

fn read_if_present(root: &Path, relative: &str) -> Option<String> {
    let path = root.join(relative);
    path.exists()
        .then(|| std::fs::read_to_string(&path).expect("read image render source"))
}

#[test]
fn image_render_paths_do_not_read_or_decode_cached_media() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let render_paths = [
        "src/features/plugins/presentation/rendering/image.rs",
        "src/features/plugins/presentation/rendering/widgets/display.rs",
        "src/shared/components/carousel/image_view.rs",
        "src/shared/components/carousel/thumbnail_strip.rs",
        "src/shared/dialogs/lightbox.rs",
    ];

    for relative in render_paths {
        let source = read_if_present(root, relative).expect("render source exists");
        for forbidden in [
            ".content_cache.get",
            "cache.get(",
            // A per-frame block on the runtime from the egui thread is what
            // the image pipeline exists to avoid: reads and fetches belong
            // on the store's blocking pool, not in a render pass.
            "block_on",
            "image::load_from_memory",
            "request_repaint()",
        ] {
            assert!(
                !source.contains(forbidden),
                "{relative} performs blocking cached-media work via {forbidden}"
            );
        }
    }
}

#[test]
fn image_loading_placeholders_do_not_use_animated_widgets() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = read_if_present(
        root,
        "src/features/plugins/presentation/rendering/widgets/display.rs",
    )
    .expect("plugin display renderer exists");
    let thumbnail_renderer = source
        .split("fn render_list_item_thumbnail")
        .nth(1)
        .expect("thumbnail renderer exists")
        .split("fn render_list_item_text")
        .next()
        .expect("thumbnail renderer has an end boundary");

    assert!(
        !thumbnail_renderer.contains("egui::Spinner"),
        "egui::Spinner requests repaint every frame and turns image loading into polling"
    );
}

#[test]
fn plugin_image_surfaces_construct_typed_owners() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let page_and_dialog =
        read_if_present(root, "src/features/plugins/presentation/views/rendering.rs")
            .expect("plugin page and dialog renderer exists");
    let archive_panel = read_if_present(
        root,
        "src/features/archive_browser/presentation/components/panel.rs",
    )
    .expect("archive panel renderer exists");

    assert!(page_and_dialog.contains("ImageOwner::plugin_page("));
    assert!(page_and_dialog.contains("ImageOwner::plugin_dialog("));
    assert!(archive_panel.contains("ImageOwner::plugin_panel("));
    for encoded_prefix in ["dialog:{}", "panel:{}", "dialog:", "panel:"] {
        assert!(
            !page_and_dialog.contains(encoded_prefix),
            "plugin owner identity must not be encoded in a string prefix: {encoded_prefix}"
        );
        assert!(
            !archive_panel.contains(encoded_prefix),
            "archive owner identity must not be encoded in a string prefix: {encoded_prefix}"
        );
    }
}

#[test]
fn image_surfaces_do_not_own_separate_texture_namespaces() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let image_sources = [
        "src/features/plugins/presentation/rendering/async_image.rs",
        "src/features/plugins/presentation/rendering/image.rs",
        "src/shared/async_image.rs",
        "src/shared/components/carousel/image_view.rs",
        "src/shared/components/carousel/thumbnail_strip.rs",
        "src/shared/dialogs/lightbox.rs",
    ];

    let combined = image_sources
        .iter()
        .filter_map(|relative| read_if_present(root, relative))
        .collect::<Vec<_>>()
        .join("\n");

    for namespace in ["plugin_image", "carousel_image", "lightbox_image"] {
        assert!(
            !combined.contains(namespace),
            "legacy texture namespace {namespace} remains"
        );
    }
}
