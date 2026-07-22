//! Apply an `OrganizationPlan`'s moves to an already-extracted directory.
//!
//! The pipeline executor has already extracted the source archive to a
//! work dir. This module takes a plan (from `RuleEngine::create_plan`)
//! and reorganizes files within the work dir according to `plan.moves`.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use crate::features::organization::engine::OrganizationPlan;
use crate::features::organization::organizer::persist_plan_output;
use crate::utilities::CheckedRelativePath;

struct StagedMoveRollback {
    moves: Vec<(PathBuf, PathBuf)>,
    armed: bool,
}

impl StagedMoveRollback {
    fn new() -> Self {
        Self {
            moves: Vec::new(),
            armed: true,
        }
    }

    fn record(&mut self, staged: PathBuf, source: PathBuf) {
        self.moves.push((staged, source));
    }

    fn disarm(&mut self) {
        self.armed = false;
        self.moves.clear();
    }

    fn rollback(&mut self) -> Result<()> {
        let mut failures = Vec::new();

        for (staged, source) in self.moves.iter().rev() {
            match fs::symlink_metadata(staged) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    failures.push(format!("inspect staged source {:?}: {error}", staged));
                    continue;
                }
            }

            match fs::symlink_metadata(source) {
                Ok(_) => {
                    failures.push(format!(
                        "refusing to replace existing rollback source {:?}",
                        source
                    ));
                    continue;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    failures.push(format!("inspect rollback source {:?}: {error}", source));
                    continue;
                }
            }

            if let Err(error) = fs::rename(staged, source) {
                failures.push(format!(
                    "restore staged source {:?} to {:?}: {error}",
                    staged, source
                ));
            }
        }

        if failures.is_empty() {
            self.disarm();
            Ok(())
        } else {
            anyhow::bail!("{}", failures.join("; "))
        }
    }
}

impl Drop for StagedMoveRollback {
    fn drop(&mut self) {
        if self.armed {
            if let Err(error) = self.rollback() {
                tracing::error!("failed to roll back staged organization moves: {error:#}");
            }
        }
    }
}

/// Apply the plan's moves + generated_files to `work_dir` in place.
/// Does NOT handle `plan.downloads` (pipeline executor doesn't run HTTP
/// fetches; downloads are expected to be prefetched or skipped).
pub fn apply_plan_to_workdir(plan: &OrganizationPlan, work_dir: &Path) -> Result<()> {
    plan.validate_paths()?;

    let root_folder = CheckedRelativePath::new(&plan.root_folder)?;
    root_folder.resolve_under(work_dir)?;

    let checked_moves = plan
        .moves
        .iter()
        .map(|(source, destination)| {
            Ok((
                CheckedRelativePath::new(source)?,
                CheckedRelativePath::new(destination)?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let checked_generated = plan
        .generated_files
        .iter()
        .map(|(path, _)| CheckedRelativePath::new(path))
        .collect::<Result<Vec<_>>>()?;
    let checked_downloads = plan
        .downloads
        .iter()
        .map(|download| CheckedRelativePath::new(&download.dest_path))
        .collect::<Result<Vec<_>>>()?;

    // Resolve the final paths before touching the work tree. This catches
    // static symlinked parents while the original layout is still intact.
    for (source, destination) in &checked_moves {
        source.resolve_under(work_dir)?;
        destination.resolve_under(work_dir)?;
    }
    for path in &checked_generated {
        path.resolve_under(work_dir)?;
    }
    for path in &checked_downloads {
        path.resolve_under(work_dir)?;
    }

    // Stage 1: stage all moves in an owned sibling directory. Keeping staging
    // outside the work tree prevents collisions with valid archive paths and
    // still guarantees same-filesystem renames.
    let work_root = work_dir
        .canonicalize()
        .with_context(|| format!("canonicalize work directory {:?}", work_dir))?;
    let work_parent = work_root
        .parent()
        .context("pipeline work directory has no parent")?;
    let staging_dir = tempfile::Builder::new()
        .prefix(".arclain-plan-")
        .tempdir_in(work_parent)
        .context("create pipeline plan staging directory")?;
    let staging = staging_dir.path().to_path_buf();
    let mut rollback = StagedMoveRollback::new();

    let staging_result: Result<()> = (|| {
        for (source, destination) in &checked_moves {
            let src = source.resolve_under(work_dir)?;
            let dest = destination.resolve_under(&staging)?;
            if !src.exists() {
                tracing::warn!("[apply_plan] source missing: {}", src.display());
                continue;
            }
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).with_context(|| format!("mkdir {:?}", parent))?;
            }

            let src = source.resolve_under(work_dir)?;
            let dest = destination.resolve_under(&staging)?;
            fs::rename(&src, &dest)
                .with_context(|| format!("stage move {:?} -> {:?}", src, dest))?;
            rollback.record(dest, src);
        }
        Ok(())
    })();

    if let Err(staging_error) = staging_result {
        if let Err(rollback_error) = rollback.rollback() {
            rollback.disarm();
            let recovery_path = staging_dir.keep();
            return Err(anyhow::anyhow!(
                "staging organization moves failed: {staging_error:#}; rollback failed: \
                 {rollback_error:#}; staged sources retained at {}",
                recovery_path.display()
            ));
        }
        return Err(staging_error);
    }
    rollback.disarm();

    // Stage 2: remove the old layout. Staging is an owned sibling and cannot
    // be mistaken for a plan-controlled work-tree entry.
    for entry in fs::read_dir(work_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            fs::remove_dir_all(&path).ok();
        } else {
            fs::remove_file(&path).ok();
        }
    }

    // Move staging contents up to work_dir root
    for entry in fs::read_dir(&staging)? {
        let entry = entry?;
        let top_level = CheckedRelativePath::new(entry.file_name().to_string_lossy())?;
        let from = top_level.resolve_under(&staging)?;
        let to = top_level.resolve_under(work_dir)?;
        fs::rename(&from, &to).with_context(|| format!("move {:?} -> {:?}", from, to))?;
    }
    // Stage 3: write generated files (metadata.json etc.)
    for ((_, content), checked_path) in plan.generated_files.iter().zip(&checked_generated) {
        let mut bytes = std::io::Cursor::new(content.as_bytes());
        persist_plan_output(work_dir, checked_path, &mut bytes)
            .with_context(|| format!("write generated output {:?}", checked_path.as_path()))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(p: &Path) {
        fs::create_dir_all(p.parent().unwrap()).ok();
        fs::write(p, b"test").unwrap();
    }

    fn assert_no_plan_staging_directories(parent: &Path) {
        let staging_directories = fs::read_dir(parent)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".arclain-plan-")
            })
            .collect::<Vec<_>>();
        assert!(
            staging_directories.is_empty(),
            "staging directories remain: {staging_directories:?}"
        );
    }

    #[cfg(unix)]
    fn symlink_dir_for_test(target: &Path, link: &Path) {
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    #[cfg(windows)]
    fn symlink_dir_for_test(target: &Path, link: &Path) {
        std::os::windows::fs::symlink_dir(target, link)
            .expect("Windows symlink support is required for this containment regression");
    }

    fn empty_plan(
        moves: Vec<(String, String)>,
        generated: Vec<(String, String)>,
    ) -> OrganizationPlan {
        OrganizationPlan {
            rule_name: "test".into(),
            root_folder: "MyGame".into(),
            root_folder_template: "MyGame".into(),
            moves,
            generated_files: generated,
            downloads: vec![],
            use_standard_layout: true,
            resolved_variables: Default::default(),
        }
    }

    #[test]
    fn apply_plan_moves_files_into_subfolder() {
        let tmp = tempfile::tempdir().unwrap();
        touch(&tmp.path().join("game.exe"));
        touch(&tmp.path().join("data/sprites.dat"));

        let plan = empty_plan(
            vec![
                ("game.exe".into(), "MyGame/game.exe".into()),
                ("data/sprites.dat".into(), "MyGame/data/sprites.dat".into()),
            ],
            vec![],
        );

        apply_plan_to_workdir(&plan, tmp.path()).unwrap();

        assert!(tmp.path().join("MyGame/game.exe").exists());
        assert!(tmp.path().join("MyGame/data/sprites.dat").exists());
        assert!(!tmp.path().join("game.exe").exists());
        assert!(!tmp.path().join("data").exists());
    }

    #[test]
    fn apply_plan_preserves_source_named_like_legacy_staging_directory() {
        let tmp = tempfile::tempdir().unwrap();
        touch(&tmp.path().join("__pipeline_staging__/game.exe"));
        let plan = empty_plan(
            vec![(
                "__pipeline_staging__/game.exe".into(),
                "MyGame/game.exe".into(),
            )],
            vec![],
        );

        apply_plan_to_workdir(&plan, tmp.path()).unwrap();

        assert_eq!(
            fs::read(tmp.path().join("MyGame/game.exe")).unwrap(),
            b"test"
        );
    }

    #[test]
    fn apply_plan_allows_destination_named_like_legacy_staging_directory() {
        let tmp = tempfile::tempdir().unwrap();
        touch(&tmp.path().join("game.exe"));
        let plan = empty_plan(
            vec![("game.exe".into(), "__pipeline_staging__/game.exe".into())],
            vec![],
        );

        apply_plan_to_workdir(&plan, tmp.path()).unwrap();

        assert_eq!(
            fs::read(tmp.path().join("__pipeline_staging__/game.exe")).unwrap(),
            b"test"
        );
    }

    #[test]
    fn apply_plan_preserves_two_file_move_cycles() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a.bin"), b"a").unwrap();
        fs::write(tmp.path().join("b.bin"), b"b").unwrap();
        let plan = empty_plan(
            vec![
                ("a.bin".into(), "b.bin".into()),
                ("b.bin".into(), "a.bin".into()),
            ],
            vec![],
        );

        apply_plan_to_workdir(&plan, tmp.path()).unwrap();

        assert_eq!(fs::read(tmp.path().join("a.bin")).unwrap(), b"b");
        assert_eq!(fs::read(tmp.path().join("b.bin")).unwrap(), b"a");
    }

    #[test]
    fn apply_plan_rolls_back_sources_when_later_staging_path_conflicts() {
        let temp = tempfile::tempdir().unwrap();
        let work = temp.path().join("work");
        fs::create_dir(&work).unwrap();
        fs::write(work.join("a.bin"), b"a").unwrap();
        fs::write(work.join("b.bin"), b"b").unwrap();
        let plan = empty_plan(
            vec![
                ("a.bin".into(), "conflict".into()),
                ("b.bin".into(), "conflict/child.bin".into()),
            ],
            vec![],
        );

        assert!(apply_plan_to_workdir(&plan, &work).is_err());
        assert_eq!(fs::read(work.join("a.bin")).unwrap(), b"a");
        assert_eq!(fs::read(work.join("b.bin")).unwrap(), b"b");
        assert_no_plan_staging_directories(temp.path());
    }

    #[test]
    fn apply_plan_rolls_back_directories_and_cycles_on_staging_failure() {
        let temp = tempfile::tempdir().unwrap();
        let work = temp.path().join("work");
        fs::create_dir(&work).unwrap();
        touch(&work.join("dir/nested.bin"));
        fs::write(work.join("a.bin"), b"a").unwrap();
        fs::write(work.join("b.bin"), b"b").unwrap();
        fs::write(work.join("c.bin"), b"c").unwrap();
        fs::write(work.join("d.bin"), b"d").unwrap();
        let plan = empty_plan(
            vec![
                ("dir".into(), "StagedDir".into()),
                ("a.bin".into(), "b.bin".into()),
                ("b.bin".into(), "a.bin".into()),
                ("c.bin".into(), "conflict".into()),
                ("d.bin".into(), "conflict/child.bin".into()),
            ],
            vec![],
        );

        assert!(apply_plan_to_workdir(&plan, &work).is_err());
        assert_eq!(fs::read(work.join("dir/nested.bin")).unwrap(), b"test");
        assert_eq!(fs::read(work.join("a.bin")).unwrap(), b"a");
        assert_eq!(fs::read(work.join("b.bin")).unwrap(), b"b");
        assert_eq!(fs::read(work.join("c.bin")).unwrap(), b"c");
        assert_eq!(fs::read(work.join("d.bin")).unwrap(), b"d");
        assert_no_plan_staging_directories(temp.path());
    }

    #[test]
    fn apply_plan_writes_generated_files() {
        let tmp = tempfile::tempdir().unwrap();
        touch(&tmp.path().join("game.exe"));

        let plan = empty_plan(
            vec![("game.exe".into(), "MyGame/game.exe".into())],
            vec![("MyGame/metadata.json".into(), r#"{"title":"Test"}"#.into())],
        );

        apply_plan_to_workdir(&plan, tmp.path()).unwrap();

        let meta = fs::read_to_string(tmp.path().join("MyGame/metadata.json")).unwrap();
        assert!(meta.contains("Test"));
    }

    #[test]
    fn apply_plan_skips_missing_sources() {
        let tmp = tempfile::tempdir().unwrap();
        let plan = empty_plan(
            vec![("ghost.exe".into(), "MyGame/ghost.exe".into())],
            vec![],
        );
        // Should not error on missing source — just warn
        apply_plan_to_workdir(&plan, tmp.path()).unwrap();
    }

    #[test]
    fn apply_plan_revalidates_hand_built_escaping_destination() {
        let temp = tempfile::tempdir().unwrap();
        let work = temp.path().join("work");
        fs::create_dir(&work).unwrap();
        touch(&work.join("game.exe"));
        let outside = temp.path().join("escaped.exe");
        let plan = empty_plan(vec![("game.exe".into(), "../escaped.exe".into())], vec![]);

        assert!(apply_plan_to_workdir(&plan, &work).is_err());
        assert!(!outside.exists());
        assert!(work.join("game.exe").exists());
    }

    #[test]
    fn apply_plan_rejects_escaping_source_before_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let work = temp.path().join("work");
        fs::create_dir(&work).unwrap();
        let outside = temp.path().join("outside.exe");
        touch(&outside);
        let plan = empty_plan(
            vec![("../outside.exe".into(), "MyGame/outside.exe".into())],
            vec![],
        );

        assert!(apply_plan_to_workdir(&plan, &work).is_err());
        assert_eq!(fs::read(&outside).unwrap(), b"test");
    }

    #[test]
    fn apply_plan_rejects_escaping_generated_path_before_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let work = temp.path().join("work");
        fs::create_dir(&work).unwrap();
        touch(&work.join("game.exe"));
        let outside = temp.path().join("generated.txt");
        let plan = empty_plan(vec![], vec![("../generated.txt".into(), "bad".into())]);

        assert!(apply_plan_to_workdir(&plan, &work).is_err());
        assert!(!outside.exists());
        assert!(work.join("game.exe").exists());
    }

    #[test]
    fn apply_plan_rejects_escaping_download_destination_before_mutation() {
        use crate::features::organization::engine::PendingDownload;

        let temp = tempfile::tempdir().unwrap();
        let work = temp.path().join("work");
        fs::create_dir(&work).unwrap();
        touch(&work.join("game.exe"));
        let mut plan = empty_plan(vec![], vec![]);
        plan.downloads.push(PendingDownload {
            product_id: None,
            url: "https://example.invalid/file".into(),
            dest_path: "../download.bin".into(),
            cache_key: "test".into(),
            cached: false,
        });

        assert!(apply_plan_to_workdir(&plan, &work).is_err());
        assert!(work.join("game.exe").exists());
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn apply_plan_resolves_download_destinations_before_mutation() {
        use crate::features::organization::engine::PendingDownload;

        let temp = tempfile::tempdir().unwrap();
        let work = temp.path().join("work");
        let outside = temp.path().join("outside");
        fs::create_dir(&work).unwrap();
        fs::create_dir(&outside).unwrap();
        touch(&work.join("game.exe"));
        symlink_dir_for_test(&outside, &work.join("linked"));
        let mut plan = empty_plan(vec![("game.exe".into(), "MyGame/game.exe".into())], vec![]);
        plan.downloads.push(PendingDownload {
            product_id: None,
            url: "https://example.invalid/image.jpg".into(),
            dest_path: "linked/image.jpg".into(),
            cache_key: "test".into(),
            cached: false,
        });

        assert!(apply_plan_to_workdir(&plan, &work).is_err());
        assert!(work.join("game.exe").exists());
        assert!(!outside.join("image.jpg").exists());
    }

    #[test]
    fn apply_plan_rejects_duplicate_normalized_destinations_before_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let work = temp.path().join("work");
        fs::create_dir(&work).unwrap();
        touch(&work.join("game.exe"));
        let plan = empty_plan(
            vec![("game.exe".into(), r"MyGame\output.bin".into())],
            vec![("MyGame/output.bin".into(), "generated".into())],
        );

        assert!(apply_plan_to_workdir(&plan, &work).is_err());
        assert!(work.join("game.exe").exists());
    }

    #[test]
    fn apply_plan_rejects_normalized_download_destination_collision() {
        use crate::features::organization::engine::PendingDownload;

        let temp = tempfile::tempdir().unwrap();
        let work = temp.path().join("work");
        fs::create_dir(&work).unwrap();
        touch(&work.join("game.exe"));
        let mut plan = empty_plan(
            vec![("game.exe".into(), r"MyGame\output.bin".into())],
            vec![],
        );
        plan.downloads.push(PendingDownload {
            product_id: None,
            url: "https://example.invalid/file".into(),
            dest_path: "MyGame/output.bin".into(),
            cache_key: "test".into(),
            cached: false,
        });

        assert!(apply_plan_to_workdir(&plan, &work).is_err());
        assert!(work.join("game.exe").exists());
    }

    #[cfg(unix)]
    #[test]
    fn apply_plan_rejects_static_symlinked_generated_parent_before_mutation() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let work = temp.path().join("work");
        let outside = temp.path().join("outside");
        fs::create_dir(&work).unwrap();
        fs::create_dir(&outside).unwrap();
        touch(&work.join("game.exe"));
        symlink(&outside, work.join("linked")).unwrap();
        let plan = empty_plan(
            vec![("game.exe".into(), "MyGame/game.exe".into())],
            vec![("linked/generated.txt".into(), "bad".into())],
        );

        assert!(apply_plan_to_workdir(&plan, &work).is_err());
        assert!(!outside.join("generated.txt").exists());
        assert!(work.join("game.exe").exists());
    }

    #[cfg(windows)]
    #[test]
    fn apply_plan_rejects_static_symlinked_generated_parent_before_mutation() {
        use std::os::windows::fs::symlink_dir;

        let temp = tempfile::tempdir().unwrap();
        let work = temp.path().join("work");
        let outside = temp.path().join("outside");
        fs::create_dir(&work).unwrap();
        fs::create_dir(&outside).unwrap();
        touch(&work.join("game.exe"));
        symlink_dir(&outside, work.join("linked"))
            .expect("Windows symlink support is required for this containment regression");
        let plan = empty_plan(
            vec![("game.exe".into(), "MyGame/game.exe".into())],
            vec![("linked/generated.txt".into(), "bad".into())],
        );

        assert!(apply_plan_to_workdir(&plan, &work).is_err());
        assert!(!outside.join("generated.txt").exists());
        assert!(work.join("game.exe").exists());
    }
}
