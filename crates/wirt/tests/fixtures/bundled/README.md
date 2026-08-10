# Bundled component fixtures

These WebAssembly components are test inputs for the Wirt loader and plugin
manager. Product builds do not ship these loose files; `just plugins` builds
and validates `.wirt` packages from the maintained plugin projects.

The fixtures were built from the matching projects with the repository's
`wasm32-wasip2` release profile:

- `dlsite-metadata.wasm`: SHA-256
  `b1e155e563f13b5c5f890c7685bf67f6b8a052a73c0aa128b28c6cbf173b450f`
- `ui-demo.wasm`: SHA-256
  `6ec2d51bb97f84cf71d8c05f7bf3e642d9c0e2df9c8554dec152ddad75f8d0b3`

When either maintained guest changes, rebuild all plugin projects with
`just plugins`, copy the corresponding release component from the isolated
Cargo target into this directory, update the hashes above, and run the full
Wirt and plugin test suites. Keep the committed filenames stable because the
tests use them as immutable component inputs.
