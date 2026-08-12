# Bundled component fixtures

These WebAssembly components are test inputs for the Wirt loader and plugin
manager. Product builds do not ship these loose files; `just plugins` builds
and validates `.wirt` packages from the maintained plugin projects.

The fixtures were built from the matching projects with the repository's
`wasm32-wasip2` release profile:

- `dlsite-metadata.wasm`: SHA-256
  `40f99ec24cebc2cfcbef8f543db2e6f4a353dd73d7ceb0db912e359e83ca5476`
- `ui-demo.wasm`: SHA-256
  `282e94f520a1677314851a069f7ae33cf08620e5331b30de85fd6c9cc6ac98a9`

When either maintained guest changes, rebuild all plugin projects with
`just plugins`, copy the corresponding release component from the isolated
Cargo target into this directory, update the hashes above, and run the full
Wirt and plugin test suites. Keep the committed filenames stable because the
tests use them as immutable component inputs.
