# Bundled component fixtures

These WebAssembly components are test inputs for the Wirt loader and plugin
manager. Product builds do not ship these loose files; `just plugins` builds
and validates `.wirt` packages from the maintained plugin projects.

The fixtures were built from the matching projects with the repository's
`wasm32-wasip2` release profile:

- `dlsite-metadata.wasm`: SHA-256
  `4fe02a41bd63ba68191e17c0b825042acb01a36c178e555707889fbac018b556`
- `ui-demo.wasm`: SHA-256
  `315a7663100cd4500b206fcfabad548269add3076c58d2d7a5e057cd36e237d4`

When either maintained guest changes, rebuild all plugin projects with
`just plugins`, copy the corresponding release component from the isolated
Cargo target into this directory, update the hashes above, and run the full
Wirt and plugin test suites. Keep the committed filenames stable because the
tests use them as immutable component inputs.

## Extraction contract

These hashes characterize the reviewed Wirt extraction inputs for ABI `0.3.0`;
they do not define bytes for future ABI versions.

- Canonical WIT (`wirt-sdk/wit/plugin.wit`): SHA-256
  `71e32b758c7b512d2f0e9f41050f19cc7060123820dd5f2445e0dda01af7957b`
- Generated schema projection body (`crates/wirt/src/wirt_schema.rs`): SHA-256
  `6e1193d3062be83a0cd3ffadfd5762f4118d965cc8a38a45258072e71fcf24f1`
- Original full generated schema file: SHA-256
  `65e8231d55b0dadcab1a677e888010af15edf04342a7118145f26e5d254a931f`
- Immutable package fixture (`facade-test-fixture.wirt`): SHA-256
  `bd04a0eb2684c8eb6e284f68c59a5804b8876455a0864e42bbc537545df00479`

The schema body hash excludes only the first generated source-path comment.
