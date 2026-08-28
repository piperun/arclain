//! Resolve a plan's scheduled downloads to local files before the plan
//! is applied.
//!
//! The applier runs inside a backup/promote transaction over the work
//! directory. A fetch inside that transaction would hold a half-applied
//! layout for as long as the network takes, so downloads are resolved
//! first and handed to the applier as ordinary moves from a staging
//! directory. The applier therefore never performs I/O over the network,
//! and a fetched image is placed and rolled back with everything else.

use anyhow::{Context, Result};
use std::io::Read;
use std::path::Path;

use crate::features::organization::engine::{OrganizationPlan, PendingDownload};

/// A plan whose downloads have been resolved to local files.
pub struct StagedPlan {
    /// The plan with an empty `downloads` list and one appended move per
    /// image that was fetched.
    pub plan: OrganizationPlan,
    /// One `(url, dest_path, reason)` per download that could not be fetched. A
    /// screenshot that fails to arrive does not fail the run; the caller
    /// reports these to its own progress log.
    pub unfetched: Vec<(String, String, String)>,
}

/// Fetch every download the plan schedules into a staging directory
/// inside `work_dir`, and return the plan with each download rewritten
/// as an ordinary move from that staging path.
///
/// The staging directory is created under `work_dir` with a randomized
/// name and persists after this function returns; the applier will move
/// files out of it, resolving each move source relative to `work_dir`.
///
/// `fetch` is supplied by the caller so the cache and transport are the
/// caller's concern: the application consults its content cache and
/// falls back to HTTP, and tests return fixed bytes. A caller that wants
/// no network passes a `fetch` that errors, and every download is
/// reported in `unfetched` instead.
pub fn stage_plan_downloads(
    plan: &OrganizationPlan,
    work_dir: &Path,
    fetch: &dyn Fn(&PendingDownload) -> Result<Vec<u8>>,
) -> Result<StagedPlan> {
    let mut staged = plan.clone();
    let mut unfetched = Vec::new();
    for output in &mut staged.outputs {
        output.downloads = Vec::new();
    }

    if plan
        .outputs
        .iter()
        .all(|output| output.downloads.is_empty())
    {
        return Ok(StagedPlan {
            plan: staged,
            unfetched,
        });
    }

    let staging = tempfile::Builder::new()
        .prefix(".arclain-downloads-")
        .tempdir_in(work_dir)?
        .keep();
    let staging_name = staging
        .file_name()
        .and_then(|name| name.to_str())
        .context("staging directory name is not valid UTF-8")?
        .to_string();

    // One staging directory for the whole plan, but the name inside it
    // counts across outputs rather than restarting: two outputs' first
    // screenshots would otherwise both be staged as `000`.
    let mut staged_name = 0usize;
    for (output, staged_output) in plan.outputs.iter().zip(staged.outputs.iter_mut()) {
        for download in &output.downloads {
            let name = format!("{staged_name:03}");
            staged_name += 1;
            match fetch(download) {
                Ok(bytes) => match std::fs::write(staging.join(&name), &bytes) {
                    Ok(_) => {
                        staged_output
                            .moves
                            .push((format!("{staging_name}/{name}"), download.dest_path.clone()));
                    }
                    Err(error) => {
                        unfetched.push((
                            download.url.clone(),
                            download.dest_path.clone(),
                            format!("{error:#}"),
                        ));
                    }
                },
                Err(error) => {
                    unfetched.push((
                        download.url.clone(),
                        download.dest_path.clone(),
                        format!("{error:#}"),
                    ));
                }
            }
        }
    }

    Ok(StagedPlan {
        plan: staged,
        unfetched,
    })
}

/// The most this will read for one image.
///
/// The number is the content cache's own per-resource ceiling, so a
/// screenshot fetched over the network and the same screenshot read
/// back out of the cache are bounded alike rather than by two limits
/// that can drift apart.
///
/// Nothing downstream enforces its use: [`read_bounded`] applies
/// whatever limit its caller passes, and the single call site in
/// [`http_downloader`] below is the only thing that supplies this
/// constant. Narrow that call and no test fails -- the call site is
/// correct by inspection, not by assertion.
const MAX_DOWNLOAD_BYTES: usize = arclain_data::DEFAULT_MAX_RESOURCE_SIZE_BYTES;

/// How many redirects one image may take. Image URLs in scraped
/// metadata routinely hop — http to https, or on to a CDN host — so
/// refusing redirects outright would fail those fetches for no gain.
/// The bound exists so a redirect loop cannot spin, not to police where
/// a hop leads.
const MAX_DOWNLOAD_REDIRECTS: usize = 3;

/// True when an address is somewhere a screenshot has no business
/// living: the host itself, a private network, a link-local range, or
/// the unspecified address.
///
/// Screenshot URLs come from scraped third-party metadata, so the host
/// one names is chosen by whoever controls that metadata rather than by
/// the user. Refusing these stops a scraped URL turning an organize run
/// into a request against the machine it runs on, or the network that
/// machine sits in.
fn is_off_public_internet(address: std::net::IpAddr) -> bool {
    match address {
        std::net::IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
        }
        std::net::IpAddr::V6(v6) => {
            // An IPv4 address wearing an IPv6 mapping is the same host,
            // so unwrap it and judge it by the rules above.
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_off_public_internet(mapped.into());
            }
            let segments = v6.segments();
            v6.is_loopback()
                || v6.is_unspecified()
                // Unique local, fc00::/7. `is_unique_local` is not
                // stable, so match the prefix directly.
                || (segments[0] & 0xfe00) == 0xfc00
                // Link local, fe80::/10.
                || (segments[0] & 0xffc0) == 0xfe80
        }
    }
}

/// A DNS resolver that answers only with addresses on the public
/// internet.
///
/// Installed on the fetch client rather than run as a check before the
/// request, because this is the resolution reqwest performs: every hop
/// of a redirect chain passes through it, and there is no gap between
/// the check and the connect for an answer to change in.
#[derive(Debug)]
struct PublicOnlyResolver;

impl reqwest::dns::Resolve for PublicOnlyResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let lookup = host.clone();
            let resolved = tokio::task::spawn_blocking(move || {
                std::net::ToSocketAddrs::to_socket_addrs(&(lookup.as_str(), 0))
                    .map(|addrs| addrs.collect::<Vec<_>>())
            })
            .await
            .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { Box::new(error) })?
            .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { Box::new(error) })?;

            let public: Vec<std::net::SocketAddr> = resolved
                .into_iter()
                .filter(|addr| !is_off_public_internet(addr.ip()))
                .collect();

            if public.is_empty() {
                let refused: Box<dyn std::error::Error + Send + Sync> =
                    format!("{host} resolves only to addresses off the public internet").into();
                return Err(refused);
            }
            Ok(Box::new(public.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// One HTTP fetcher for a whole plan's downloads, holding a single
/// client so a batch from one host reuses its connection rather than
/// re-handshaking per image.
///
/// Two bounds, each against a different failure:
///
/// - The redirect chain is capped at [`MAX_DOWNLOAD_REDIRECTS`], so a
///   server that redirects in a cycle cannot spin the fetch.
/// - The response is capped at [`MAX_DOWNLOAD_BYTES`], by the advertised
///   `Content-Length` and again by the read itself since a server can
///   omit or understate that header, so a hostile or merely mis-sized
///   image cannot exhaust memory. This is the bound that matters.
///
/// - The destination is capped to the public internet by
///   [`PublicOnlyResolver`], so a URL naming a private, loopback or
///   link-local address cannot be reached. The check lives in the
///   resolver reqwest itself uses, which is what makes it hold for the
///   initial request and for every redirect hop alike, with no window
///   between judging an address acceptable and connecting to it.
pub fn http_downloader() -> Result<impl Fn(&PendingDownload) -> Result<Vec<u8>>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("Arclain/1.0")
        .redirect(reqwest::redirect::Policy::limited(MAX_DOWNLOAD_REDIRECTS))
        .dns_resolver(std::sync::Arc::new(PublicOnlyResolver))
        .build()?;
    Ok(move |download: &PendingDownload| {
        // The one place the production limit enters a fetch, so
        // narrowing it later is a one-line change rather than three.
        let limit = MAX_DOWNLOAD_BYTES;

        let mut response = client.get(&download.url).send()?;
        if !response.status().is_success() {
            anyhow::bail!("status {}", response.status());
        }
        // The advertised length rejects an oversized image for the cost
        // of one header, before any body is transferred.
        if let Some(length) = response.content_length() {
            if length > limit as u64 {
                anyhow::bail!("{length} bytes, over the {limit}-byte limit for one image");
            }
        }
        read_bounded(&mut response, limit)
    })
}

/// Read a body, refusing anything past `limit` bytes.
///
/// Applied even when `Content-Length` already passed, because that
/// header is the server's claim rather than a fact: it can be absent
/// (chunked transfer) or simply understate the body.
///
/// `limit` is a parameter only so the tests can exercise the boundary
/// without allocating the real ceiling twice over. Production has one
/// caller and it passes [`MAX_DOWNLOAD_BYTES`]; this stays private so
/// the limit cannot be widened from outside the crate.
fn read_bounded(body: &mut impl std::io::Read, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    // One byte past the limit, so a body sitting exactly at the limit is
    // still accepted and anything longer is detectable.
    body.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        anyhow::bail!("body ran past the {limit}-byte limit for one image");
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::organization::engine::{OrganizationPlan, PendingDownload, PlannedOutput};

    /// The bound the boundary tests exercise. The cap is a comparison,
    /// so a few hundred bytes tests it exactly as well as
    /// [`MAX_DOWNLOAD_BYTES`] would — and does not cost the suite the
    /// real ceiling in a body plus the same again in a read buffer, per
    /// test.
    const TEST_LIMIT: usize = 256;

    fn plan_with_one_download() -> OrganizationPlan {
        OrganizationPlan {
            rule_name: "Test".to_string(),
            outputs: vec![PlannedOutput {
                root_folder: "Root".to_string(),
                root_folder_template: "Root".to_string(),
                moves: vec![("a.txt".to_string(), "Root/a.txt".to_string())],
                generated_files: vec![],
                downloads: vec![PendingDownload {
                    product_id: Some("RJ123456".to_string()),
                    url: "https://img.example.test/RJ123456_img_main.jpg".to_string(),
                    dest_path: "Root/screenshots/image_001.jpg".to_string(),
                    cache_key: "dlsite:RJ123456:screenshot_0".to_string(),
                    cached: false,
                }],
                resolved_variables: Default::default(),
                reasoning: vec![],
            }],
            skipped_outputs: vec![],
        }
    }

    /// Every plan here has one output; staging is what turns downloads
    /// into moves on it, so the assertions read that output's lists.
    fn moves(plan: &OrganizationPlan) -> &[(String, String)] {
        &plan.outputs[0].moves
    }

    fn downloads(plan: &OrganizationPlan) -> &[PendingDownload] {
        &plan.outputs[0].downloads
    }

    /// A body sitting exactly on the limit is legitimate and must not be
    /// rejected by the cap that exists for the byte after it.
    #[test]
    fn a_body_at_the_limit_is_read_whole() {
        let body = vec![b'x'; TEST_LIMIT];
        let read = read_bounded(&mut body.as_slice(), TEST_LIMIT)
            .expect("a body at the limit must be read");
        assert_eq!(read.len(), TEST_LIMIT);
    }

    /// The read cap is the one that matters: a server that omits or
    /// understates `Content-Length` cannot make the client allocate past
    /// the limit anyway. `read_bounded` sees only the stream, so this is
    /// exactly the no-header case.
    #[test]
    fn a_body_past_the_limit_is_refused_and_the_error_names_the_limit() {
        let body = vec![b'x'; TEST_LIMIT + 1];
        let error = read_bounded(&mut body.as_slice(), TEST_LIMIT)
            .expect_err("a body past the limit must be refused");
        let message = format!("{error:#}");
        assert!(
            message.contains(&TEST_LIMIT.to_string()),
            "the reason reaches the user, so it must name the limit: {message}"
        );
    }

    #[test]
    fn a_fetched_download_becomes_a_move_from_the_staging_directory() {
        let work = tempfile::tempdir().unwrap();
        let staged = stage_plan_downloads(&plan_with_one_download(), work.path(), &|_| {
            Ok(b"jpegbytes".to_vec())
        })
        .expect("staging must succeed");

        assert!(
            downloads(&staged.plan).is_empty(),
            "downloads must be consumed"
        );
        assert!(staged.unfetched.is_empty());
        assert_eq!(
            moves(&staged.plan).len(),
            2,
            "one original move plus one staged image"
        );

        let (source, destination) = moves(&staged.plan)
            .iter()
            .find(|(_, d)| d == "Root/screenshots/image_001.jpg")
            .expect("the download must appear as a move to its declared destination");
        assert!(source.ends_with("/000"), "source must end with /000");
        assert_eq!(destination, "Root/screenshots/image_001.jpg");
        assert_eq!(
            std::fs::read(work.path().join(source)).unwrap(),
            b"jpegbytes".to_vec(),
            "the fetched bytes must be on disk where the move points"
        );
    }

    #[test]
    fn a_failed_fetch_is_reported_and_leaves_the_rest_of_the_plan_intact() {
        let work = tempfile::tempdir().unwrap();
        let staged = stage_plan_downloads(&plan_with_one_download(), work.path(), &|_| {
            Err(anyhow::anyhow!("connection refused"))
        })
        .expect("a failed screenshot must not fail the run");

        assert!(downloads(&staged.plan).is_empty());
        assert_eq!(
            moves(&staged.plan).len(),
            1,
            "only the original move survives"
        );
        assert_eq!(staged.unfetched.len(), 1);
        assert_eq!(
            staged.unfetched[0].0,
            "https://img.example.test/RJ123456_img_main.jpg"
        );
        assert_eq!(staged.unfetched[0].1, "Root/screenshots/image_001.jpg");
        assert!(staged.unfetched[0].2.contains("connection refused"));
    }

    #[test]
    fn a_plan_with_no_downloads_is_returned_unchanged_and_stages_nothing() {
        let work = tempfile::tempdir().unwrap();
        let mut plan = plan_with_one_download();
        plan.outputs[0].downloads.clear();

        let staged = stage_plan_downloads(&plan, work.path(), &|_| unreachable!("must not fetch"))
            .expect("staging must succeed");

        assert_eq!(moves(&staged.plan), moves(&plan));
        // The staging directory is not created if there are no downloads
        let staging_exists = work
            .path()
            .read_dir()
            .ok()
            .map(|mut entries| {
                entries.any(|e| {
                    if let Ok(entry) = e {
                        if let Some(name) = entry.file_name().to_str() {
                            name.starts_with(".arclain-downloads-")
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                })
            })
            .unwrap_or(false);
        assert!(!staging_exists, "no staging directory should exist");
    }

    #[test]
    fn three_downloads_with_middle_failure_indexes_correctly_and_reports_it() {
        let work = tempfile::tempdir().unwrap();

        let mut plan = plan_with_one_download();
        plan.outputs[0].downloads = vec![
            PendingDownload {
                product_id: Some("RJ123456".to_string()),
                url: "https://img.example.test/RJ123456_img_1.jpg".to_string(),
                dest_path: "Root/screenshots/image_001.jpg".to_string(),
                cache_key: "dlsite:RJ123456:screenshot_0".to_string(),
                cached: false,
            },
            PendingDownload {
                product_id: Some("RJ123456".to_string()),
                url: "https://img.example.test/RJ123456_img_2.jpg".to_string(),
                dest_path: "Root/screenshots/image_002.jpg".to_string(),
                cache_key: "dlsite:RJ123456:screenshot_1".to_string(),
                cached: false,
            },
            PendingDownload {
                product_id: Some("RJ123456".to_string()),
                url: "https://img.example.test/RJ123456_img_3.jpg".to_string(),
                dest_path: "Root/screenshots/image_003.jpg".to_string(),
                cache_key: "dlsite:RJ123456:screenshot_2".to_string(),
                cached: false,
            },
        ];

        let fetch_count = std::cell::Cell::new(0);
        let staged = stage_plan_downloads(&plan, work.path(), &|_download| {
            let count = fetch_count.get();
            fetch_count.set(count + 1);

            if count == 1 {
                // Middle download fails
                Err(anyhow::anyhow!("server error"))
            } else {
                Ok(b"imagedata".to_vec())
            }
        })
        .expect("staging must succeed");

        // Downloads should be consumed
        assert!(downloads(&staged.plan).is_empty());

        // Should have one original move plus two survivors (index 0 and 2)
        assert_eq!(
            moves(&staged.plan).len(),
            3,
            "one original plus two staged images"
        );

        // Verify the survivors have correct indices
        let move_sources: Vec<_> = moves(&staged.plan)
            .iter()
            .filter(|(_, d)| d.contains("image_00"))
            .map(|(s, _)| s.clone())
            .collect();

        let has_000 = move_sources.iter().any(|s| s.ends_with("/000"));
        let has_002 = move_sources.iter().any(|s| s.ends_with("/002"));
        assert!(has_000, "first successful download should be at index 000");
        assert!(has_002, "third successful download should be at index 002");

        // Verify the failed download is in unfetched with all three fields
        assert_eq!(staged.unfetched.len(), 1);
        assert_eq!(
            staged.unfetched[0].0,
            "https://img.example.test/RJ123456_img_2.jpg"
        );
        assert_eq!(staged.unfetched[0].1, "Root/screenshots/image_002.jpg");
        assert!(staged.unfetched[0].2.contains("server error"));

        // Verify the plan can be validated (both appliers call validate_paths before use)
        staged
            .plan
            .validate_paths()
            .expect("staged plan must pass validation");
    }
    #[test]
    fn addresses_off_the_public_internet_are_refused() {
        use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

        let refused: Vec<IpAddr> = vec![
            Ipv4Addr::new(127, 0, 0, 1).into(),
            Ipv4Addr::new(10, 0, 0, 5).into(),
            Ipv4Addr::new(172, 16, 3, 4).into(),
            Ipv4Addr::new(192, 168, 1, 1).into(),
            Ipv4Addr::new(169, 254, 169, 254).into(),
            Ipv4Addr::new(0, 0, 0, 0).into(),
            Ipv6Addr::LOCALHOST.into(),
            Ipv6Addr::UNSPECIFIED.into(),
            "fc00::1".parse::<Ipv6Addr>().unwrap().into(),
            "fe80::1".parse::<Ipv6Addr>().unwrap().into(),
            // An IPv4 loopback wearing an IPv6 mapping is the same host.
            "::ffff:127.0.0.1".parse::<Ipv6Addr>().unwrap().into(),
            // TEST-NET-3, a documentation range: reachable on nobody's
            // network, so a screenshot URL naming one is not a fetch
            // worth attempting.
            Ipv4Addr::new(203, 0, 113, 7).into(),
        ];
        for address in refused {
            assert!(
                is_off_public_internet(address),
                "{address} must not be reachable from a scraped screenshot URL"
            );
        }

        let allowed: Vec<IpAddr> = vec![
            Ipv4Addr::new(1, 1, 1, 1).into(),
            Ipv4Addr::new(8, 8, 8, 8).into(),
            "2606:4700::1111".parse::<Ipv6Addr>().unwrap().into(),
        ];
        for address in allowed {
            assert!(
                !is_off_public_internet(address),
                "{address} is an ordinary public address and must resolve"
            );
        }
    }
}
