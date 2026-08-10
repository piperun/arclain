use arclain_plugins::{QuarantineLedger, QuarantineState};

fn ledger_root() -> (tempfile::TempDir, std::sync::Arc<wirt::TrustedPluginRoot>) {
    let root = tempfile::tempdir().expect("create plugin root");
    let loader = wirt::PluginLoader::new(root.path().to_path_buf()).expect("open plugin root");
    (root, loader.trusted_root())
}

#[test]
fn retry_counts_are_persistent_bounded_and_fingerprint_scoped() {
    let (_root, trusted_root) = ledger_root();
    let fingerprint = wirt::PackageFingerprint::sha256(b"first package");
    let replacement = wirt::PackageFingerprint::sha256(b"replacement package");
    let ledger = QuarantineLedger::open(trusted_root.clone()).unwrap();

    ledger
        .record_initial_violation(&fingerprint, "plugin fuel quota exceeded")
        .unwrap();
    assert!(matches!(
        ledger.state(&fingerprint),
        QuarantineState::Retryable(ref record)
            if record.failed_retries == 0
                && record.last_reason == "plugin fuel quota exceeded"
    ));
    assert_eq!(ledger.state(&replacement), QuarantineState::Clear);

    let reopened = QuarantineLedger::open(trusted_root.clone()).unwrap();
    assert_eq!(reopened.state(&fingerprint), QuarantineState::Clear);

    for expected in 1..=3 {
        reopened
            .record_failed_retry(&fingerprint, "plugin fuel quota exceeded")
            .unwrap();
        let state = reopened.state(&fingerprint);
        if expected < 3 {
            assert!(matches!(
                state,
                QuarantineState::Retryable(ref record)
                    if record.failed_retries == expected
            ));
        } else {
            assert!(matches!(
                state,
                QuarantineState::PersistentlyDisabled(ref record)
                    if record.failed_retries == 3
            ));
        }
    }

    let persisted = QuarantineLedger::open(trusted_root.clone()).unwrap();
    assert!(matches!(
        persisted.state(&fingerprint),
        QuarantineState::PersistentlyDisabled(ref record)
            if record.failed_retries == 3
    ));
    persisted.reset(&fingerprint).unwrap();
    assert_eq!(persisted.state(&fingerprint), QuarantineState::Clear);
    assert_eq!(
        QuarantineLedger::open(trusted_root)
            .unwrap()
            .state(&fingerprint),
        QuarantineState::Clear
    );
}

#[test]
fn corrupt_or_oversized_ledger_fails_closed_with_a_bounded_error() {
    for bytes in [b"{".to_vec(), vec![b'x'; 1024 * 1024 + 1]] {
        let (root, trusted_root) = ledger_root();
        std::fs::write(root.path().join(".wirt-quarantine.json"), bytes).unwrap();

        let error = QuarantineLedger::open(trusted_root).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("quarantine ledger"));
        assert!(message.len() <= 256);
    }
}

#[test]
fn ledger_rejects_invalid_fingerprints_counts_reasons_and_entry_overflow() {
    let cases = [
        r#"{"version":1,"records":{"ABC":{"failed_retries":1,"last_reason":"x"}}}"#.to_string(),
        format!(
            r#"{{"version":1,"records":{{"{}":{{"failed_retries":4,"last_reason":"x"}}}}}}"#,
            "0".repeat(64)
        ),
        format!(
            r#"{{"version":1,"records":{{"{}":{{"failed_retries":1,"last_reason":"{}"}}}}}}"#,
            "0".repeat(64),
            "x".repeat(257)
        ),
        format!(
            r#"{{"version":1,"records":{{"{0}":{{"failed_retries":1,"last_reason":"first"}},"{0}":{{"failed_retries":2,"last_reason":"second"}}}}}}"#,
            "0".repeat(64)
        ),
    ];
    for json in cases {
        let (root, trusted_root) = ledger_root();
        std::fs::write(root.path().join(".wirt-quarantine.json"), json).unwrap();
        assert!(QuarantineLedger::open(trusted_root).is_err());
    }

    let (root, trusted_root) = ledger_root();
    let mut records = serde_json::Map::new();
    for index in 0..1025_u32 {
        let fingerprint = wirt::PackageFingerprint::sha256(&index.to_le_bytes());
        records.insert(
            fingerprint.to_string(),
            serde_json::json!({"failed_retries": 1, "last_reason": "x"}),
        );
    }
    std::fs::write(
        root.path().join(".wirt-quarantine.json"),
        serde_json::to_vec(&serde_json::json!({"version": 1, "records": records})).unwrap(),
    )
    .unwrap();
    assert!(QuarantineLedger::open(trusted_root).is_err());
}
