//! End-to-end mock test for the DLSite metadata sync pipeline.
//!
//! Proves the production code path without network: canned AJAX JSON +
//! HTML responses → `DLSiteProvider::parse_responses` → `ProductMetadata`
//! → `LibraryService` (which calls `DieselBackend::sync_save_metadata`)
//! → SQLite → `sync_get_metadata` → `ProductMetadata`, with every
//! meaningful field surviving the round-trip.
//!
//! This is the layer the lost gameta refactor was about — the
//! synchronous storage wrappers that arclain depends on. The full
//! parse+merge+save+read chain runs against an in-memory SQLite
//! (tempdir-backed) so failures are deterministic and don't need
//! network access or a real RJ code.

use arclain_core::LibraryService;
use gameta_core::{HttpResponse, MetadataProvider, MetadataSource};
use gameta_lib::providers::dlsite::DLSiteProvider;
use tempfile::TempDir;

const TEST_EXTERNAL_ID: &str = "RJ999001"; // synthetic — not a real product

// ─── Canned responses ────────────────────────────────────────────────

fn ajax_response(external_id: &str) -> HttpResponse {
    // Shape matches DLSite's AJAX product endpoint: object keyed by ID.
    let body = serde_json::json!({
        external_id: {
            "work_name": "Crafted Title",
            "maker_name": "Crafted Circle",
            "regist_date": "2024-06-15 00:00:00",
            "price": 1980,
            "rate_average_2dp": 4.5,
            "rate_count": 120,
            "dl_count": 5_000,
            "wishlist_count": 200,
            "review_count": 50,
            "file_size": "1.2GB",
            "file_type": "ZIP",
            "age_category_string": "R-18",
            "genre": [{"name": "RPG"}, {"name": "Adventure"}],
            "maker_id": "RG12345",
            "site_id": "maniax",
            "work_type": "RPG",
            "work_image": "//img.example.com/cover.jpg"
        }
    });
    HttpResponse::ok(serde_json::to_vec(&body).unwrap())
}

fn html_response() -> HttpResponse {
    // Minimal scrapable HTML — matches the selectors the parser walks
    // (h1#work_name, span.maker_name, table#work_maker, table#work_outline,
    // div.work_parts_container).
    let body = r##"<html><body>
        <h1 id="work_name">Crafted Title HTML</h1>
        <div id="work_right">
            <span class="maker_name"><a href="#">Crafted Circle</a></span>
        </div>
        <table id="work_maker">
            <tr><th>Brand</th><td>Crafted Brand</td></tr>
            <tr><th>Publisher</th><td>Crafted Publisher</td></tr>
        </table>
        <table id="work_outline">
            <tr><th>Release Date</th><td>2024-06-15</td></tr>
            <tr><th>Series</th><td>Crafted Series</td></tr>
            <tr><th>File size</th><td>1.2GB</td></tr>
            <tr><th>Voice Actor</th><td><a href="#">VA One</a>, <a href="#">VA Two</a></td></tr>
            <tr><th>Author</th><td><a href="#">Author One</a></td></tr>
            <tr><th>Genre</th><td><a href="#">RPG</a> <a href="#">Adventure</a></td></tr>
        </table>
        <div class="work_parts_container">A crafted description for the round-trip test.</div>
    </body></html>"##;
    HttpResponse::ok(body.as_bytes().to_vec())
}

fn temp_library() -> (TempDir, LibraryService) {
    let dir = TempDir::new().expect("tempdir");
    let db_path = dir.path().join("dlsite_pipeline_test.sqlite");
    let svc = LibraryService::new(&db_path).expect("LibraryService::new");
    (dir, svc)
}

// ─── Tests ───────────────────────────────────────────────────────────

#[test]
fn json_and_html_pipeline_full_roundtrip() {
    let provider = DLSiteProvider::new();
    let ajax = ajax_response(TEST_EXTERNAL_ID);
    let html = html_response();
    let ajax_key = format!("dlsite:ajax:{}", TEST_EXTERNAL_ID);
    let html_key = format!("dlsite:html:{}", TEST_EXTERNAL_ID);
    let responses = [
        (ajax_key.as_str(), &ajax),
        (html_key.as_str(), &html),
    ];

    let parsed = provider
        .parse_responses(TEST_EXTERNAL_ID, &responses)
        .expect("parse_responses");

    // Sanity-check the parse output before storage.
    let expected_id = format!("dlsite:{}", TEST_EXTERNAL_ID);
    assert_eq!(parsed.id, expected_id);
    assert_eq!(parsed.source, MetadataSource::DLSite);
    assert_eq!(parsed.external_id, TEST_EXTERNAL_ID);

    // API JSON wins for fields present in both — "Crafted Title HTML"
    // never overwrites "Crafted Title" from AJAX.
    assert_eq!(parsed.title.as_deref(), Some("Crafted Title"));
    assert_eq!(parsed.creator.as_deref(), Some("Crafted Circle"));

    assert_eq!(parsed.price, Some(1980));
    assert_eq!(parsed.currency.as_deref(), Some("JPY"));
    assert_eq!(parsed.rating, Some(4.5));
    assert_eq!(parsed.rating_count, Some(120));
    assert_eq!(parsed.purchase_count, Some(5_000));
    assert_eq!(parsed.favorite_count, Some(200));
    assert_eq!(parsed.review_count, Some(50));
    assert_eq!(parsed.file_size.as_deref(), Some("1.2GB"));
    assert_eq!(parsed.file_format.as_deref(), Some("ZIP"));
    assert_eq!(parsed.age_rating.as_deref(), Some("R-18"));
    assert_eq!(parsed.genres, vec!["RPG", "Adventure"]);

    // API extras come first; HTML extras merge in for keys the API
    // didn't populate. The `work_image` "//"-prefixed URL gets upgraded
    // to https in `parse_api_response`.
    assert_eq!(parsed.extras["maker_id"], "RG12345");
    assert_eq!(parsed.extras["site_id"], "maniax");
    assert_eq!(parsed.extras["work_type"], "RPG");
    assert_eq!(
        parsed.extras["cover_image"],
        "https://img.example.com/cover.jpg"
    );
    assert_eq!(parsed.extras["brand"], "Crafted Brand");
    assert_eq!(parsed.extras["publisher"], "Crafted Publisher");
    assert_eq!(parsed.extras["series"], "Crafted Series");

    assert!(
        parsed.raw_api_response.is_some(),
        "raw_api_response must be preserved"
    );
    assert!(parsed.raw_html.is_some(), "raw_html must be preserved");
    assert!(!parsed.geo_blocked);

    // Persist through LibraryService and read back. Every field must
    // survive the SQLite round-trip.
    let (_tempdir, svc) = temp_library();
    svc.save_metadata(&parsed).expect("save_metadata");

    let loaded = svc
        .get_metadata(&parsed.id)
        .expect("get_metadata Ok")
        .expect("row must exist after save");

    assert_eq!(loaded.id, parsed.id);
    assert_eq!(loaded.source, parsed.source);
    assert_eq!(loaded.external_id, parsed.external_id);
    assert_eq!(loaded.title, parsed.title);
    assert_eq!(loaded.creator, parsed.creator);
    assert_eq!(loaded.description, parsed.description);
    assert_eq!(loaded.release_date, parsed.release_date);
    assert_eq!(loaded.price, parsed.price);
    assert_eq!(loaded.currency, parsed.currency);
    assert_eq!(loaded.rating, parsed.rating);
    assert_eq!(loaded.rating_count, parsed.rating_count);
    assert_eq!(loaded.purchase_count, parsed.purchase_count);
    assert_eq!(loaded.favorite_count, parsed.favorite_count);
    assert_eq!(loaded.review_count, parsed.review_count);
    assert_eq!(loaded.file_size, parsed.file_size);
    assert_eq!(loaded.file_format, parsed.file_format);
    assert_eq!(loaded.age_rating, parsed.age_rating);
    assert_eq!(loaded.genres, parsed.genres);
    assert_eq!(loaded.tags, parsed.tags);
    assert_eq!(loaded.languages, parsed.languages);
    assert_eq!(loaded.extras, parsed.extras);
    assert_eq!(loaded.geo_blocked, parsed.geo_blocked);
    assert_eq!(loaded.raw_api_response, parsed.raw_api_response);
    assert_eq!(loaded.raw_html, parsed.raw_html);
}

#[test]
fn html_only_pipeline_still_persists_partial_metadata() {
    // Fallback path: the AJAX endpoint is unavailable, so the plugin
    // only has HTML. Provider must still emit a usable
    // `ProductMetadata` and storage must persist it (with the
    // API-only fields left blank).
    let provider = DLSiteProvider::new();
    let html = html_response();
    let html_key = format!("dlsite:html:{}", TEST_EXTERNAL_ID);
    let responses = [(html_key.as_str(), &html)];

    let parsed = provider
        .parse_responses(TEST_EXTERNAL_ID, &responses)
        .expect("parse_responses html-only");

    // From HTML alone the parser fills title/creator from the scraped
    // <h1 id="work_name"> + .maker_name; price/rating stay empty (API-only).
    assert_eq!(parsed.title.as_deref(), Some("Crafted Title HTML"));
    assert_eq!(parsed.creator.as_deref(), Some("Crafted Circle"));
    assert!(parsed.price.is_none());
    assert!(parsed.rating.is_none());
    assert!(parsed.raw_api_response.is_none());
    assert!(parsed.raw_html.is_some());

    // Storage round-trip with partial metadata still works.
    let (_tempdir, svc) = temp_library();
    svc.save_metadata(&parsed).expect("save partial metadata");

    let loaded = svc
        .get_metadata(&parsed.id)
        .expect("get_metadata")
        .expect("row exists");
    assert_eq!(loaded.title, parsed.title);
    assert_eq!(loaded.creator, parsed.creator);
    assert!(loaded.price.is_none());
    assert_eq!(loaded.extras, parsed.extras);
    assert_eq!(loaded.raw_html, parsed.raw_html);
}

#[test]
fn geo_blocked_response_persists_with_flag_set() {
    // Geo-block detection is a parse-time concern, but the resulting
    // flag must survive storage so the LibraryService geo-block guard
    // (in `save_metadata`) can do its job on subsequent writes.
    let provider = DLSiteProvider::new();
    let html_body = r##"<html><body>
        <h1 id="work_name">Crafted Title Blocked</h1>
        <p>region restricted</p>
    </body></html>"##;
    let html = HttpResponse::ok(html_body.as_bytes().to_vec());
    let html_key = format!("dlsite:html:{}", TEST_EXTERNAL_ID);
    let responses = [(html_key.as_str(), &html)];

    let parsed = provider
        .parse_responses(TEST_EXTERNAL_ID, &responses)
        .expect("parse_responses geo-blocked");
    assert!(
        parsed.geo_blocked,
        "provider must flag geo-blocked HTML",
    );

    let (_tempdir, svc) = temp_library();
    svc.save_metadata(&parsed).expect("first save (no prior row)");

    let loaded = svc
        .get_metadata(&parsed.id)
        .expect("get_metadata")
        .expect("row exists");
    assert!(loaded.geo_blocked, "geo_blocked must round-trip through SQLite");
}
