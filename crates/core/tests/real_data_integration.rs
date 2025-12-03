use arclain_core::archive_organizer::{organize_archive, GameMetadata};
use arclain_core::sevenzip::SevenZipCli;
use arclain_core::ArchiveBackend;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use walkdir::WalkDir;

// Helper to copy directory recursively
fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> std::io::Result<()> {
    fs::create_dir_all(&dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(entry.path(), dst.as_ref().join(entry.file_name()))?;
        } else {
            fs::copy(entry.path(), dst.as_ref().join(entry.file_name()))?;
        }
    }
    Ok(())
}

// Helper to compare directories
fn assert_dirs_equal(dir1: &Path, dir2: &Path) {
    let walker1 = WalkDir::new(dir1).sort_by_file_name();
    let walker2 = WalkDir::new(dir2).sort_by_file_name();

    let mut iter1 = walker1.into_iter();
    let mut iter2 = walker2.into_iter();

    loop {
        let entry1 = iter1.next();
        let entry2 = iter2.next();

        match (entry1, entry2) {
            (Some(Ok(e1)), Some(Ok(e2))) => {
                // Compare relative paths
                let rel1 = e1.path().strip_prefix(dir1).unwrap();
                let rel2 = e2.path().strip_prefix(dir2).unwrap();
                assert_eq!(rel1, rel2, "File structure mismatch");

                // Compare file types
                assert_eq!(
                    e1.file_type().is_dir(),
                    e2.file_type().is_dir(),
                    "File type mismatch at {:?}",
                    rel1
                );

                // Compare file sizes (if file)
                if e1.file_type().is_file() {
                    assert_eq!(
                        e1.metadata().unwrap().len(),
                        e2.metadata().unwrap().len(),
                        "File size mismatch at {:?}",
                        rel1
                    );
                }
            }
            (None, None) => break,
            (Some(_), None) => panic!("Dir1 has more files than Dir2"),
            (None, Some(_)) => panic!("Dir2 has more files than Dir1"),
            (Some(Err(e)), _) => panic!("Error reading dir1: {}", e),
            (_, Some(Err(e))) => panic!("Error reading dir2: {}", e),
        }
    }
}

fn get_dummy_metadata(product_id: &str) -> GameMetadata {
    GameMetadata {
        product_id: product_id.to_string(),
        source: "dlsite".to_string(),
        title: "Test Game".to_string(),
        description: None,
        tags: vec![],
        creator: None,
        release_date: None,
        screenshots: vec![],
        metadata_json: "{}".to_string(),
    }
}

#[test]
fn test_real_data_idempotency() {
    // 1. Setup
    let temp = TempDir::new().expect("Failed to create temp dir");
    let expected_src = Path::new("d:/Programming/rust/arclain/_real_data/expected_result");

    if !expected_src.exists() {
        println!("Skipping test: Real data not found at {:?}", expected_src);
        return;
    }

    let input_dir = temp.path().join("input_folder");
    copy_dir_all(expected_src, &input_dir).expect("Failed to copy expected result");

    // Create a zip from the input folder to simulate an archive
    // We use 7z to create it
    let backend = SevenZipCli::detect(None).expect("Failed to init 7z");
    let archive_path = temp.path().join("input.7z");

    // We need to zip the CONTENTS of input_dir, not input_dir itself
    // So we pass the files inside input_dir
    let files: Vec<PathBuf> = fs::read_dir(&input_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();

    backend
        .create_archive(&archive_path, &files, "7z")
        .expect("Failed to create temp archive");

    // 2. Run Organizer
    let output_dir = temp.path().join("output");
    let metadata = get_dummy_metadata("RJ_TEST");

    organize_archive(&backend, &archive_path, &output_dir, &metadata, temp.path())
        .expect("Failed to organize archive");

    // 3. Verify
    // The organizer creates output_dir/RJ_TEST/Game/...
    // We expect the content of output_dir/RJ_TEST/Game to match input_dir
    let organized_game_dir = output_dir.join("RJ_TEST").join("Game");

    assert_dirs_equal(&input_dir, &organized_game_dir);
}

#[test]
fn test_real_data_integration() {
    // 1. Setup
    let temp = TempDir::new().expect("Failed to create temp dir");
    let integration_src = Path::new("d:/Programming/rust/arclain/_real_data/integration_data");
    let expected_src = Path::new("d:/Programming/rust/arclain/_real_data/expected_result");

    if !integration_src.exists() || !expected_src.exists() {
        println!("Skipping test: Real data not found");
        return;
    }

    // Find the RAR file
    let archive_path = fs::read_dir(integration_src)
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| e.path().extension().map_or(false, |ext| ext == "rar"))
        .map(|e| e.path())
        .expect("No RAR file found in integration_data");

    let backend = SevenZipCli::detect(None).expect("Failed to init 7z");

    // 2. Run Organizer
    let output_dir = temp.path().join("output");
    let metadata = get_dummy_metadata("RJ999001"); // Use ID from filename

    let result = organize_archive(&backend, &archive_path, &output_dir, &metadata, temp.path());

    // Handle encryption - skip test if archive is encrypted
    if let Err(e) = result {
        if e.to_string().contains("encrypted") {
            println!("Skipping test: Archive is password-protected");
            return;
        }
        panic!("Failed to organize archive: {}", e);
    }

    // 3. Verify
    let organized_game_dir = output_dir.join("RJ999001").join("Game");

    assert_dirs_equal(expected_src, &organized_game_dir);
}
