use super::*;

#[test]
fn test_logging_init() {
    // This may fail if already initialized, which is fine for tests
    let _ = init_logging();
}
