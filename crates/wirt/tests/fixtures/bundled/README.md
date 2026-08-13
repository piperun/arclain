# Bundled component fixtures

These WebAssembly components are test inputs for the Wirt loader and plugin
manager. Product builds do not ship these loose files; `just plugins` builds
and validates `.wirt` packages from the maintained plugin projects.

The fixtures were built from the matching projects with the repository's
`wasm32-wasip2` release profile:

- `dlsite-metadata.wasm`: SHA-256
  `382e669fa1316ab97449d70cffda22c2473592215ccaf00a52ad3933016a274f`
- `ui-demo.wasm`: SHA-256
  `315a7663100cd4500b206fcfabad548269add3076c58d2d7a5e057cd36e237d4`

When either maintained guest changes, rebuild all plugin projects with
`just plugins`, copy the corresponding release component from the isolated
Cargo target into this directory, update the hashes above, and run the full
Wirt and plugin test suites. Keep the committed filenames stable because the
tests use them as immutable component inputs.
