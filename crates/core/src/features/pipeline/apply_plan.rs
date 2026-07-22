//! Apply an `OrganizationPlan`'s moves to an already-extracted directory.
//!
//! The pipeline executor has already extracted the source archive to a
//! work dir. This module takes a plan (from `RuleEngine::create_plan`)
//! and reorganizes files within the work dir according to `plan.moves`.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use crate::features::organization::engine::OrganizationPlan;
use crate::features::organization::organizer::{copy_plan_source, persist_plan_output};
use crate::utilities::CheckedRelativePath;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlanSwapPoint {
    BeforeBackup(usize),
    BeforePromote(usize),
    BeforeRevertPromotion(usize),
    BeforeRestoreBackup(usize),
}

fn checked_top_level_entries(root: &Path) -> Result<Vec<CheckedRelativePath>> {
    let mut entries = fs::read_dir(root)
        .with_context(|| format!("read top-level entries from {:?}", root))?
        .collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());

    entries
        .into_iter()
        .map(|entry| {
            let name = entry.file_name().into_string().map_err(|name| {
                anyhow::anyhow!(
                    "plan layout contains a non-Unicode top-level entry: {:?}",
                    name
                )
            })?;
            CheckedRelativePath::new(name)
        })
        .collect()
}

fn rename_noclobber(source: &Path, destination: &Path) -> Result<()> {
    match fs::symlink_metadata(destination) {
        Ok(_) => anyhow::bail!(
            "refusing to replace existing plan transaction path {:?}",
            destination
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error)
                .with_context(|| format!("inspect plan transaction path {:?}", destination));
        }
    }

    fs::rename(source, destination).with_context(|| {
        format!(
            "move plan transaction path {:?} -> {:?}",
            source, destination
        )
    })
}

fn rollback_layout_swap<F>(
    promoted: &[(PathBuf, PathBuf)],
    backed_up: &[(PathBuf, PathBuf)],
    hook: &mut F,
) -> Vec<String>
where
    F: FnMut(PlanSwapPoint) -> Result<()>,
{
    let mut failures = Vec::new();

    for (index, (work_path, staged_path)) in promoted.iter().rev().enumerate() {
        if let Err(error) = hook(PlanSwapPoint::BeforeRevertPromotion(index)) {
            failures.push(format!(
                "prepare to revert promoted entry {:?}: {error:#}",
                work_path
            ));
            continue;
        }
        if let Err(error) = rename_noclobber(work_path, staged_path) {
            failures.push(format!("revert promoted entry {:?}: {error:#}", work_path));
        }
    }

    for (index, (backup_path, work_path)) in backed_up.iter().rev().enumerate() {
        if let Err(error) = hook(PlanSwapPoint::BeforeRestoreBackup(index)) {
            failures.push(format!(
                "prepare to restore backup entry {:?}: {error:#}",
                backup_path
            ));
            continue;
        }
        if let Err(error) = rename_noclobber(backup_path, work_path) {
            failures.push(format!("restore backup entry {:?}: {error:#}", backup_path));
        }
    }

    failures
}

fn close_transaction_directories(
    staging_dir: tempfile::TempDir,
    backup_dir: tempfile::TempDir,
) -> Vec<String> {
    let staging_path = staging_dir.path().to_path_buf();
    let backup_path = backup_dir.path().to_path_buf();
    let mut failures = Vec::new();

    if let Err(error) = backup_dir.close() {
        failures.push(format!(
            "remove plan transaction backup {}: {error}",
            backup_path.display()
        ));
    }
    if let Err(error) = staging_dir.close() {
        failures.push(format!(
            "remove plan transaction staging {}: {error}",
            staging_path.display()
        ));
    }

    failures
}

/// Apply the plan's moves + generated_files to `work_dir` in place.
/// Does NOT handle `plan.downloads` (pipeline executor doesn't run HTTP
/// fetches; downloads are expected to be prefetched or skipped).
pub fn apply_plan_to_workdir(plan: &OrganizationPlan, work_dir: &Path) -> Result<()> {
    apply_plan_to_workdir_with_swap_hook(plan, work_dir, |_| Ok(()))
}

fn apply_plan_to_workdir_with_swap_hook<F>(
    plan: &OrganizationPlan,
    work_dir: &Path,
    mut hook: F,
) -> Result<()>
where
    F: FnMut(PlanSwapPoint) -> Result<()>,
{
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

    // Resolve every plan path before constructing output. This rejects static
    // symlinked parents while the original layout is untouched.
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

    // Construct the complete new layout by copying. The original work tree is
    // not mutated until every source and generated output has been persisted.
    for (source, destination) in &checked_moves {
        let source_path = source.resolve_under(work_dir)?;
        match fs::symlink_metadata(&source_path) {
            Ok(_) => {
                let source_path = source.resolve_under(work_dir)?;
                copy_plan_source(&staging, destination, &source_path).with_context(|| {
                    format!(
                        "stage organization source {:?} at {:?}",
                        source.as_path(),
                        destination.as_path()
                    )
                })?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                tracing::warn!("[apply_plan] source missing: {}", source_path.display());
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("inspect move source {:?}", source_path));
            }
        }
    }
    for ((_, content), checked_path) in plan.generated_files.iter().zip(&checked_generated) {
        let mut bytes = std::io::Cursor::new(content.as_bytes());
        persist_plan_output(&staging, checked_path, &mut bytes)
            .with_context(|| format!("stage generated output {:?}", checked_path.as_path()))?;
    }

    // Capture both trees before mutation so a read/validation failure remains
    // side-effect free.
    let original_entries = checked_top_level_entries(work_dir)?;
    let staged_entries = checked_top_level_entries(&staging)?;
    let mut backup_dir = tempfile::Builder::new()
        .prefix(".arclain-plan-backup-")
        .tempdir_in(work_parent)
        .context("create pipeline plan backup directory")?;
    // Once original entries enter the backup, an unwind must preserve it. The
    // directory is explicitly closed only after commit or successful rollback.
    backup_dir.disable_cleanup(true);
    let backup = backup_dir.path().to_path_buf();

    let mut backed_up = Vec::new();
    let mut promoted = Vec::new();
    let swap_result: Result<()> = (|| {
        for (index, relative) in original_entries.iter().enumerate() {
            hook(PlanSwapPoint::BeforeBackup(index))?;
            let source = relative.resolve_under(work_dir)?;
            let destination = relative.resolve_under(&backup)?;
            rename_noclobber(&source, &destination)?;
            backed_up.push((destination, source));
        }

        for (index, relative) in staged_entries.iter().enumerate() {
            hook(PlanSwapPoint::BeforePromote(index))?;
            let source = relative.resolve_under(&staging)?;
            let destination = relative.resolve_under(work_dir)?;
            rename_noclobber(&source, &destination)?;
            promoted.push((destination, source));
        }

        Ok(())
    })();

    if let Err(swap_error) = swap_result {
        let rollback_failures = rollback_layout_swap(&promoted, &backed_up, &mut hook);
        if !rollback_failures.is_empty() {
            let staging_recovery = staging_dir.keep();
            let backup_recovery = backup_dir.keep();
            anyhow::bail!(
                "organization layout transaction failed: {swap_error:#}; rollback failed: {}; \
                 recovery staging path: {}; recovery backup path: {}",
                rollback_failures.join("; "),
                staging_recovery.display(),
                backup_recovery.display()
            );
        }

        let cleanup_failures = close_transaction_directories(staging_dir, backup_dir);
        if !cleanup_failures.is_empty() {
            anyhow::bail!(
                "organization layout transaction failed: {swap_error:#}; layout was restored, \
                 but cleanup failed: {}",
                cleanup_failures.join("; ")
            );
        }
        return Err(swap_error);
    }

    let cleanup_failures = close_transaction_directories(staging_dir, backup_dir);
    if !cleanup_failures.is_empty() {
        anyhow::bail!(
            "organization layout committed, but transaction cleanup failed: {}",
            cleanup_failures.join("; ")
        );
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

    fn plan_recovery_directories(parent: &Path) -> Vec<PathBuf> {
        fs::read_dir(parent)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".arclain-plan-")
            })
            .map(|entry| entry.path())
            .collect()
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
    fn apply_plan_rejects_nested_destination_collision_without_touching_sources() {
        let temp = tempfile::tempdir().unwrap();
        let work = temp.path().join("work");
        fs::create_dir(&work).unwrap();
        fs::create_dir(work.join("dir")).unwrap();
        fs::write(work.join("dir/nested.bin"), b"original nested").unwrap();
        fs::write(work.join("replacement.bin"), b"replacement").unwrap();
        let plan = empty_plan(
            vec![
                ("dir".into(), "Out".into()),
                ("replacement.bin".into(), "Out/nested.bin".into()),
            ],
            vec![],
        );

        assert!(apply_plan_to_workdir(&plan, &work).is_err());
        assert_eq!(
            fs::read(work.join("dir/nested.bin")).unwrap(),
            b"original nested"
        );
        assert_eq!(
            fs::read(work.join("replacement.bin")).unwrap(),
            b"replacement"
        );
        assert_no_plan_staging_directories(temp.path());
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn apply_plan_rejects_symlink_nested_inside_moved_directory() {
        let temp = tempfile::tempdir().unwrap();
        let work = temp.path().join("work");
        let outside = temp.path().join("outside");
        fs::create_dir(&work).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::create_dir(work.join("dir")).unwrap();
        fs::write(outside.join("secret.bin"), b"secret").unwrap();
        symlink_dir_for_test(&outside, &work.join("dir/linked"));
        let plan = empty_plan(vec![("dir".into(), "Out".into())], vec![]);

        assert!(apply_plan_to_workdir(&plan, &work).is_err());
        assert!(work.join("dir/linked").exists());
        assert_eq!(fs::read(outside.join("secret.bin")).unwrap(), b"secret");
        assert_no_plan_staging_directories(temp.path());
    }

    #[test]
    fn apply_plan_preserves_source_file_permissions() {
        let temp = tempfile::tempdir().unwrap();
        let work = temp.path().join("work");
        fs::create_dir(&work).unwrap();
        let source = work.join("game.bin");
        fs::write(&source, b"game").unwrap();
        let mut permissions = fs::metadata(&source).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&source, permissions).unwrap();
        let plan = empty_plan(vec![("game.bin".into(), "Out/game.bin".into())], vec![]);

        apply_plan_to_workdir(&plan, &work).unwrap();

        let output = work.join("Out/game.bin");
        assert!(fs::metadata(&output).unwrap().permissions().readonly());
        let mut permissions = fs::metadata(&output).unwrap().permissions();
        permissions.set_readonly(false);
        fs::set_permissions(output, permissions).unwrap();
    }

    #[test]
    fn apply_plan_restores_complete_layout_when_second_promotion_fails() {
        let temp = tempfile::tempdir().unwrap();
        let work = temp.path().join("work");
        fs::create_dir(&work).unwrap();
        fs::write(work.join("alpha.bin"), b"alpha").unwrap();
        fs::write(work.join("beta.bin"), b"beta").unwrap();
        fs::write(work.join("unplanned.bin"), b"unplanned").unwrap();
        let plan = empty_plan(
            vec![
                ("alpha.bin".into(), "Alpha/output.bin".into()),
                ("beta.bin".into(), "Beta/output.bin".into()),
            ],
            vec![],
        );

        let error = apply_plan_to_workdir_with_swap_hook(&plan, &work, |point| {
            if point == PlanSwapPoint::BeforePromote(1) {
                anyhow::bail!("injected second-promotion failure");
            }
            Ok(())
        })
        .unwrap_err();

        assert!(
            format!("{error:#}").contains("injected second-promotion failure"),
            "unexpected error: {error:#}"
        );
        assert_eq!(fs::read(work.join("alpha.bin")).unwrap(), b"alpha");
        assert_eq!(fs::read(work.join("beta.bin")).unwrap(), b"beta");
        assert_eq!(fs::read(work.join("unplanned.bin")).unwrap(), b"unplanned");
        assert!(!work.join("Alpha").exists());
        assert!(!work.join("Beta").exists());
        assert_no_plan_staging_directories(temp.path());
    }

    #[test]
    fn apply_plan_retains_recovery_directories_when_promotion_rollback_fails() {
        let temp = tempfile::tempdir().unwrap();
        let work = temp.path().join("work");
        fs::create_dir(&work).unwrap();
        fs::write(work.join("alpha.bin"), b"alpha").unwrap();
        fs::write(work.join("beta.bin"), b"beta").unwrap();
        let plan = empty_plan(
            vec![
                ("alpha.bin".into(), "Alpha/output.bin".into()),
                ("beta.bin".into(), "Beta/output.bin".into()),
            ],
            vec![],
        );

        let error = apply_plan_to_workdir_with_swap_hook(&plan, &work, |point| {
            if matches!(
                point,
                PlanSwapPoint::BeforePromote(1) | PlanSwapPoint::BeforeRevertPromotion(0)
            ) {
                anyhow::bail!("injected transactional swap failure");
            }
            Ok(())
        })
        .unwrap_err();
        let error = format!("{error:#}");
        let recovery_directories = plan_recovery_directories(temp.path());

        assert!(error.contains("recovery"), "unexpected error: {error}");
        assert_eq!(recovery_directories.len(), 2, "unexpected recovery state");
        for recovery_directory in recovery_directories {
            assert!(
                error.contains(&recovery_directory.display().to_string()),
                "recovery path missing from error: {error}"
            );
        }
        assert_eq!(fs::read(work.join("alpha.bin")).unwrap(), b"alpha");
        assert_eq!(fs::read(work.join("beta.bin")).unwrap(), b"beta");
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
