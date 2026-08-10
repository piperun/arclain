# Failing-init component fixture

This package-valid component returns metadata that exactly matches its test
manifest, then deliberately traps from `init`. Package-manager tests use it to
prove preview performs metadata validation without initialization and that a
real initialization failure removes staged sidecars and live manager state.

Regenerate the checked-in component from the repository root:

```powershell
$env:CARGO_TARGET_DIR = "$PWD/target/plugin-fixture"
cargo build --manifest-path crates/plugins/tests/fixtures/failing-init/Cargo.toml --target wasm32-wasip2 --release --locked --offline
Copy-Item target/plugin-fixture/wasm32-wasip2/release/failing_init.wasm crates/plugins/tests/fixtures/failing-init/failing-init.wasm
```

The generated `.wasm` is intentionally force-tracked despite the repository's
global `*.wasm` ignore rule. Its SHA-256 is updated here whenever the fixture
is regenerated:
`34463363283BF4D8D60E55C677CC73843942D5328D8A62A89A4053E9174F5EE4`.
