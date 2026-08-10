# Malicious metadata component fixture

This component deliberately calls the `create-file`, `log`, and
`show-message` host imports from `get-metadata` before returning an invalid
plugin ID. Its `init` export also traps when invoked by the restricted
metadata-validation host, making an accidental pre-validation `init` call
observable. Metadata retrieval also probes WASI arguments and environment,
writes unique raw stdout/stderr sentinels while ignoring write errors, and
returns the safe ID `args-leaked` if any process context is visible.

Its `get-default-rules` export logs once and returns a rule whose neutral-only
description is 1 MiB. Wirt runtime tests use that export to prove the complete
neutral rule counts toward the serialized-result quota and that terminal reuse
does not re-enter the guest.

Regenerate the checked-in component from the repository root:

```powershell
$env:CARGO_TARGET_DIR = "$PWD/target/plugin-fixture"
cargo build --manifest-path crates/plugins/tests/fixtures/malicious-metadata/Cargo.toml --target wasm32-wasip2 --release --locked --offline
Copy-Item target/plugin-fixture/wasm32-wasip2/release/malicious_metadata.wasm crates/plugins/tests/fixtures/malicious-metadata/malicious-metadata.wasm
```

The generated `.wasm` is intentionally force-tracked despite the repository's
global `*.wasm` ignore rule. Its SHA-256 is updated here whenever the fixture
is regenerated:
`493AECA9A016DAB31657C801217CFA84EE6AF3B96813268132B5BE7AD7DD7559`.
