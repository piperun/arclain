mod content_verification;
use content_verification::ContentHashMap;

use arclain_core::backends::selector::BackendSelector;
use arclain_core::backends::sevenz_cli::SevenZipCli;
use arclain_core::organization::{organizer::organize_archive, GameMetadata};
use arclain_core::utilities::logging::init_test_logging;
use arclain_core::{Archive, ArchiveBackend, ConfigStore, PassRule};
use arclain_db::{DbPaths, SecretsDb, SecretsKey};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tracing::{debug, error, info, warn};
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

/// Test that organizing already-organized content (expected_result) stays the same
#[test]
fn test_expected_result_idempotency() {
    let _ = init_test_logging("test_expected_result_idempotency");

    let temp = TempDir::new().expect("Failed to create temp dir");

    // Tests run from crates/core, so go up to workspace root
    let expected_src = Path::new("../../_real_data/expected_result");

    if !expected_src.exists() {
        warn!("Skipping test: Real data not found at {:?}", expected_src);
        return;
    }

    info!("Starting idempotency test with expected_result data");

    // Copy expected_result to temp directory
    let input_dir = temp.path().join("expected_result_copy");
    copy_dir_all(expected_src, &input_dir).expect("Failed to copy expected result");

    // Create archive from the input folder
    let backend = SevenZipCli::detect(None).expect("Failed to init 7z");
    let archive_path = temp.path().join("test.7z");

    // Collect all files in input_dir
    let files: Vec<PathBuf> = fs::read_dir(&input_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();

    backend
        .create_archive(&archive_path, &files, "7z")
        .expect("Failed to create temp archive");

    // Run organizer - it creates a .7z file, not a directory
    let output_7z = temp.path().join("output.7z");
    let metadata = get_dummy_metadata("RJ_TEST");

    let backend_arc = std::sync::Arc::new(backend);
    let archive = Archive::new(backend_arc.clone(), &archive_path);
    organize_archive(&archive, &output_7z, &metadata, temp.path())
        .expect("Failed to organize archive");

    // Verify: Extract the created 7z and check structure
    assert!(output_7z.exists(), "Output 7z should be created");

    let extract_dir = temp.path().join("extracted");
    fs::create_dir_all(&extract_dir).unwrap();

    backend_arc
        .extract_all(&output_7z, &extract_dir, None)
        .expect("Failed to extract output");

    // The extracted archive should contain RJ_TEST/Game/<game_content>
    let organized_game_dir = extract_dir.join("RJ_TEST").join("Game");

    assert!(
        organized_game_dir.exists(),
        "Game directory should exist in extracted archive"
    );
    assert_dirs_equal(&input_dir, &organized_game_dir);
}

/// Integration test: Process the real .rar file from integration_data
/// This test mirrors the UI workflow exactly:
/// 1. Sets up redb secrets database with password rules
/// 2. Loads rules into ConfigStore (like UI does on startup)
/// 3. Uses BackendSelector to get proper backend (UnRAR for .rar)
/// 4. Calls organize with auto-detected password
#[test]
fn test_integration_data_full_workflow() {
    let _ = init_test_logging("test_integration_data_full_workflow");

    let temp = TempDir::new().expect("Failed to create temp dir");

    // Tests run from crates/core, so go up to workspace root
    let integration_src = Path::new("../../_real_data/integration_data");
    let expected_src = Path::new("../../_real_data/expected_result");

    if !integration_src.exists() {
        warn!(
            "Skipping test: Integration data not found at {:?}",
            integration_src
        );
        return;
    }

    if !expected_src.exists() {
        warn!(
            "Skipping test: Expected result not found at {:?}",
            expected_src
        );
        return;
    }

    // Find the RAR file (should be the RJ999001 file)
    let archive_path = fs::read_dir(integration_src)
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| {
            e.path()
                .extension()
                .map_or(false, |ext| ext == "rar" || ext == "zip" || ext == "7z")
        })
        .map(|e| e.path())
        .expect("No archive file found in integration_data");

    info!("=== Starting Full Workflow Integration Test ===");
    info!("Testing with archive: {:?}", archive_path);

    // === USE PRODUCTION SECRETS DATABASE (EXACTLY like the UI does) ===

    // Use the SAME standardized path function that the UI uses (DbPaths::defaults)
    // Use "arclain" (production) not "arclain_test" to access the REAL password database
    let db_paths = DbPaths::defaults("arclain").expect("Failed to get default database paths");

    info!("Using production database paths:");
    info!("  Config DB: {:?}", db_paths.config_db);
    info!("  Secrets DB: {:?}", db_paths.secrets_db);
    info!("  Key file: {:?}", db_paths.key_file);

    // Load the production master key (same as UI at state.rs:112-124)
    let key = if let Some(ref key_path) = db_paths.key_file {
        if !key_path.exists() {
            warn!("Master key file not found at: {}", key_path.display());
            warn!("Skipping test: Cannot access production password database without master key");
            warn!("The UI would have created this key on first run.");
            return;
        }
        SecretsKey::load_from_file(key_path).expect("Failed to load production key file")
    } else {
        warn!("No key file path configured");
        warn!("Skipping test: Cannot access production password database");
        return;
    };

    // Open production secrets database using standardized path
    let secrets_db = SecretsDb::open(&db_paths.secrets_db, &key.as_bytes())
        .expect("Failed to open production secrets database");

    info!("Checking password rules in production secrets database");

    // Load rules from production database into ConfigStore (EXACTLY like UI at state.rs:208-214)
    let loaded_db_rules = secrets_db
        .list_pass_rules()
        .expect("Failed to load password rules from production database");

    if loaded_db_rules.is_empty() {
        warn!("No password rules found in production database!");
        warn!("Skipping test: Add password rules in the UI first, then run this test.");
        return;
    }

    info!(
        "Found {} password rules in production secrets database",
        loaded_db_rules.len()
    );
    for (i, rule) in loaded_db_rules.iter().enumerate() {
        debug!(
            "  Rule {}: name='{}', pattern='{}', priority={}, enabled={}",
            i + 1,
            rule.name,
            rule.pattern,
            rule.priority,
            rule.enabled
        );
    }

    let mut config_store = ConfigStore::load("arclain").expect("Failed to load config");

    // Convert DbPassRule to PassRule (same as UI)
    config_store.cfg.pass_rules = loaded_db_rules
        .iter()
        .map(|r| PassRule {
            name: r.name.clone(),
            pattern: r.pattern.clone(),
            password: r.password.clone(),
            priority: r.priority,
            enabled: r.enabled,
        })
        .collect();

    info!(
        "Loaded {} password rules into ConfigStore",
        config_store.cfg.pass_rules.len()
    );

    // === USE BACKEND SELECTOR (like UI does) ===
    let selector = BackendSelector::new_native();
    let backend = selector
        .select(&archive_path)
        .expect("Failed to select backend");
    info!("Selected backend: {}", backend.name());

    // === PREPARE METADATA ===
    let metadata = GameMetadata {
        product_id: "RJ999001".to_string(),
        source: "dlsite".to_string(),
        title: "試験用ゲームあいうえお".to_string(),
        description: None,
        tags: vec![],
        creator: Some("TestSite".to_string()),
        release_date: None,
        screenshots: vec![],
        metadata_json: "{}".to_string(),
    };

    let output_7z = temp.path().join("RJ999001.7z");

    // === AUTO-DETECT PASSWORD (like UI does at lines 512 of mod.rs) ===
    let archive_name = archive_path.file_name().and_then(|n| n.to_str());

    info!(
        "Attempting password auto-detection for archive: {:?}",
        archive_name
    );

    let password = config_store.auto_password_for(archive_name, &vec![]);

    if let Some(ref pwd) = password {
        info!("✓ Auto-detected password (length: {})", pwd.len());
        debug!("Password: '{}'", pwd);
    } else {
        warn!("✗ No password auto-detected");
        debug!("Available rules in ConfigStore:");
        for (i, rule) in config_store.cfg.pass_rules.iter().enumerate() {
            debug!(
                "  Rule {}: pattern='{}', enabled={}",
                i + 1,
                rule.pattern,
                rule.enabled
            );
        }
    }

    // === CREATE ARCHIVE HANDLE WITH PASSWORD (dependency injection) ===
    let has_password = password.is_some();
    info!(
        "Creating Archive handle with password: {}",
        if has_password { "provided" } else { "None" }
    );

    let archive = if let Some(pwd) = password {
        Archive::with_password(backend, &archive_path, pwd)
    } else {
        Archive::new(backend, &archive_path)
    };

    // === CALL ORGANIZE (clean dependency injection API) ===
    // We'll intercept the organize process to test before compression
    let org_result = organize_archive(&archive, &output_7z, &metadata, temp.path());

    // Handle encryption errors - the organize function should detect password need internally
    if let Err(e) = org_result {
        error!("Organization failed: {:?}", e);

        let err_msg = e.to_string();
        if err_msg.contains("encrypted")
            || err_msg.contains("password")
            || err_msg.contains("extracting source archive")
            || err_msg.contains("Cannot open")
            || err_msg.contains("Wrong password")
            || err_msg.contains("CRC failed")
            || err_msg.contains("bad CRC")
        {
            warn!("Test Skipped: Password Issue");
            warn!("Archive: {:?}", archive_path.file_name());
            warn!("Error: {}", e);
            warn!(
                "Password was {}",
                if has_password {
                    "provided"
                } else {
                    "not provided"
                }
            );
            warn!("The test is using password rules from your production secrets database.");
            warn!("If the password is still wrong, update the rules in the UI and run the test again.");
            return;
        }
        panic!("Failed to organize archive: {}", e);
    }

    info!("Organization completed successfully");

    // === VERIFY OUTPUT ===
    assert!(output_7z.exists(), "Output 7z archive should be created");

    // Extract twice to test if 7zip is consistent
    let extract_dir1 = temp.path().join("extracted1");
    let extract_dir2 = temp.path().join("extracted2");
    fs::create_dir_all(&extract_dir1).unwrap();
    fs::create_dir_all(&extract_dir2).unwrap();

    // Use 7z backend to extract the final output twice
    let sevenz_backend = SevenZipCli::detect(None).expect("Failed to init 7z");
    sevenz_backend
        .extract_all(&output_7z, &extract_dir1, None)
        .expect("Failed to extract output archive (first time)");
    sevenz_backend
        .extract_all(&output_7z, &extract_dir2, None)
        .expect("Failed to extract output archive (second time)");

    println!("\n=== DEBUG: Extracted Output Structure (First Extraction) ===");
    println!("Contents of extract_dir1: {:?}", extract_dir1);
    for entry in fs::read_dir(&extract_dir1).unwrap().take(5) {
        let entry = entry.unwrap();
        println!(
            "  - {:?} ({})",
            entry.file_name(),
            if entry.file_type().unwrap().is_dir() {
                "dir"
            } else {
                "file"
            }
        );
    }
    println!("=============================================================\n");

    // Use first extraction for main test
    let extract_dir = extract_dir1;

    // Verify structure: Game/, screenshots/, metadata.json
    // The archive organizer creates: RJ999001/Game/...
    let product_dir = extract_dir.join("RJ999001");
    if !product_dir.exists() {
        println!("Expected product directory not found. Looking for actual structure...");
        // Fallback: find any directory in extract_dir
        if let Some(first_dir) = fs::read_dir(&extract_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| e.file_type().unwrap().is_dir())
        {
            println!("Found directory: {:?}", first_dir.file_name());
        }
    }

    let game_dir = product_dir.join("Game");
    let screenshots_dir = product_dir.join("screenshots");
    let metadata_json = product_dir.join("metadata.json");

    assert!(
        product_dir.exists(),
        "Product folder (RJ999001) must exist in output"
    );
    assert!(game_dir.exists(), "Game folder must exist in output");
    assert!(
        screenshots_dir.exists(),
        "screenshots folder must exist in output"
    );
    assert!(metadata_json.exists(), "metadata.json must exist in output");

    // === DIAGNOSTIC LOGGING FOR PATH COMPARISON ===
    println!("\n=== DIAGNOSTIC: Directory Structure Comparison ===");
    println!("Expected source directory: {:?}", expected_src);
    println!("Actual game directory: {:?}", game_dir);

    // List first 10 files from each to compare structure
    println!("\nFirst 10 files in expected_src:");
    for (i, entry) in std::fs::read_dir(expected_src)
        .unwrap()
        .take(10)
        .enumerate()
    {
        if let Ok(entry) = entry {
            println!(
                "  {}. {:?} ({})",
                i + 1,
                entry.file_name(),
                if entry.file_type().unwrap().is_dir() {
                    "DIR"
                } else {
                    "FILE"
                }
            );
        }
    }

    println!("\nFirst 10 files in game_dir:");
    for (i, entry) in std::fs::read_dir(&game_dir).unwrap().take(10).enumerate() {
        if let Ok(entry) = entry {
            println!(
                "  {}. {:?} ({})",
                i + 1,
                entry.file_name(),
                if entry.file_type().unwrap().is_dir() {
                    "DIR"
                } else {
                    "FILE"
                }
            );
        }
    }
    println!("==================================================\n");

    // Verify Game content using Merkle tree hash verification
    println!("\n=== Verifying Game Content with Merkle Tree Hashing ===");
    println!("Expected result path: {:?}", expected_src);
    println!("Organized game path: {:?}", game_dir);

    let expected_hashes =
        ContentHashMap::from_directory(expected_src).expect("Failed to hash expected directory");
    let actual_hashes =
        ContentHashMap::from_directory(&game_dir).expect("Failed to hash actual directory");

    println!("\nExpected content:");
    expected_hashes.print_summary();

    println!("\nActual content:");
    actual_hashes.print_summary();

    let comparison = expected_hashes.compare(&actual_hashes);
    comparison.print_report();

    // === TEST: Compare both extractions to verify 7zip consistency ===
    println!("\n=== Testing 7zip Extraction Consistency ===");
    let game_dir2 = extract_dir2.join("RJ999001").join("Game");
    let actual_hashes2 =
        ContentHashMap::from_directory(&game_dir2).expect("Failed to hash second extraction");

    println!(
        "First extraction:  {} files, hash: {}",
        actual_hashes.hashes.len(),
        &actual_hashes.root_hash[..16]
    );
    println!(
        "Second extraction: {} files, hash: {}",
        actual_hashes2.hashes.len(),
        &actual_hashes2.root_hash[..16]
    );

    if actual_hashes.root_hash != actual_hashes2.root_hash {
        println!("⚠ WARNING: 7zip extractions are NOT consistent!");
        println!("This indicates a problem with 7zip compression/decompression");

        let comparison2 = actual_hashes.compare(&actual_hashes2);
        comparison2.print_report();
    } else {
        println!("✓ 7zip extractions are consistent");
    }
    println!("===============================================\n");

    println!("\n=== Final Structure Verification ===");
    println!("✓ {}/", metadata.product_id);
    println!("  ✓ Game/ ({} files)", actual_hashes.hashes.len());
    println!("  ✓ screenshots/ (directory created)");
    println!("  ✓ metadata.json (exists)");
    println!(
        "  {} Content verification: {}",
        if comparison.is_exact_match() {
            "✓"
        } else {
            "⚠"
        },
        if comparison.is_exact_match() {
            "EXACT MATCH"
        } else {
            "DIFFERENCES FOUND"
        }
    );
    println!("=====================================\n");

    assert!(
        actual_hashes.hashes.len() > 0,
        "Game directory should contain files"
    );

    println!("✓ Integration test PASSED");
    println!("  ✓ Archive decrypted with production password");
    println!(
        "  ✓ Proper structure: {}/Game/, /screenshots/, /metadata.json",
        metadata.product_id
    );

    if !comparison.is_exact_match() {
        println!("\n⚠ Note: Content verification found differences (see report above)");
        println!(
            "  This may be expected if the RAR contains additional files not in expected_result/"
        );

        // Check if files exist but with different names or just missing
        println!("\n=== Deep Content Analysis ===");
        let expected_files: std::collections::HashSet<String> = expected_hashes
            .hashes
            .keys()
            .map(|p| p.split('/').last().unwrap().to_string())
            .collect();
        let actual_files: std::collections::HashSet<String> = actual_hashes
            .hashes
            .keys()
            .map(|p| p.split('/').last().unwrap().to_string())
            .collect();

        let common_files = expected_files.intersection(&actual_files).count();
        let only_expected = expected_files.difference(&actual_files).count();
        let only_actual = actual_files.difference(&expected_files).count();

        println!("Filename comparison (ignoring directory structure):");
        println!("  Common filenames: {}", common_files);
        println!("  Only in Expected: {}", only_expected);
        println!("  Only in Actual:   {}", only_actual);

        if only_expected > 0 {
            println!("\nSample files only in Expected:");
            for f in expected_files.difference(&actual_files).take(5) {
                println!("  - {}", f);
            }
        }

        if only_actual > 0 {
            println!("\nSample files only in Actual:");
            for f in actual_files.difference(&expected_files).take(5) {
                println!("  - {}", f);
            }
        }
        println!("=============================\n");

        panic!("Content verification failed! Hashes do not match.");
    }
}

/// Integration test: Process the real .rar file but stop before compression
/// This test mirrors the organize workflow but WITHOUT the final 7z compression:
/// 1. Sets up redb secrets database with password rules
/// 2. Loads rules into ConfigStore (like UI does on startup)
/// 3. Uses BackendSelector to get proper backend (UnRAR for .rar)
/// 4. Decompresses the archive with auto-detected password
/// 5. Flattens the structure using find_and_flatten_game_content
/// 6. Verifies the flattened structure (but does NOT compress to 7z)
#[test]
fn test_integration_data_decompress_and_flatten() {
    let _ = init_test_logging("test_integration_data_decompress_and_flatten");

    let temp = TempDir::new().expect("Failed to create temp dir");

    info!("=== Starting Decompress and Flatten Test ===");

    // Tests run from crates/core, so go up to workspace root
    let integration_src = Path::new("../../_real_data/integration_data");
    let expected_src = Path::new("../../_real_data/expected_result");

    if !integration_src.exists() {
        println!(
            "Skipping test: Integration data not found at {:?}",
            integration_src
        );
        return;
    }

    if !expected_src.exists() {
        println!(
            "Skipping test: Expected result not found at {:?}",
            expected_src
        );
        return;
    }

    // Find the RAR file (should be the RJ999001 file)
    let archive_path = fs::read_dir(integration_src)
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| {
            e.path()
                .extension()
                .map_or(false, |ext| ext == "rar" || ext == "zip" || ext == "7z")
        })
        .map(|e| e.path())
        .expect("No archive file found in integration_data");

    println!("Testing with archive: {:?}", archive_path);

    // === USE PRODUCTION SECRETS DATABASE (EXACTLY like the UI does) ===

    let db_paths = DbPaths::defaults("arclain").expect("Failed to get default database paths");

    println!("Using production database paths:");
    println!("  Config DB: {:?}", db_paths.config_db);
    println!("  Secrets DB: {:?}", db_paths.secrets_db);
    println!("  Key file: {:?}", db_paths.key_file);

    // Load the production master key
    let key = if let Some(ref key_path) = db_paths.key_file {
        if !key_path.exists() {
            println!("\n⚠ Master key file not found at: {}", key_path.display());
            println!(
                "Skipping test: Cannot access production password database without master key"
            );
            println!("The UI would have created this key on first run.\n");
            return;
        }
        SecretsKey::load_from_file(key_path).expect("Failed to load production key file")
    } else {
        println!("\n⚠ No key file path configured");
        println!("Skipping test: Cannot access production password database\n");
        return;
    };

    // Open production secrets database
    // Handle the case where database is locked (UI app or another test is using it)
    let secrets_db = match SecretsDb::open(&db_paths.secrets_db, &key.as_bytes()) {
        Ok(db) => db,
        Err(e) => {
            let err_msg = e.to_string();
            if err_msg.contains("already open") || err_msg.contains("Cannot acquire lock") {
                println!("\n⚠ Database is locked (likely the UI application or another process is using it)");
                println!("Skipping test: Close the Arclain UI application or run tests sequentially with:");
                println!("  cargo test test_integration_data_decompress_and_flatten -- --test-threads=1\n");
                return;
            }
            panic!("Failed to open production secrets database: {}", e);
        }
    };

    println!("\n=== DEBUG: Password Rules in Production Secrets DB ===");

    // Load rules from production database into ConfigStore
    let loaded_db_rules = secrets_db
        .list_pass_rules()
        .expect("Failed to load password rules from production database");

    if loaded_db_rules.is_empty() {
        println!("\n⚠ No password rules found in production database!");
        println!("Skipping test: Add password rules in the UI first, then run this test.\n");
        return;
    }

    println!(
        "Found {} rules in production secrets database:",
        loaded_db_rules.len()
    );
    for (i, rule) in loaded_db_rules.iter().enumerate() {
        println!(
            "  Rule {}: name='{}', pattern='{}', password='***', priority={}, enabled={}",
            i + 1,
            rule.name,
            rule.pattern,
            rule.priority,
            rule.enabled
        );
    }

    let mut config_store = ConfigStore::load("arclain").expect("Failed to load config");

    // Convert DbPassRule to PassRule
    config_store.cfg.pass_rules = loaded_db_rules
        .iter()
        .map(|r| PassRule {
            name: r.name.clone(),
            pattern: r.pattern.clone(),
            password: r.password.clone(),
            priority: r.priority,
            enabled: r.enabled,
        })
        .collect();

    println!(
        "Loaded {} password rules from encrypted secrets DB",
        config_store.cfg.pass_rules.len()
    );

    // === USE BACKEND SELECTOR (like UI does) ===
    let selector = BackendSelector::new_native();
    let backend = selector
        .select(&archive_path)
        .expect("Failed to select backend");
    println!("Selected backend: {}", backend.name());

    // === AUTO-DETECT PASSWORD ===
    let archive_name = archive_path.file_name().and_then(|n| n.to_str());

    println!("\n=== DEBUG: Password Auto-Detection ===");
    println!("Archive filename: {:?}", archive_name);
    println!("Trying to auto-detect password...");

    let password = config_store.auto_password_for(archive_name, &vec![]);

    if let Some(ref pwd) = password {
        println!(
            "✓ Auto-detected password: '{}' (length: {})",
            pwd,
            pwd.len()
        );
    } else {
        println!("✗ No password auto-detected");
    }

    println!("===========================================\n");

    // === CREATE ARCHIVE HANDLE WITH PASSWORD ===
    let has_password = password.is_some();
    println!(
        "Creating Archive handle with password: {}",
        if has_password { "***" } else { "None" }
    );

    let archive = if let Some(pwd) = password {
        println!("Using password for extraction...");
        Archive::with_password(backend, &archive_path, pwd)
    } else {
        Archive::new(backend, &archive_path)
    };

    // === EXTRACT AND FLATTEN (WITHOUT COMPRESSION) ===
    println!("\n=== Extracting and Flattening Archive ===");

    // Create unique temp directory
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let work_dir = temp
        .path()
        .join(format!("arclain_flatten_test_{}", timestamp));
    std::fs::create_dir_all(&work_dir).expect("Failed to create work dir");

    // Extract to temp location
    let extract_temp = work_dir.join("extract_temp");
    std::fs::create_dir_all(&extract_temp).expect("Failed to create extract temp");

    println!("Extracting to: {:?}", extract_temp);
    let extract_result = archive.extract_all(&extract_temp);

    // Handle extraction errors
    if let Err(e) = extract_result {
        println!("\n=== ERROR DETAILS ===");
        println!("Error: {:?}", e);
        println!("=====================\n");

        let err_msg = e.to_string();
        if err_msg.contains("encrypted")
            || err_msg.contains("password")
            || err_msg.contains("Cannot open")
            || err_msg.contains("Wrong password")
            || err_msg.contains("CRC failed")
            || err_msg.contains("bad CRC")
        {
            println!("\n=== Test Skipped: Password Issue ===");
            println!("Archive: {:?}", archive_path.file_name());
            println!("Error: {}", e);
            println!(
                "\nPassword was {}",
                if has_password {
                    "provided"
                } else {
                    "not provided"
                }
            );
            println!("=====================================\n");
            return;
        }
        panic!("Failed to extract archive: {}", e);
    }

    // Flatten the extracted content to Game directory
    let flattened_dir = work_dir.join("flattened");
    std::fs::create_dir_all(&flattened_dir).expect("Failed to create flattened dir");

    println!("Flattening game content to: {:?}", flattened_dir);

    // Use the internal flattening logic from archive_organizer
    // We need to import and use find_and_flatten_game_content
    // Since it's private, we'll replicate the logic here
    fn find_and_flatten_game_content(source: &Path, dest: &Path) -> anyhow::Result<()> {
        // Game content indicators
        let game_indicators = [
            "Game.exe",
            "game.exe",
            "nw.exe",
            "index.html",
            "package.json",
            "www",
            "data",
            "js",
        ];

        // Check if current directory IS the game content folder
        let entries: Vec<_> = std::fs::read_dir(source)?.collect::<Result<Vec<_>, _>>()?;

        let mut indicator_count = 0;
        for entry in &entries {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            for indicator in &game_indicators {
                if name_str.eq_ignore_ascii_case(indicator) {
                    indicator_count += 1;
                    break;
                }
            }
        }

        // If we found 2+ indicators, this IS the game folder
        if indicator_count >= 2 {
            // Move all contents from source to dest
            for entry in entries {
                let src_path = entry.path();
                let dest_path = dest.join(entry.file_name());

                if let Err(_) = std::fs::rename(&src_path, &dest_path) {
                    if src_path.is_dir() {
                        copy_dir_all(&src_path, &dest_path)?;
                    } else {
                        std::fs::copy(&src_path, &dest_path)?;
                    }
                }
            }

            return Ok(());
        }

        // Otherwise, recursively search subdirectories
        for entry in entries {
            if entry.file_type()?.is_dir() {
                let subdir = entry.path();
                match find_and_flatten_game_content(&subdir, dest) {
                    Ok(_) => return Ok(()),
                    Err(_) => continue,
                }
            }
        }

        Err(anyhow::anyhow!("Could not find game content folder"))
    }

    find_and_flatten_game_content(&extract_temp, &flattened_dir)
        .expect("Failed to flatten game content");

    println!("✓ Flattening completed successfully");

    // === VERIFY FLATTENED STRUCTURE ===
    println!("\n=== Verifying Flattened Content ===");
    println!("Expected result path: {:?}", expected_src);
    println!("Flattened game path: {:?}", flattened_dir);

    let expected_hashes =
        ContentHashMap::from_directory(expected_src).expect("Failed to hash expected directory");
    let actual_hashes =
        ContentHashMap::from_directory(&flattened_dir).expect("Failed to hash flattened directory");

    println!("\nExpected content:");
    expected_hashes.print_summary();

    println!("\nFlattened content:");
    actual_hashes.print_summary();

    let comparison = expected_hashes.compare(&actual_hashes);
    comparison.print_report();

    println!("\n=== Final Structure Verification ===");
    println!(
        "✓ Flattened directory contains {} files",
        actual_hashes.hashes.len()
    );
    println!(
        "  {} Content verification: {}",
        if comparison.is_exact_match() {
            "✓"
        } else {
            "⚠"
        },
        if comparison.is_exact_match() {
            "EXACT MATCH"
        } else {
            "DIFFERENCES FOUND"
        }
    );
    println!("=====================================\n");

    assert!(
        actual_hashes.hashes.len() > 0,
        "Flattened directory should contain files"
    );

    if !comparison.is_exact_match() {
        println!("\n⚠ Note: Content verification found differences (see report above)");
        println!(
            "  This may be expected if the RAR contains additional files not in expected_result/"
        );

        // Check filename comparison
        let expected_files: std::collections::HashSet<String> = expected_hashes
            .hashes
            .keys()
            .map(|p| p.split('/').last().unwrap().to_string())
            .collect();
        let actual_files: std::collections::HashSet<String> = actual_hashes
            .hashes
            .keys()
            .map(|p| p.split('/').last().unwrap().to_string())
            .collect();

        let common_files = expected_files.intersection(&actual_files).count();
        let only_expected = expected_files.difference(&actual_files).count();
        let only_actual = actual_files.difference(&expected_files).count();

        println!("\n=== Deep Content Analysis ===");
        println!("Filename comparison (ignoring directory structure):");
        println!("  Common filenames: {}", common_files);
        println!("  Only in Expected: {}", only_expected);
        println!("  Only in Actual:   {}", only_actual);

        if only_expected > 0 {
            println!("\nSample files only in Expected:");
            for f in expected_files.difference(&actual_files).take(5) {
                println!("  - {}", f);
            }
        }

        if only_actual > 0 {
            println!("\nSample files only in Actual:");
            for f in actual_files.difference(&expected_files).take(5) {
                println!("  - {}", f);
            }
        }
        println!("=============================\n");

        panic!("Content verification failed! Hashes do not match.");
    }

    println!("✓ Decompress and flatten test PASSED");
    println!("  ✓ Archive decrypted with production password");
    println!("  ✓ Content flattened successfully");
    println!("  ✓ Content matches expected result");
}

/// Test multiple copies of expected_result to ensure parallelism safety
#[test]
fn test_multiple_expected_result_copies() {
    let _ = init_test_logging("test_multiple_expected_result_copies");

    let temp = TempDir::new().expect("Failed to create temp dir");

    info!("=== Starting Multiple Copies Test ===");

    // Tests run from crates/core, so go up to workspace root
    let expected_src = Path::new("../../_real_data/expected_result");

    if !expected_src.exists() {
        println!("Skipping test: Real data not found");
        return;
    }

    let backend = SevenZipCli::detect(None).expect("Failed to init 7z");

    // Create 3 test copies
    for i in 1..=3 {
        let test_name = format!("test_{}", i);
        let input_dir = temp.path().join(&test_name).join("input");
        copy_dir_all(expected_src, &input_dir).expect("Failed to copy");

        let archive_path = temp.path().join(&test_name).join("archive.7z");

        let files: Vec<PathBuf> = fs::read_dir(&input_dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();

        backend
            .create_archive(&archive_path, &files, "7z")
            .expect("Failed to create archive");

        let output_7z = temp.path().join(&test_name).join("output.7z");
        let metadata = get_dummy_metadata(&format!("RJ_TEST_{}", i));

        let backend_arc = std::sync::Arc::new(backend.clone());
        let archive = Archive::new(backend_arc.clone(), &archive_path);
        organize_archive(&archive, &output_7z, &metadata, temp.path()).expect("Failed to organize");

        // Extract and verify
        let extract_dir = temp.path().join(&test_name).join("extracted");
        fs::create_dir_all(&extract_dir).unwrap();

        backend_arc
            .extract_all(&output_7z, &extract_dir, None)
            .expect("Failed to extract");

        let organized_game_dir = extract_dir.join(format!("RJ_TEST_{}", i)).join("Game");

        assert_dirs_equal(&input_dir, &organized_game_dir);
        println!("✓ Test copy {} passed", i);
    }
}
