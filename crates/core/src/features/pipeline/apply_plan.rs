//! Apply an `OrganizationPlan`'s moves to an already-extracted directory.
//!
//! The pipeline executor has already extracted the source archive to a
//! work dir. This module takes a plan (from `RuleEngine::create_plan`)
//! and reorganizes files within the work dir according to `plan.moves`.

use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

use crate::features::organization::engine::OrganizationPlan;

/// Apply the plan's moves + generated_files to `work_dir` in place.
/// Does NOT handle `plan.downloads` (pipeline executor doesn't run HTTP
/// fetches; downloads are expected to be prefetched or skipped).
pub fn apply_plan_to_workdir(plan: &OrganizationPlan, work_dir: &Path) -> Result<()> {
    // Stage 1: stage all moves into a temp subdir to avoid clobbering
    //          when src and dest overlap
    let staging = work_dir.join("__pipeline_staging__");
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    fs::create_dir_all(&staging)?;

    for (src_rel, dest_rel) in &plan.moves {
        let src = work_dir.join(src_rel);
        let dest = staging.join(dest_rel);
        if !src.exists() {
            tracing::warn!("[apply_plan] source missing: {}", src.display());
            continue;
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).with_context(|| format!("mkdir {:?}", parent))?;
        }
        // Use rename if possible (same filesystem), else copy+remove
        if fs::rename(&src, &dest).is_err() {
            fs::copy(&src, &dest)
                .with_context(|| format!("copy {:?} -> {:?}", src, dest))?;
            fs::remove_file(&src).ok();
        }
    }

    // Stage 2: remove everything outside the staging dir (old layout)
    for entry in fs::read_dir(work_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path == staging {
            continue;
        }
        if path.is_dir() {
            fs::remove_dir_all(&path).ok();
        } else {
            fs::remove_file(&path).ok();
        }
    }

    // Move staging contents up to work_dir root
    for entry in fs::read_dir(&staging)? {
        let entry = entry?;
        let from = entry.path();
        let to = work_dir.join(entry.file_name());
        fs::rename(&from, &to).with_context(|| format!("move {:?} -> {:?}", from, to))?;
    }
    fs::remove_dir_all(&staging).ok();

    // Stage 3: write generated files (metadata.json etc.)
    for (path_rel, content) in &plan.generated_files {
        let path = work_dir.join(path_rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, content).with_context(|| format!("write {:?}", path))?;
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

    fn empty_plan(moves: Vec<(String, String)>, generated: Vec<(String, String)>) -> OrganizationPlan {
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
    fn apply_plan_writes_generated_files() {
        let tmp = tempfile::tempdir().unwrap();
        touch(&tmp.path().join("game.exe"));

        let plan = empty_plan(
            vec![("game.exe".into(), "MyGame/game.exe".into())],
            vec![(
                "MyGame/metadata.json".into(),
                r#"{"title":"Test"}"#.into(),
            )],
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
}
