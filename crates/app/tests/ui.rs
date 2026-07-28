//! Compile-fail harness proving `SecretInput` implements neither `Clone`
//! nor either Serde trait. See `tests/ui/*.rs` for the individual cases
//! and their committed `.stderr` snapshots.

#[test]
fn ui() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/*.rs");
}
