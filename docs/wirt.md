# Wirt in Arclain

Arclain consumes the shared [Wirt platform](https://codeberg.org/0xdev/wirt)
as an external host crate, guest SDK, and developer CLI. The exact reviewed
Git revision, CLI version, and ABI are recorded in
[`wirt-toolchain.toml`](../wirt-toolchain.toml); `scripts/wirt_dependency.py`
keeps every host manifest, guest manifest, and lockfile on that one pin.

The Wirt repository owns the canonical WIT, runtime sandbox, package format,
guest SDK, developer CLI, platform security model, and product-neutral
conformance fixtures. Arclain does not carry a second Wirt implementation or
WIT package.

Arclain owns only its product adapters around Wirt:

- plugin discovery, installation, lifecycle, and application-facing manager;
- host implementations for settings, data, networking, logging, files, and
  archive operations;
- Gameta/DLsite integration and the legacy DLsite compatibility fixture;
- application session and UI projection code; and
- release assembly for Arclain's maintained plugin packages.

Install the CLI pin manually before building maintained guests:

```console
cargo install --locked --git https://codeberg.org/0xdev/wirt.git --rev 1fc2a8edcbb17830a6c6f46604453ca9126dc387 wirt-cli
wirt --version
```

The version output must be `wirt-cli 0.3.0 (ABI 0.3.0)`. The `WIRT`
environment variable may name an alternate path to that exact executable.
`just plugins` verifies the identity once before building any guest and never
installs or updates the tool.

Platform details for this pin are available in Wirt's
[ABI policy](https://codeberg.org/0xdev/wirt/src/commit/1fc2a8edcbb17830a6c6f46604453ca9126dc387/docs/abi-policy.md),
[package format](https://codeberg.org/0xdev/wirt/src/commit/1fc2a8edcbb17830a6c6f46604453ca9126dc387/docs/package-format.md), and
[security model](https://codeberg.org/0xdev/wirt/src/commit/1fc2a8edcbb17830a6c6f46604453ca9126dc387/docs/security-model.md).
