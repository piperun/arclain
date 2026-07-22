//! Apply an `OrganizationPlan`'s moves to an already-extracted directory.
//!
//! The pipeline executor has already extracted the source archive to a
//! work dir. This module takes a plan (from `RuleEngine::create_plan`)
//! and reorganizes files within the work dir according to `plan.moves`.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use crate::features::organization::engine::OrganizationPlan;
#[cfg(test)]
use crate::features::organization::organizer::open_plan_metadata_handle;
use crate::features::organization::organizer::{
    apply_deferred_plan_metadata, copy_plan_source, persist_plan_output, DeferredPlanMetadata,
};
use crate::utilities::CheckedRelativePath;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlanSwapPoint {
    BeforeBackup(usize),
    BeforePromote(usize),
    BeforeRevertPromotion(usize),
    BeforeRestoreBackup(usize),
}

#[derive(Debug, PartialEq, Eq)]
enum PlanApplyOutcome {
    Committed,
    CommittedWithCleanupWarnings {
        warnings: Vec<String>,
        retained_paths: Vec<PathBuf>,
    },
}

fn report_plan_apply_outcome(outcome: PlanApplyOutcome) {
    if let PlanApplyOutcome::CommittedWithCleanupWarnings {
        warnings,
        retained_paths,
    } = outcome
    {
        for warning in warnings {
            tracing::warn!("[apply_plan] {warning}");
        }
        for path in retained_paths {
            tracing::warn!(
                "[apply_plan] committed organization recovery path retained: {}",
                path.display()
            );
        }
    }
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
    hook: F,
) -> Result<()>
where
    F: FnMut(PlanSwapPoint) -> Result<()>,
{
    apply_plan_to_workdir_with_hooks(plan, work_dir, hook, |directory| {
        directory
            .close()
            .context("remove pre-backup plan staging directory")
    })
}

fn apply_plan_to_workdir_with_hooks<F, C>(
    plan: &OrganizationPlan,
    work_dir: &Path,
    hook: F,
    close_prebackup_staging: C,
) -> Result<()>
where
    F: FnMut(PlanSwapPoint) -> Result<()>,
    C: FnOnce(tempfile::TempDir) -> Result<()>,
{
    apply_plan_to_workdir_with_cleanup_hooks(
        plan,
        work_dir,
        hook,
        close_prebackup_staging,
        |directory| {
            directory
                .close()
                .context("remove committed plan staging directory")
        },
        |directory| {
            directory
                .close()
                .context("remove committed plan backup directory")
        },
    )
}

fn apply_plan_to_workdir_with_cleanup_hooks<F, C, S, B>(
    plan: &OrganizationPlan,
    work_dir: &Path,
    hook: F,
    close_prebackup_staging: C,
    close_committed_staging: S,
    close_committed_backup: B,
) -> Result<()>
where
    F: FnMut(PlanSwapPoint) -> Result<()>,
    C: FnOnce(tempfile::TempDir) -> Result<()>,
    S: FnOnce(tempfile::TempDir) -> Result<()>,
    B: FnOnce(tempfile::TempDir) -> Result<()>,
{
    let outcome = apply_plan_transaction_with_hooks(
        plan,
        work_dir,
        hook,
        close_prebackup_staging,
        close_committed_staging,
        close_committed_backup,
    )?;
    report_plan_apply_outcome(outcome);
    Ok(())
}

fn apply_plan_transaction_with_hooks<F, C, S, B>(
    plan: &OrganizationPlan,
    work_dir: &Path,
    mut hook: F,
    close_prebackup_staging: C,
    close_committed_staging: S,
    close_committed_backup: B,
) -> Result<PlanApplyOutcome>
where
    F: FnMut(PlanSwapPoint) -> Result<()>,
    C: FnOnce(tempfile::TempDir) -> Result<()>,
    S: FnOnce(tempfile::TempDir) -> Result<()>,
    B: FnOnce(tempfile::TempDir) -> Result<()>,
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
    let prebackup_result: Result<(
        Vec<CheckedRelativePath>,
        Vec<CheckedRelativePath>,
        Vec<DeferredPlanMetadata>,
        tempfile::TempDir,
    )> = (|| {
        let mut deferred_metadata = Vec::new();
        for (source, destination) in &checked_moves {
            let source_path = source.resolve_under(work_dir)?;
            match fs::symlink_metadata(&source_path) {
                Ok(_) => {
                    let source_path = source.resolve_under(work_dir)?;
                    copy_plan_source(&staging, destination, &source_path, &mut deferred_metadata)
                        .with_context(|| {
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
        let backup_dir = tempfile::Builder::new()
            .prefix(".arclain-plan-backup-")
            .tempdir_in(work_parent)
            .context("create pipeline plan backup directory")?;
        Ok((
            original_entries,
            staged_entries,
            deferred_metadata,
            backup_dir,
        ))
    })();

    let (original_entries, staged_entries, mut deferred_metadata, mut backup_dir) =
        match prebackup_result {
            Ok(result) => result,
            Err(staging_error) => {
                let staging_path = staging_dir.path().to_path_buf();
                if let Err(cleanup_error) = close_prebackup_staging(staging_dir) {
                    anyhow::bail!(
                        "building organization staging layout failed: {staging_error:#}; removing \
                     pre-backup staging directory failed: {cleanup_error:#}; staging path: {}",
                        staging_path.display()
                    );
                }
                return Err(staging_error);
            }
        };

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

        apply_deferred_plan_metadata(work_dir, &mut deferred_metadata)
            .context("preserve committed organization metadata")?;

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

    // The layout is committed after every promotion and final metadata update
    // succeeds. Cleanup can no longer be represented as an ordinary failure:
    // callers would discard a valid committed work tree. Close the now-empty
    // staging directory first while the complete original backup still exists.
    let staging_path = staging_dir.path().to_path_buf();
    if let Err(error) = close_committed_staging(staging_dir) {
        let backup_path = backup_dir.keep();
        let mut retained_paths = vec![backup_path.clone()];
        if staging_path.exists() {
            retained_paths.insert(0, staging_path.clone());
        }
        return Ok(PlanApplyOutcome::CommittedWithCleanupWarnings {
            warnings: vec![
                format!(
                    "organization layout committed, but staging cleanup failed at {}: {error:#}",
                    staging_path.display()
                ),
                format!(
                    "organization backup retained after staging cleanup failure at {}",
                    backup_path.display()
                ),
            ],
            retained_paths,
        });
    }

    let backup_path = backup_dir.path().to_path_buf();
    if let Err(error) = close_committed_backup(backup_dir) {
        let retained_paths = backup_path
            .exists()
            .then_some(backup_path.clone())
            .into_iter()
            .collect();
        return Ok(PlanApplyOutcome::CommittedWithCleanupWarnings {
            warnings: vec![format!(
                "organization layout committed, but backup cleanup failed at {}: {error:#}",
                backup_path.display()
            )],
            retained_paths,
        });
    }

    Ok(PlanApplyOutcome::Committed)
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

    fn assert_original_layout(work: &Path) {
        assert_eq!(fs::read(work.join("alpha.bin")).unwrap(), b"alpha");
        assert_eq!(fs::read(work.join("beta.bin")).unwrap(), b"beta");
        assert_eq!(fs::read(work.join("unplanned.bin")).unwrap(), b"unplanned");
        assert!(!work.join("Alpha").exists());
        assert!(!work.join("Beta").exists());
    }

    fn transactional_fixture(parent: &Path) -> (PathBuf, OrganizationPlan) {
        let work = parent.join("work");
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
        (work, plan)
    }

    fn assert_time_close(actual: std::time::SystemTime, expected: std::time::SystemTime) {
        let difference = actual
            .duration_since(expected)
            .or_else(|_| expected.duration_since(actual))
            .unwrap();
        assert!(
            difference <= std::time::Duration::from_secs(2),
            "timestamp differs by {difference:?}: actual={actual:?}, expected={expected:?}"
        );
    }

    #[cfg(any(windows, target_os = "macos"))]
    fn set_creation_time(path: &Path, created: std::time::SystemTime) {
        #[cfg(target_os = "macos")]
        use std::os::macos::fs::FileTimesExt;
        #[cfg(windows)]
        use std::os::windows::fs::FileTimesExt;

        open_plan_metadata_handle(path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_created(created))
            .unwrap();
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
    fn apply_plan_merges_later_file_into_readonly_source_directory() {
        let temp = tempfile::tempdir().unwrap();
        let work = temp.path().join("work");
        fs::create_dir(&work).unwrap();
        fs::create_dir(work.join("source-dir")).unwrap();
        fs::write(work.join("source-dir/original.bin"), b"original").unwrap();
        fs::write(work.join("later.bin"), b"later").unwrap();
        let mut permissions = fs::metadata(work.join("source-dir")).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(work.join("source-dir"), permissions).unwrap();
        let plan = empty_plan(
            vec![
                ("source-dir".into(), "Out".into()),
                ("later.bin".into(), "Out/later.bin".into()),
            ],
            vec![],
        );

        apply_plan_to_workdir(&plan, &work).unwrap();

        assert_eq!(
            fs::read(work.join("Out/original.bin")).unwrap(),
            b"original"
        );
        assert_eq!(fs::read(work.join("Out/later.bin")).unwrap(), b"later");
        assert!(fs::metadata(work.join("Out"))
            .unwrap()
            .permissions()
            .readonly());
        let mut permissions = fs::metadata(work.join("Out")).unwrap().permissions();
        permissions.set_readonly(false);
        fs::set_permissions(work.join("Out"), permissions).unwrap();
    }

    #[test]
    fn apply_plan_writes_generated_file_under_readonly_source_directory() {
        let temp = tempfile::tempdir().unwrap();
        let work = temp.path().join("work");
        fs::create_dir(&work).unwrap();
        fs::create_dir(work.join("source-dir")).unwrap();
        fs::write(work.join("source-dir/original.bin"), b"original").unwrap();
        let mut permissions = fs::metadata(work.join("source-dir")).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(work.join("source-dir"), permissions).unwrap();
        let plan = empty_plan(
            vec![("source-dir".into(), "Out".into())],
            vec![("Out/metadata.json".into(), "generated".into())],
        );

        apply_plan_to_workdir(&plan, &work).unwrap();

        assert_eq!(
            fs::read(work.join("Out/metadata.json")).unwrap(),
            b"generated"
        );
        assert!(fs::metadata(work.join("Out"))
            .unwrap()
            .permissions()
            .readonly());
        let mut permissions = fs::metadata(work.join("Out")).unwrap().permissions();
        permissions.set_readonly(false);
        fs::set_permissions(work.join("Out"), permissions).unwrap();
    }

    #[test]
    fn apply_plan_preserves_source_file_timestamps() {
        let temp = tempfile::tempdir().unwrap();
        let work = temp.path().join("work");
        fs::create_dir(&work).unwrap();
        let source = work.join("game.bin");
        fs::write(&source, b"game").unwrap();
        let expected_modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(946_684_800);
        let expected_accessed = expected_modified + std::time::Duration::from_secs(60);
        open_plan_metadata_handle(&source)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_accessed(expected_accessed)
                    .set_modified(expected_modified),
            )
            .unwrap();
        let source_metadata = fs::metadata(&source).unwrap();
        let plan = empty_plan(vec![("game.bin".into(), "Out/game.bin".into())], vec![]);

        apply_plan_to_workdir(&plan, &work).unwrap();

        let output_metadata = fs::metadata(work.join("Out/game.bin")).unwrap();
        assert_time_close(
            output_metadata.modified().unwrap(),
            source_metadata.modified().unwrap(),
        );
        assert_time_close(
            output_metadata.accessed().unwrap(),
            source_metadata.accessed().unwrap(),
        );
    }

    #[test]
    fn apply_plan_preserves_source_directory_timestamps_after_population() {
        let temp = tempfile::tempdir().unwrap();
        let work = temp.path().join("work");
        fs::create_dir(&work).unwrap();
        let source = work.join("source-dir");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("original.bin"), b"original").unwrap();
        let expected_modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(978_307_200);
        let expected_accessed = expected_modified + std::time::Duration::from_secs(60);
        open_plan_metadata_handle(&source)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_accessed(expected_accessed)
                    .set_modified(expected_modified),
            )
            .unwrap();
        let source_metadata = fs::metadata(&source).unwrap();
        let plan = empty_plan(
            vec![("source-dir".into(), "Out".into())],
            vec![("Out/generated.txt".into(), "generated".into())],
        );

        apply_plan_to_workdir(&plan, &work).unwrap();

        let output_metadata = fs::metadata(work.join("Out")).unwrap();
        assert_time_close(
            output_metadata.modified().unwrap(),
            source_metadata.modified().unwrap(),
        );
        assert_time_close(
            output_metadata.accessed().unwrap(),
            source_metadata.accessed().unwrap(),
        );
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn apply_plan_preserves_source_file_creation_time() {
        let temp = tempfile::tempdir().unwrap();
        let work = temp.path().join("work");
        fs::create_dir(&work).unwrap();
        let source = work.join("game.bin");
        fs::write(&source, b"game").unwrap();
        let expected_created =
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_104_537_600);
        set_creation_time(&source, expected_created);
        let source_created = fs::metadata(&source).unwrap().created().unwrap();
        let plan = empty_plan(vec![("game.bin".into(), "Out/game.bin".into())], vec![]);

        apply_plan_to_workdir(&plan, &work).unwrap();

        assert_time_close(
            fs::metadata(work.join("Out/game.bin"))
                .unwrap()
                .created()
                .unwrap(),
            source_created,
        );
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn apply_plan_preserves_source_directory_creation_time() {
        let temp = tempfile::tempdir().unwrap();
        let work = temp.path().join("work");
        fs::create_dir(&work).unwrap();
        let source = work.join("source-dir");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("original.bin"), b"original").unwrap();
        let expected_created =
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_135_987_200);
        set_creation_time(&source, expected_created);
        let source_created = fs::metadata(&source).unwrap().created().unwrap();
        let plan = empty_plan(vec![("source-dir".into(), "Out".into())], vec![]);

        apply_plan_to_workdir(&plan, &work).unwrap();

        assert_time_close(
            fs::metadata(work.join("Out")).unwrap().created().unwrap(),
            source_created,
        );
    }

    #[test]
    fn apply_plan_reports_prebackup_error_and_staging_cleanup_failure() {
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
        let retained_path = std::sync::Arc::new(std::sync::Mutex::new(None));
        let retained_path_for_cleanup = retained_path.clone();

        let error = apply_plan_to_workdir_with_hooks(
            &plan,
            &work,
            |_| Ok(()),
            move |directory| {
                let path = directory.keep();
                *retained_path_for_cleanup.lock().unwrap() = Some(path.clone());
                anyhow::bail!("injected staging cleanup failure at {}", path.display());
            },
        )
        .unwrap_err();
        let error = format!("{error:#}");
        let retained_path = retained_path.lock().unwrap().clone().unwrap();

        assert!(error.contains("conflict"), "missing build error: {error}");
        assert!(
            error.contains("injected staging cleanup failure"),
            "missing cleanup error: {error}"
        );
        assert!(
            error.contains(&retained_path.display().to_string()),
            "missing exact staging path: {error}"
        );
        assert!(retained_path.exists());
        assert_eq!(fs::read(work.join("a.bin")).unwrap(), b"a");
        assert_eq!(fs::read(work.join("b.bin")).unwrap(), b"b");
        fs::remove_dir_all(&retained_path).unwrap();
        assert_no_plan_staging_directories(temp.path());
    }

    #[test]
    fn apply_plan_keeps_committed_layout_when_staging_cleanup_fails() {
        let temp = tempfile::tempdir().unwrap();
        let (work, plan) = transactional_fixture(temp.path());
        let retained_staging = std::sync::Arc::new(std::sync::Mutex::new(None));
        let retained_staging_for_cleanup = retained_staging.clone();

        apply_plan_to_workdir_with_cleanup_hooks(
            &plan,
            &work,
            |_| Ok(()),
            |directory| directory.close().map_err(Into::into),
            move |directory| {
                let path = directory.keep();
                *retained_staging_for_cleanup.lock().unwrap() = Some(path.clone());
                anyhow::bail!(
                    "injected committed staging cleanup failure at {}",
                    path.display()
                );
            },
            |_| panic!("backup cleanup must not run after staging cleanup fails"),
        )
        .unwrap();

        assert_eq!(fs::read(work.join("Alpha/output.bin")).unwrap(), b"alpha");
        assert_eq!(fs::read(work.join("Beta/output.bin")).unwrap(), b"beta");
        let retained_staging = retained_staging.lock().unwrap().clone().unwrap();
        let recovery_directories = plan_recovery_directories(temp.path());
        assert_eq!(recovery_directories.len(), 2);
        let retained_staging = retained_staging.canonicalize().unwrap();
        assert!(recovery_directories
            .iter()
            .any(|path| path.canonicalize().unwrap() == retained_staging));
        let backup = recovery_directories
            .iter()
            .find(|path| path.canonicalize().unwrap() != retained_staging)
            .unwrap();
        assert_eq!(fs::read(backup.join("alpha.bin")).unwrap(), b"alpha");
        assert_eq!(fs::read(backup.join("beta.bin")).unwrap(), b"beta");
        assert_eq!(
            fs::read(backup.join("unplanned.bin")).unwrap(),
            b"unplanned"
        );
        for path in recovery_directories {
            fs::remove_dir_all(path).unwrap();
        }
        assert!(work.exists(), "committed work tree was discarded");
        assert_no_plan_staging_directories(temp.path());
    }

    #[test]
    fn apply_plan_keeps_committed_layout_when_backup_cleanup_fails() {
        let temp = tempfile::tempdir().unwrap();
        let (work, plan) = transactional_fixture(temp.path());
        let retained_backup = std::sync::Arc::new(std::sync::Mutex::new(None));
        let retained_backup_for_cleanup = retained_backup.clone();

        apply_plan_to_workdir_with_cleanup_hooks(
            &plan,
            &work,
            |_| Ok(()),
            |directory| directory.close().map_err(Into::into),
            |directory| directory.close().map_err(Into::into),
            move |directory| {
                let path = directory.keep();
                *retained_backup_for_cleanup.lock().unwrap() = Some(path.clone());
                anyhow::bail!(
                    "injected committed backup cleanup failure at {}",
                    path.display()
                );
            },
        )
        .unwrap();

        assert_eq!(fs::read(work.join("Alpha/output.bin")).unwrap(), b"alpha");
        assert_eq!(fs::read(work.join("Beta/output.bin")).unwrap(), b"beta");
        let retained_backup = retained_backup.lock().unwrap().clone().unwrap();
        assert!(retained_backup.exists());
        assert_eq!(
            fs::read(retained_backup.join("alpha.bin")).unwrap(),
            b"alpha"
        );
        assert_eq!(fs::read(retained_backup.join("beta.bin")).unwrap(), b"beta");
        assert_eq!(
            fs::read(retained_backup.join("unplanned.bin")).unwrap(),
            b"unplanned"
        );
        fs::remove_dir_all(retained_backup).unwrap();
        assert!(work.exists(), "committed work tree was discarded");
        assert_no_plan_staging_directories(temp.path());
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
    fn apply_plan_rolls_back_when_final_metadata_application_fails() {
        let temp = tempfile::tempdir().unwrap();
        let (work, plan) = transactional_fixture(temp.path());

        let error = apply_plan_to_workdir_with_swap_hook(&plan, &work, |point| {
            if point == PlanSwapPoint::BeforePromote(1) {
                fs::remove_file(work.join("Alpha/output.bin"))?;
            }
            Ok(())
        })
        .unwrap_err();

        assert!(
            format!("{error:#}").contains("metadata"),
            "unexpected error: {error:#}"
        );
        assert_original_layout(&work);
        assert_no_plan_staging_directories(temp.path());
    }

    #[test]
    fn apply_plan_restores_complete_layout_at_each_forward_swap_failure() {
        let cases = [
            PlanSwapPoint::BeforeBackup(0),
            PlanSwapPoint::BeforeBackup(1),
            PlanSwapPoint::BeforePromote(0),
            PlanSwapPoint::BeforePromote(1),
        ];

        for failure_point in cases {
            let temp = tempfile::tempdir().unwrap();
            let (work, plan) = transactional_fixture(temp.path());

            let error = apply_plan_to_workdir_with_swap_hook(&plan, &work, |point| {
                if point == failure_point {
                    anyhow::bail!("injected forward swap failure at {point:?}");
                }
                Ok(())
            })
            .unwrap_err();

            assert!(
                format!("{error:#}").contains("injected forward swap failure"),
                "unexpected error for {failure_point:?}: {error:#}"
            );
            assert_original_layout(&work);
            assert_no_plan_staging_directories(temp.path());
        }
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
        assert_eq!(fs::read(work.join("Alpha/output.bin")).unwrap(), b"alpha");
        let recovery_directories = plan_recovery_directories(temp.path());
        let staging = recovery_directories
            .iter()
            .find(|path| {
                !path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .contains("backup")
            })
            .unwrap();
        let backup = recovery_directories
            .iter()
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .contains("backup")
            })
            .unwrap();
        assert_eq!(fs::read(staging.join("Beta/output.bin")).unwrap(), b"beta");
        assert!(fs::read_dir(backup).unwrap().next().is_none());
        for path in recovery_directories {
            fs::remove_dir_all(path).unwrap();
        }
        assert_no_plan_staging_directories(temp.path());
    }

    #[test]
    fn apply_plan_retains_exact_recovery_contents_when_backup_restore_fails() {
        let temp = tempfile::tempdir().unwrap();
        let (work, plan) = transactional_fixture(temp.path());

        let error = apply_plan_to_workdir_with_swap_hook(&plan, &work, |point| {
            if matches!(
                point,
                PlanSwapPoint::BeforePromote(0) | PlanSwapPoint::BeforeRestoreBackup(0)
            ) {
                anyhow::bail!("injected backup restore failure");
            }
            Ok(())
        })
        .unwrap_err();
        let error = format!("{error:#}");
        let recovery_directories = plan_recovery_directories(temp.path());
        let staging = recovery_directories
            .iter()
            .find(|path| {
                !path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .contains("backup")
            })
            .unwrap();
        let backup = recovery_directories
            .iter()
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .contains("backup")
            })
            .unwrap();

        assert!(error.contains(&staging.display().to_string()));
        assert!(error.contains(&backup.display().to_string()));
        assert_eq!(fs::read(work.join("alpha.bin")).unwrap(), b"alpha");
        assert_eq!(fs::read(work.join("beta.bin")).unwrap(), b"beta");
        assert!(!work.join("unplanned.bin").exists());
        assert_eq!(
            fs::read(backup.join("unplanned.bin")).unwrap(),
            b"unplanned"
        );
        assert_eq!(
            fs::read(staging.join("Alpha/output.bin")).unwrap(),
            b"alpha"
        );
        assert_eq!(fs::read(staging.join("Beta/output.bin")).unwrap(), b"beta");
        for path in recovery_directories {
            fs::remove_dir_all(path).unwrap();
        }
        assert_no_plan_staging_directories(temp.path());
    }

    #[test]
    fn apply_plan_retains_original_backup_when_restore_destination_collides() {
        let temp = tempfile::tempdir().unwrap();
        let (work, plan) = transactional_fixture(temp.path());

        let error = apply_plan_to_workdir_with_swap_hook(&plan, &work, |point| {
            if point == PlanSwapPoint::BeforePromote(0) {
                anyhow::bail!("injected promotion failure");
            }
            if point == PlanSwapPoint::BeforeRestoreBackup(0) {
                fs::write(work.join("unplanned.bin"), b"collision")?;
            }
            Ok(())
        })
        .unwrap_err();
        let error = format!("{error:#}");
        let recovery_directories = plan_recovery_directories(temp.path());
        let backup = recovery_directories
            .iter()
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .contains("backup")
            })
            .unwrap();

        assert!(error.contains("refusing to replace existing"));
        assert_eq!(fs::read(work.join("alpha.bin")).unwrap(), b"alpha");
        assert_eq!(fs::read(work.join("beta.bin")).unwrap(), b"beta");
        assert_eq!(fs::read(work.join("unplanned.bin")).unwrap(), b"collision");
        assert_eq!(
            fs::read(backup.join("unplanned.bin")).unwrap(),
            b"unplanned"
        );
        for path in recovery_directories {
            fs::remove_dir_all(path).unwrap();
        }
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
