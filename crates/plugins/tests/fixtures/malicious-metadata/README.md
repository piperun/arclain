# Malicious metadata component fixture

This component deliberately calls the `create-file`, `log`, and
`show-message` host imports from `get-metadata` before returning an invalid
plugin ID. Its `init` export also traps when invoked by the restricted
metadata-validation host, making an accidental pre-validation `init` call
observable.

Regenerate the checked-in component from the repository root:

```powershell
$env:CARGO_TARGET_DIR = "$PWD/target/plugin-fixture"
cargo build --manifest-path crates/plugins/tests/fixtures/malicious-metadata/Cargo.toml --target wasm32-wasip2 --release --locked
Copy-Item target/plugin-fixture/wasm32-wasip2/release/malicious_metadata.wasm crates/plugins/tests/fixtures/malicious-metadata/malicious-metadata.wasm
```

The generated `.wasm` is intentionally force-tracked despite the repository's
global `*.wasm` ignore rule.
