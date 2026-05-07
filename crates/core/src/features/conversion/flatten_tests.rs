//! Tests for `flatten.rs`. Lifted out of the inline `#[cfg(test)]
//! mod tests { ... }` block — same names, same coverage.
//!
//! Loaded as `flatten::tests` via `#[path]` from `flatten.rs`, so
//! `super::*` here is everything in `flatten.rs`.

use super::*;

#[test]
fn test_is_archive_filename() {
    assert!(is_archive_filename("mod.rar"));
    assert!(is_archive_filename("mod.RAR"));
    assert!(is_archive_filename("mod.zip"));
    assert!(is_archive_filename("mod.7z"));
    assert!(is_archive_filename("Something - Patch Main.rar"));
    assert!(!is_archive_filename("mod.pak"));
    assert!(!is_archive_filename("readme.txt"));
    assert!(!is_archive_filename("no_extension"));
}

#[test]
fn test_strip_archive_extension() {
    assert_eq!(strip_archive_extension("mod.rar"), "mod");
    assert_eq!(strip_archive_extension("Patch.Main.zip"), "Patch.Main");
    assert_eq!(strip_archive_extension("mod.RAR"), "mod");
    assert_eq!(strip_archive_extension("not_archive.pak"), "not_archive.pak");
}

#[test]
fn test_target_folder_names_no_prefix_strip() {
    let paths = vec![
        PathBuf::from("/tmp/AG - Silver - Main.rar"),
        PathBuf::from("/tmp/AG - Silver - Patch A.rar"),
    ];
    let result = target_folder_names(&paths, false);
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].1, "AG - Silver - Main");
    assert_eq!(result[1].1, "AG - Silver - Patch A");
}

#[test]
fn test_target_folder_names_with_prefix_strip() {
    let paths = vec![
        PathBuf::from("/tmp/AG - LK - Silver Linning Lingerie - Main.rar"),
        PathBuf::from("/tmp/AG - LK - Silver Linning Lingerie - Patch Makeup.rar"),
        PathBuf::from("/tmp/AG - LK - Silver Linning Lingerie - Patch No Clothes.rar"),
    ];
    let result = target_folder_names(&paths, true);
    assert_eq!(result.len(), 3);
    assert_eq!(result[0].1, "Main");
    assert_eq!(result[1].1, "Patch Makeup");
    assert_eq!(result[2].1, "Patch No Clothes");
}

#[test]
fn test_target_folder_names_prefix_would_empty_name() {
    let paths = vec![
        PathBuf::from("/tmp/MyModName.rar"),
        PathBuf::from("/tmp/MyModName Extra.rar"),
    ];
    let result = target_folder_names(&paths, true);
    assert_eq!(result[0].1, "MyModName"); // kept original (would be empty)
    assert_eq!(result[1].1, " Extra");
}

#[test]
fn test_find_archive_files_empty_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let result = find_archive_files(tmp.path()).unwrap();
    assert!(result.is_empty());
}

#[test]
fn test_find_archive_files_mixed() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("mod.rar"), b"").unwrap();
    std::fs::write(tmp.path().join("data.zip"), b"").unwrap();
    std::fs::write(tmp.path().join("readme.txt"), b"").unwrap();
    std::fs::write(tmp.path().join("game.pak"), b"").unwrap();

    let result = find_archive_files(tmp.path()).unwrap();
    assert_eq!(result.len(), 2);
}

#[test]
fn test_flatten_extracts_and_removes() {
    let tmp = tempfile::tempdir().unwrap();
    let inner_rar = tmp.path().join("inner.rar");
    std::fs::write(&inner_rar, b"fake archive").unwrap();

    let report = flatten_nested_archives(tmp.path(), false, |_archive, dest| {
        std::fs::write(dest.join("extracted.txt"), b"ok")?;
        Ok(())
    })
    .unwrap();

    assert_eq!(report.extracted.len(), 1);
    assert_eq!(report.extracted[0], "inner");
    assert!(tmp.path().join("inner").join("extracted.txt").exists());
    assert!(!inner_rar.exists());
}

#[test]
fn test_flatten_handles_extraction_failure() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("bad.rar"), b"").unwrap();

    let report = flatten_nested_archives(tmp.path(), false, |_, _| {
        Err(anyhow::anyhow!("extraction failed"))
    })
    .unwrap();

    assert_eq!(report.failed.len(), 1);
    assert_eq!(report.failed[0].0, "bad");
    assert!(!tmp.path().join("bad").exists());
    assert!(tmp.path().join("bad.rar").exists());
}

#[test]
fn test_flatten_with_prefix_strip() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("MyPack - Main.rar"), b"").unwrap();
    std::fs::write(tmp.path().join("MyPack - Variant A.rar"), b"").unwrap();

    let report = flatten_nested_archives(tmp.path(), true, |_archive, dest| {
        std::fs::write(dest.join("marker"), b"")?;
        Ok(())
    })
    .unwrap();

    assert_eq!(report.extracted.len(), 2);
    assert!(tmp.path().join("Main").exists());
    assert!(tmp.path().join("Variant A").exists());
}

#[test]
fn test_flatten_unwraps_single_root_folder() {
    // Archive contains its own root folder matching the real mod name —
    // the wrapper from strip_common_prefix should be promoted away so
    // mod managers see the mod folder at the top.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("Pack - Main.rar"), b"").unwrap();

    let report = flatten_nested_archives(tmp.path(), true, |_archive, dest| {
        // Simulate an archive that expands to `dest/ModName/...`
        let inner = dest.join("ModName");
        std::fs::create_dir(&inner)?;
        std::fs::write(inner.join("mod.dll"), b"")?;
        Ok(())
    })
    .unwrap();

    assert_eq!(report.extracted, vec!["ModName".to_string()]);
    // "Main" wrapper gone, "ModName" promoted next to the (now-removed) archive
    assert!(tmp.path().join("ModName/mod.dll").exists());
    assert!(!tmp.path().join("Main").exists());
}

#[test]
fn test_flatten_keeps_wrapper_when_multiple_roots() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("pack.rar"), b"").unwrap();

    let report = flatten_nested_archives(tmp.path(), false, |_archive, dest| {
        std::fs::create_dir(dest.join("folder_a"))?;
        std::fs::write(dest.join("loose.txt"), b"")?;
        Ok(())
    })
    .unwrap();

    // Multiple entries — wrapper stays
    assert_eq!(report.extracted, vec!["pack".to_string()]);
    assert!(tmp.path().join("pack/folder_a").exists());
    assert!(tmp.path().join("pack/loose.txt").exists());
}

#[test]
fn test_flatten_keeps_wrapper_when_single_file_at_root() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("pack.rar"), b"").unwrap();

    let report = flatten_nested_archives(tmp.path(), false, |_archive, dest| {
        std::fs::write(dest.join("only_file.txt"), b"")?;
        Ok(())
    })
    .unwrap();

    // Single entry but it's a file, not a folder — wrapper stays
    assert_eq!(report.extracted, vec!["pack".to_string()]);
    assert!(tmp.path().join("pack/only_file.txt").exists());
}

#[test]
fn test_flatten_unwrap_when_inner_name_matches_wrapper() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("Main.rar"), b"").unwrap();

    let report = flatten_nested_archives(tmp.path(), false, |_archive, dest| {
        let inner = dest.join("Main");
        std::fs::create_dir(&inner)?;
        std::fs::write(inner.join("a.txt"), b"")?;
        Ok(())
    })
    .unwrap();

    // Wrapper and inner happen to share the name "Main" — unwrap still succeeds
    assert_eq!(report.extracted, vec!["Main".to_string()]);
    assert!(tmp.path().join("Main/a.txt").exists());
    // No leftover temp files or double-nesting
    assert!(!tmp.path().join("Main/Main").exists());
}

#[test]
fn test_flatten_finds_archives_in_subfolders() {
    // Regression: outer archive extracts to a subfolder layout,
    // inner archives must still be found and flattened next to them.
    let tmp = tempfile::tempdir().unwrap();
    let sub = tmp.path().join("PackRoot");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(sub.join("Main.rar"), b"").unwrap();
    std::fs::write(sub.join("Patch.rar"), b"").unwrap();

    let report = flatten_nested_archives(tmp.path(), false, |_archive, dest| {
        std::fs::write(dest.join("marker"), b"")?;
        Ok(())
    })
    .unwrap();

    assert_eq!(report.extracted.len(), 2);
    // Output folders should sit next to their source archive, not at root
    assert!(sub.join("Main/marker").exists());
    assert!(sub.join("Patch/marker").exists());
    // Originals removed
    assert!(!sub.join("Main.rar").exists());
    assert!(!sub.join("Patch.rar").exists());
}

// ---- Recursive flatten tests (Phase 1 of pipeline-collision plan) ----

/// Helper: "extracts" by writing a dest marker and optionally dropping
/// additional archive files at known paths. Tracks how many times it ran.
fn make_counting_extractor(
    dir_layouts: std::collections::HashMap<String, Vec<String>>,
) -> (
    impl FnMut(&Path, &Path) -> Result<()>,
    std::rc::Rc<std::cell::Cell<u32>>,
) {
    let count = std::rc::Rc::new(std::cell::Cell::new(0u32));
    let count_clone = count.clone();
    let extractor = move |archive_path: &Path, dest: &Path| -> Result<()> {
        count_clone.set(count_clone.get() + 1);
        let name = archive_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        // Always drop a marker so the unwrap logic sees content
        std::fs::write(dest.join(".flattened"), b"")?;
        if let Some(next_files) = dir_layouts.get(&name) {
            for rel in next_files {
                let p = dest.join(rel);
                if let Some(parent) = p.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&p, b"")?;
            }
        }
        Ok(())
    };
    (extractor, count)
}

#[test]
fn test_recursive_flatten_three_levels() {
    // Layout: outer.rar extracts producing inner.zip which extracts
    // producing innermost.7z which extracts producing loose files.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("outer.rar"), b"").unwrap();

    let mut layouts = std::collections::HashMap::new();
    // outer.rar extraction deposits an inner.zip sibling
    layouts.insert("outer.rar".to_string(), vec!["inner.zip".to_string()]);
    // inner.zip extraction deposits innermost.7z sibling
    layouts.insert("inner.zip".to_string(), vec!["innermost.7z".to_string()]);
    // innermost.7z extraction deposits loose payload
    layouts.insert("innermost.7z".to_string(), vec!["payload.txt".to_string()]);

    let (extractor, count) = make_counting_extractor(layouts);
    let report = flatten_nested_archives_recursive(tmp.path(), false, 0, extractor).unwrap();

    // Three archives should have been processed
    assert_eq!(count.get(), 3);
    assert_eq!(report.extracted.len(), 3);
    // Original outer.rar is gone
    assert!(!tmp.path().join("outer.rar").exists());
    // Payload is somewhere in the tree (exact path depends on unwrapping, just probe recursively)
    let mut found_payload = false;
    for entry in walkdir::WalkDir::new(tmp.path()) {
        let e = entry.unwrap();
        if e.file_name() == "payload.txt" {
            found_payload = true;
            break;
        }
    }
    assert!(found_payload, "payload.txt should have been extracted somewhere");
}

#[test]
fn test_max_depth_1_matches_single_pass() {
    // With max_depth=1, an outer archive that produces an inner archive
    // should leave the inner one unflattened (same as calling flatten once).
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("outer.rar"), b"").unwrap();

    let mut layouts = std::collections::HashMap::new();
    layouts.insert("outer.rar".to_string(), vec!["inner.zip".to_string()]);

    let (extractor, count) = make_counting_extractor(layouts);
    let _report = flatten_nested_archives_recursive(tmp.path(), false, 1, extractor).unwrap();

    // Only the outer archive should have been extracted
    assert_eq!(count.get(), 1);
    // The inner.zip should still exist somewhere in the tree
    let mut found_inner = false;
    for entry in walkdir::WalkDir::new(tmp.path()) {
        let e = entry.unwrap();
        if e.file_name() == "inner.zip" {
            found_inner = true;
            break;
        }
    }
    assert!(found_inner, "inner.zip should remain after a single-pass flatten");
}

#[test]
fn test_recursive_flatten_exits_early_when_stable() {
    // Layout produces no new archives on the first pass.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("plain.rar"), b"").unwrap();

    let layouts = std::collections::HashMap::new(); // no nested archives
    let (extractor, count) = make_counting_extractor(layouts);

    // max_depth=0 (unlimited) must still exit after one pass when nothing new appears
    let report = flatten_nested_archives_recursive(tmp.path(), false, 0, extractor).unwrap();

    assert_eq!(count.get(), 1, "should only run once when nothing nested");
    assert_eq!(report.extracted.len(), 1);
}

#[test]
fn test_recursive_flatten_respects_depth_cap() {
    // Self-replicating layout: every extraction produces another archive
    // with the same name pattern. max_depth=3 should cap at 3 extractions.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("a.rar"), b"").unwrap();

    let mut layouts = std::collections::HashMap::new();
    layouts.insert("a.rar".to_string(), vec!["b.rar".to_string()]);
    layouts.insert("b.rar".to_string(), vec!["c.rar".to_string()]);
    layouts.insert("c.rar".to_string(), vec!["d.rar".to_string()]);
    layouts.insert("d.rar".to_string(), vec!["e.rar".to_string()]);
    layouts.insert("e.rar".to_string(), vec!["f.rar".to_string()]);

    let (extractor, count) = make_counting_extractor(layouts);
    let report = flatten_nested_archives_recursive(tmp.path(), false, 3, extractor).unwrap();

    // Exactly 3 iterations → 3 extractions
    assert_eq!(count.get(), 3);
    assert_eq!(report.extracted.len(), 3);
}

#[test]
fn test_recursive_flatten_hard_safety_cap_iterations() {
    // Force unlimited mode (max_depth=0) against a layout that keeps
    // producing archives. The FLATTEN_MAX_ITERATIONS cap must stop it.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("level0.rar"), b"").unwrap();

    let mut layouts = std::collections::HashMap::new();
    for i in 0..(FLATTEN_MAX_ITERATIONS + 5) {
        layouts.insert(
            format!("level{}.rar", i),
            vec![format!("level{}.rar", i + 1)],
        );
    }

    let (extractor, count) = make_counting_extractor(layouts);
    let _ = flatten_nested_archives_recursive(tmp.path(), false, 0, extractor).unwrap();

    // Must not exceed the hard cap even in unlimited mode
    assert!(
        count.get() <= FLATTEN_MAX_ITERATIONS,
        "extraction count {} exceeded hard cap {}",
        count.get(),
        FLATTEN_MAX_ITERATIONS
    );
}
