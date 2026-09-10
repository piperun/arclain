#![cfg(feature = "worker")]
use arclain_archive::{ArchiveClient, Cancellation, WorkerOptions};
use std::{io::Read, path::PathBuf, time::Duration};
use unboxer::{inspect_inventory, SequentialArchive};
fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}
fn options() -> WorkerOptions {
    WorkerOptions::new(PathBuf::from(env!("CARGO_BIN_EXE_arclain-archive-worker")))
}
fn limits() -> unboxer::Limits {
    unboxer::Limits::new(100, 1 << 20, 200, 1 << 20)
}

#[test]
fn zip_solid_lzma2_256m_and_bcj2_stream_to_completion() {
    for name in ["game.zip", "solid.7z", "bcj2.7z"] {
        let mut archive = ArchiveClient::new(options())
            .open(&fixture(name), limits(), Cancellation::default())
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let summary = inspect_inventory(archive.inventory(), &limits()).unwrap();
        assert_eq!(summary.declared_total_bytes, 16384);
        assert_eq!(summary.wrapping_folder.as_deref(), Some("Game"));
        let mut ids = vec![];
        let mut bytes = 0;
        while let Some(mut entry) = archive.next_entry().unwrap() {
            let mut buffer = [0; 4096];
            loop {
                let n = entry.reader.read(&mut buffer).unwrap();
                if n == 0 {
                    break;
                }
                bytes += n;
            }
            ids.push(entry.index);
        }
        ids.sort();
        assert_eq!(ids, (0..summary.entries.len()).collect::<Vec<_>>());
        assert_eq!(bytes, 16384);
    }
}

#[test]
fn a_worker_below_dictionary_budget_fails_explicitly() {
    let mut options = options();
    options.memory_bytes = 64 << 20;
    let client = ArchiveClient::new(options);
    let result = client
        .open(&fixture("solid.7z"), limits(), Cancellation::default())
        .and_then(|mut session| {
            while let Some(mut stream) = session
                .next_entry()
                .map_err(|_| arclain_archive::ImportError::WorkerFailed)?
            {
                std::io::copy(&mut stream.reader, &mut std::io::sink())
                    .map_err(|_| arclain_archive::ImportError::WorkerFailed)?;
            }
            Ok(())
        });
    assert!(
        result.is_err(),
        "decoder unexpectedly ignored the 64 MiB memory ceiling"
    );
}

#[test]
fn cancellation_and_drop_reap_the_worker() {
    for cancel_now in [false, true] {
        let cancellation = Cancellation::default();
        let mut session = ArchiveClient::new(options())
            .open(&fixture("solid.7z"), limits(), cancellation.clone())
            .unwrap();
        let pid = session.process_id();
        if cancel_now {
            cancellation.cancel();
            assert!(session.next_entry().is_err());
        }
        drop(session);
        #[cfg(unix)]
        assert!(!PathBuf::from(format!("/proc/{pid}")).exists());
        #[cfg(windows)]
        {
            use windows_sys::Win32::Foundation::CloseHandle;
            use windows_sys::Win32::System::Threading::{
                GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
            };
            unsafe {
                let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
                if !handle.is_null() {
                    let mut code = 259;
                    assert_ne!(GetExitCodeProcess(handle, &mut code), 0);
                    CloseHandle(handle);
                    assert_ne!(code, 259);
                }
            }
        }
    }
}

#[test]
fn zero_timeout_does_not_hang() {
    let mut config = options();
    config.stall_timeout = Duration::ZERO;
    assert!(ArchiveClient::new(config)
        .open(&fixture("game.zip"), limits(), Cancellation::default())
        .is_err());
}

#[test]
fn rar4_ascii_and_rar5_utf8_decode_without_native_output_paths() {
    for name in ["game-rar4.rar", "game-rar5.rar"] {
        let mut session = ArchiveClient::new(options())
            .open(&fixture(name), limits(), Cancellation::default())
            .unwrap();
        assert_eq!(session.inventory().entries.len(), 1);
        inspect_inventory(session.inventory(), &limits()).unwrap();
        let mut stream = session.next_entry().unwrap().unwrap();
        let mut bytes = Vec::new();
        stream.reader.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"benign archive fixture");
        drop(stream);
        assert!(session.next_entry().unwrap().is_none());
    }
}

#[test]
fn rar_headers_winrar_writes_decode_to_the_names_and_bytes_unrar_reports() {
    // RAR4 with an extended time record, a Unicode name, and sub-blocks
    // around the entry; RAR5 as WinRAR 7 writes it: an extra area on every
    // header, a locator in the main header, and the archive's own name and
    // time when asked for.
    for (name, expected) in [
        ("rar4-exttime.rar", "Game/game.exe"),
        ("rar4-unicode.rar", "Game/日本.txt"),
        ("rar4-unicode-runs.rar", "Game/ゲーム.txt"),
        ("rar4-subblocks.rar", "Game/game.exe"),
        ("rar5-winrar.rar", "benign.txt"),
        ("rar5-winrar-unicode.rar", "日本語.txt"),
        ("rar5-winrar-metadata.rar", "benign.txt"),
    ] {
        let mut session = ArchiveClient::new(options())
            .open(&fixture(name), limits(), Cancellation::default())
            .unwrap_or_else(|error| panic!("{name}: {error:?}"));
        let validated = inspect_inventory(session.inventory(), &limits()).unwrap();
        assert_eq!(validated.entries.len(), 1, "{name}");
        assert_eq!(validated.entries[0].path.join("/"), expected, "{name}");
        let mut stream = session.next_entry().unwrap().unwrap();
        let mut bytes = Vec::new();
        stream.reader.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"benign archive fixture", "{name}");
        drop(stream);
        assert!(session.next_entry().unwrap().is_none());
    }
}

#[test]
fn a_solid_rar5_with_quick_open_and_recovery_records_lists_only_its_entries() {
    let mut session = ArchiveClient::new(options())
        .open(
            &fixture("rar5-winrar-solid-rr.rar"),
            limits(),
            Cancellation::default(),
        )
        .unwrap();
    let validated = inspect_inventory(session.inventory(), &limits()).unwrap();
    assert_eq!(
        validated
            .entries
            .iter()
            .map(|entry| entry.path.join("/"))
            .collect::<Vec<_>>(),
        vec!["benign.txt", "日本語.txt"]
    );
    let mut count = 0;
    while let Some(mut entry) = session.next_entry().unwrap() {
        let mut bytes = Vec::new();
        entry.reader.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"benign archive fixture");
        count += 1;
    }
    assert_eq!(count, 2);
}

#[test]
fn an_encrypted_rar_is_reported_as_encrypted_before_anything_is_decoded() {
    for name in [
        "rar4-password.rar",
        "rar4-encrypted-headers.rar",
        "rar5-winrar-encrypted.rar",
        "rar5-winrar-encrypted-headers.rar",
    ] {
        let error = ArchiveClient::new(options())
            .open(&fixture(name), limits(), Cancellation::default())
            .err()
            .unwrap_or_else(|| panic!("{name} opened"));
        assert_eq!(error, arclain_archive::ImportError::Encrypted, "{name}");
    }
}

#[test]
fn a_delta_filtered_7z_entry_decodes() {
    let mut session = ArchiveClient::new(options())
        .open(&fixture("delta.7z"), limits(), Cancellation::default())
        .unwrap();
    let validated = inspect_inventory(session.inventory(), &limits()).unwrap();
    assert_eq!(validated.entries[0].path, vec!["sound.bin".to_owned()]);
    let mut stream = session.next_entry().unwrap().unwrap();
    let mut bytes = Vec::new();
    stream.reader.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes.len(), 4096);
    assert!(bytes
        .iter()
        .enumerate()
        .all(|(index, byte)| *byte == ((index * 7) % 256) as u8));
}

#[test]
fn rar_native_name_rewriting_is_never_accepted_as_original_metadata() {
    for name in [
        "rar5-invalid-utf8.rar",
        "rar5-nul.rar",
        "rar5-colon.rar",
        "rar5-backslash.rar",
        "rar5-overlong.rar",
        "rar5-traversal.rar",
        "rar4-unproven-encoding.rar",
    ] {
        let result = ArchiveClient::new(options())
            .open(&fixture(name), limits(), Cancellation::default())
            .and_then(|session| {
                inspect_inventory(session.inventory(), &limits())
                    .map_err(|_| arclain_archive::ImportError::Unsupported)
            });
        assert!(result.is_err(), "accepted native rewriting for {name}");
    }
}

#[test]
fn stream_cancellation_retains_the_worker_cause() {
    let cancel = Cancellation::default();
    let mut session = ArchiveClient::new(options())
        .open(&fixture("solid.7z"), limits(), cancel.clone())
        .unwrap();
    let mut entry = session.next_entry().unwrap().unwrap();
    cancel.cancel();
    assert!(std::io::copy(&mut entry.reader, &mut std::io::sink()).is_err());
    drop(entry);
    assert_eq!(
        session.take_error(),
        Some(arclain_archive::ImportError::Cancelled)
    );
}

#[test]
fn an_idle_session_outlives_the_stall_timeout_and_still_decodes() {
    let mut config = options();
    config.stall_timeout = Duration::from_millis(300);
    let mut session = ArchiveClient::new(config)
        .open(&fixture("game.zip"), limits(), Cancellation::default())
        .unwrap();
    std::thread::sleep(Duration::from_millis(1000));
    let mut entry = session.next_entry().unwrap().unwrap();
    std::io::copy(&mut entry.reader, &mut std::io::sink()).unwrap();
}

#[test]
fn empty_entries_between_solid_data_streams_keep_physical_identity() {
    let mut session = ArchiveClient::new(options())
        .open(
            &fixture("interleaved.7z"),
            limits(),
            Cancellation::default(),
        )
        .unwrap();
    assert_eq!(session.inventory().entries.len(), 3);
    let mut actual = Vec::new();
    while let Some(mut entry) = session.next_entry().unwrap() {
        let mut bytes = Vec::new();
        entry.reader.read_to_end(&mut bytes).unwrap();
        actual.push((entry.index, bytes));
    }
    actual.sort_by_key(|e| e.0);
    assert_eq!(
        actual,
        vec![(0, b"A".to_vec()), (1, vec![]), (2, b"B".to_vec())]
    );
}
#[test]
fn unproven_rar4_host_schemes_are_refused() {
    for name in ["host6.rar", "host255.rar"] {
        assert!(
            ArchiveClient::new(options())
                .open(&fixture(name), limits(), Cancellation::default())
                .is_err(),
            "accepted {name}"
        );
    }
}

#[test]
fn malformed_names_metadata_and_methods_are_refused() {
    for name in [
        "invalid-utf16.7z",
        "unsafe-name.7z",
        "duplicate-name.7z",
        "unsupported-method.7z",
        "bad-header-crc.7z",
        "encrypted.7z",
    ] {
        let result = ArchiveClient::new(options())
            .open(&fixture(name), limits(), Cancellation::default())
            .and_then(|s| {
                inspect_inventory(s.inventory(), &limits())
                    .map_err(|_| arclain_archive::ImportError::Unsupported)
            });
        assert!(result.is_err(), "accepted {name}");
    }
}
#[test]
fn original_unicode_names_survive_inventory_and_decode() {
    let mut session = ArchiveClient::new(options())
        .open(&fixture("unicode.7z"), limits(), Cancellation::default())
        .unwrap();
    assert_eq!(
        session.inventory().entries[0].name,
        unboxer::EntryName::Unicode("猫".into())
    );
    while let Some(mut entry) = session.next_entry().unwrap() {
        std::io::copy(&mut entry.reader, &mut std::io::sink()).unwrap();
    }
}
#[test]
fn corrupted_payloads_cannot_reach_successful_completion() {
    for name in [
        "bad-data-crc.7z",
        "bad-data-crc-rar4.rar",
        "bad-data-crc-rar5.rar",
    ] {
        let result = ArchiveClient::new(options())
            .open(&fixture(name), limits(), Cancellation::default())
            .and_then(|mut s| {
                while let Some(mut stream) = s
                    .next_entry()
                    .map_err(|_| arclain_archive::ImportError::Corrupt)?
                {
                    std::io::copy(&mut stream.reader, &mut std::io::sink())
                        .map_err(|_| arclain_archive::ImportError::Corrupt)?;
                }
                Ok(())
            });
        assert!(result.is_err(), "accepted {name}");
    }
}
#[test]
fn entry_archive_and_metadata_limits_are_enforced() {
    for name in ["game.zip", "solid.7z", "game-rar5.rar"] {
        for kind in 0..3 {
            let mut l = limits();
            match kind {
                0 => l.max_entries = 0,
                1 => l.max_archive_bytes = 1,
                _ => l.max_index_bytes = 1,
            };
            assert!(
                ArchiveClient::new(options())
                    .open(&fixture(name), l, Cancellation::default())
                    .is_err(),
                "ignored limit {kind} for {name}"
            );
        }
    }
}

#[test]
fn the_default_memory_cap_is_half_the_machine_within_its_clamp() {
    let default = options().memory_bytes;
    assert!((1u64 << 30..=8u64 << 30).contains(&default), "{default}");
}

#[test]
fn a_worker_accepts_a_cap_above_four_gibibytes() {
    let mut config = options();
    config.memory_bytes = 5 << 30;
    let session = ArchiveClient::new(config)
        .open(&fixture("game.zip"), limits(), Cancellation::default())
        .unwrap();
    // The physical inventory of game.zip: two directories and two files.
    assert_eq!(session.inventory().entries.len(), 4);
}
