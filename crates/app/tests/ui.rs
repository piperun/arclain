//! Compile-fail harness proving `SecretInput` implements neither `Clone`
//! nor either Serde trait. See `tests/ui/*.rs` for the individual cases
//! and their committed `.stderr` snapshots.

// Ignored by default because of what it costs, not what it is worth.
// trybuild invokes the compiler once per case, and this single test took
// 310 seconds of a 360-second CI step -- the next slowest test in the
// whole workspace is 58s, so the run was very nearly just waiting for
// this. On donated runners that is the largest thing the project spends.
//
// Safe to scope, unlike the ordinary test suites: the property is that a
// type declared in *this* crate implements neither `Clone` nor either
// Serde trait, and the orphan rule means no other crate can add those
// impls. Only a change under `crates/app/` can alter the answer, which is
// exactly what CI gates it on -- see the `compile-fail` step in
// `.woodpecker.yml`. Tags run it unconditionally.
//
// Run locally with `cargo test -p arclain_app --test ui -- --ignored`.
#[test]
#[ignore = "compiler-invoking; CI runs it when crates/app changes, and on every tag"]
fn ui() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/*.rs");
}
