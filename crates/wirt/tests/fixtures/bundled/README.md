# Bundled component fixtures

These WebAssembly components are test inputs for the Wirt loader and plugin
manager. Product builds do not ship these loose files; `just plugins` builds
and validates `.wirt` packages from the maintained plugin projects.

The fixtures were built from the matching projects with the repository's
`wasm32-wasip2` release profile:

- `dlsite-metadata.wasm`: SHA-256
  `6bbf0862e779541486e0d04dde4d4ba946943fbf4e208b7ecf786c75cccf460c`
- `ui-demo.wasm`: SHA-256
  `538514b87cdaf977ad586b04b3de225973178eae8d348d44c3814ada972313f8`

When either maintained guest changes, rebuild all plugin projects with
`just plugins`, copy the corresponding release component from the isolated
Cargo target into this directory, update the hashes above, and run the full
Wirt and plugin test suites. Keep the committed filenames stable because the
tests use them as immutable component inputs.
