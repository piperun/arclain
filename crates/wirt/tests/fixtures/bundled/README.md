# Bundled component fixtures

These WebAssembly components are test inputs for the Wirt loader and plugin
manager. Product builds do not ship these loose files; `just plugins` builds
and validates `.wirt` packages from the maintained plugin projects.

The fixtures were built from the matching projects with the repository's
`wasm32-wasip2` release profile:

- `dlsite-metadata.wasm`: SHA-256
  `cf49ad0c35ccb515c3873243b3bb2c0dd1bcc682dded6507b030531362766f82`
- `ui-demo.wasm`: SHA-256
  `4050a8799d131a22e09d5d24ff59fef0d4cb80155da1cd663dd6f6a7d7904b2f`

When either maintained guest changes, rebuild all plugin projects with
`just plugins`, copy the corresponding release component from the isolated
Cargo target into this directory, update the hashes above, and run the full
Wirt and plugin test suites. Keep the committed filenames stable because the
tests use them as immutable component inputs.
