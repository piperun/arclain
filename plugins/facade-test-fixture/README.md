# Facade integration component fixture

This maintained Wirt component exercises the application and UI facade with a
real guest. In addition to ordinary layout and action paths, its
`trigger-result-quota` button returns a response above the one-message limit so
the resource-quarantine lifecycle is tested end to end. The host-seeded
`fail-init = true` setting produces an ordinary init trap so a failed fresh
retry can be distinguished from another quota violation.

Regenerate the checked-in component from a standalone copy of this directory
and `wirt-sdk`:

```powershell
$env:CARGO_TARGET_DIR = "$PWD/target/facade-test-fixture"
cargo build --manifest-path plugins/facade-test-fixture/Cargo.toml --target wasm32-wasip2 --release --locked --offline
Copy-Item target/facade-test-fixture/wasm32-wasip2/release/facade_test_fixture.wasm plugins/facade-test-fixture/facade-test-fixture.wasm
```

The component is force-tracked despite the repository-wide `*.wasm` ignore
rule. Its SHA-256 is:
`ABA33647FFE6C5D623309B25724111DC395B7D366108A77736A30AD369C723ED`.
