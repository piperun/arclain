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
    staged.downloads = Vec::new();

    if plan.downloads.is_empty() {
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

    for (index, download) in plan.downloads.iter().enumerate() {
        match fetch(download) {
            Ok(bytes) => {
                let name = format!("{index:03}");
                match std::fs::write(staging.join(&name), &bytes) {
                    Ok(_) => {
                        staged
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
                }
            }
            Err(error) => {
                unfetched.push((
                    download.url.clone(),
                    download.dest_path.clone(),
                    format!("{error:#}"),
                ));
            }
        }
    }

    Ok(StagedPlan {
        plan: staged,
        unfetched,
    })
}

/// One HTTP fetcher for a whole plan's downloads, holding a single
/// client so a batch from one host reuses its connection rather than
/// re-handshaking per image.
pub fn http_downloader() -> Result<impl Fn(&PendingDownload) -> Result<Vec<u8>>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("Arclain/1.0")
        .build()?;
    Ok(move |download: &PendingDownload| {
        let response = client.get(&download.url).send()?;
        if !response.status().is_success() {
            anyhow::bail!("status {}", response.status());
        }
        Ok(response.bytes()?.to_vec())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::organization::engine::{OrganizationPlan, PendingDownload};

    fn plan_with_one_download() -> OrganizationPlan {
        OrganizationPlan {
            rule_name: "Test".to_string(),
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
            use_standard_layout: false,
            resolved_variables: Default::default(),
        }
    }

    #[test]
    fn a_fetched_download_becomes_a_move_from_the_staging_directory() {
        let work = tempfile::tempdir().unwrap();
        let staged = stage_plan_downloads(&plan_with_one_download(), work.path(), &|_| {
            Ok(b"jpegbytes".to_vec())
        })
        .expect("staging must succeed");

        assert!(
            staged.plan.downloads.is_empty(),
            "downloads must be consumed"
        );
        assert!(staged.unfetched.is_empty());
        assert_eq!(
            staged.plan.moves.len(),
            2,
            "one original move plus one staged image"
        );

        let (source, destination) = staged
            .plan
            .moves
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

        assert!(staged.plan.downloads.is_empty());
        assert_eq!(
            staged.plan.moves.len(),
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
        plan.downloads.clear();

        let staged = stage_plan_downloads(&plan, work.path(), &|_| unreachable!("must not fetch"))
            .expect("staging must succeed");

        assert_eq!(staged.plan.moves, plan.moves);
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
        plan.downloads = vec![
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
        assert!(staged.plan.downloads.is_empty());

        // Should have one original move plus two survivors (index 0 and 2)
        assert_eq!(
            staged.plan.moves.len(),
            3,
            "one original plus two staged images"
        );

        // Verify the survivors have correct indices
        let move_sources: Vec<_> = staged
            .plan
            .moves
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
}
