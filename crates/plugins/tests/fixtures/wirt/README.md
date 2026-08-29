# Arclain Wirt host-integration fixtures

These files are immutable Arclain host-integration inputs, not Wirt source.
Their reviewed SHA-256 values are:

```text
dlsite-metadata.wasm       4fe02a41bd63ba68191e17c0b825042acb01a36c178e555707889fbac018b556
ui-demo.wasm               315a7663100cd4500b206fcfabad548269add3076c58d2d7a5e057cd36e237d4
gstreamer-preview.wasm     e58962d3469481f94e889c03a1152588b0b50644277f765e38c028a64adac871
facade-test-fixture.wirt   bd04a0eb2684c8eb6e284f68c59a5804b8876455a0864e42bbc537545df00479
dlsite-metadata.plugin.toml c40a1ce78a67e21050cd0dedd28aa3bf9a9798496743e5a09505d9d3d5bfd78e
```

`ui-demo.wasm` is built from Arclain's maintained `plugins/ui-demo` guest.
`dlsite-metadata.wasm` is the legacy product-compatibility input pending the
separate Gameta migration, and `dlsite-metadata.plugin.toml` is the manifest
it was built with. A Wirt package loads only when its manifest and component
agree on a version, so the pair is frozen together; reading the live manifest
under `plugins/` instead made these checks fail whenever the plugin was
version-bumped. `gstreamer-preview.wasm` is an Arclain integration
fixture, not a Wirt conformance guest. `facade-test-fixture.wirt` exercises
Arclain's host facade; Wirt's own compatibility copy remains independently
pinned in the Wirt repository.
