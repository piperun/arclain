# Bundled component fixtures

These WebAssembly components are test inputs for the Wirt loader and plugin
manager. Product builds do not ship these loose files; `just plugins` builds
and validates `.wirt` packages from the maintained plugin projects.

The fixtures were built from the matching projects with the repository's
`wasm32-wasip2` release profile:

- `dlsite-metadata.wasm`: SHA-256
  `ada8048bf36b3edcefc1605756a5bdb1887106b19128c16e58916507e9f2c5cc`
- `ui-demo.wasm`: SHA-256
  `83ca758c6e2ee75b808d950880fc69cabae9e0bb0b6f64ab44cd0e470a8eea47`

When either maintained guest changes, rebuild all plugin projects with
`just plugins`, copy the corresponding release component from the isolated
Cargo target into this directory, update the hashes above, and run the full
Wirt and plugin test suites. Keep the committed filenames stable because the
tests use them as immutable component inputs.
