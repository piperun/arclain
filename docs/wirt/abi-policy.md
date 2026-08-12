# Wirt ABI policy

Wirt is a product-neutral plugin API, SDK, and ABI. The current ABI is
`0.2.0`, declared by the sole source file at
[`wirt-sdk/wit/plugin.wit`](../../wirt-sdk/wit/plugin.wit).

## Exact compatibility while Wirt is 0.x

The host accepts an exact ABI string, not a semver range. A package declaring
`0.2.0` must contain a component implementing the `0.2.0` Wirt world, and the
host itself must be built for `0.2.0`. Any mismatch is rejected before plugin
initialization.

In particular:

- `0.2.1` is not assumed compatible with `0.2.0`.
- `0.3.0` is not assumed compatible with `0.2.0`.
- An omitted `[wirt]` table or malformed version is invalid.
- Renaming a package or changing its filename does not change its ABI.

This strict rule is deliberate while the contract is pre-1.0. It prevents a
host and guest from silently disagreeing about capabilities, resource types,
or nested UI/action shapes.

## One source of truth

Only `wirt-sdk/wit/plugin.wit` may define the Wirt namespace. Build-time schema
generation, host bindings, guest bindings, and component preflight all point
to that file. The repository boundary check rejects duplicate WIT packages,
alternate bindgen inputs, inline Wirt definitions, and unproven macro/import
aliases.

When the WIT changes:

1. Decide whether the observable guest/host contract changed.
2. If it changed, choose a new exact ABI version and update the WIT package,
   host constant, starter `plugin.toml`, SDK/template, maintained manifests,
   schema projection, fixtures, and public documentation together.
3. Rebuild every maintained component and `.wirt` package.
4. Run the Wirt boundary, component-contract, SDK guest, plugin-manager, and
   full workspace tests.
5. Treat old packages as unsupported until an explicit compatibility adapter
   is designed and tested. Do not accept them by weakening the version check.

Generated bindings and generated schema data are outputs of the canonical
WIT; they are not independent specifications.

## Stable package identity

The manifest and guest export five identity fields: ID, name, version, author,
and description. Preview and installation compare all five exactly before
initialization. The plugin ID is also a portable filesystem identity: at most
64 ASCII bytes from `[A-Za-z0-9_-]`, not `.` or `..`, not a Windows reserved
name, and not ending in a dot or space.

Changing any identity field or the component bytes changes the canonical
package SHA-256 fingerprint. Quarantine records are fingerprint-scoped, so a
different valid package is not treated as the same executable artifact.

## What versioning does not promise

The Rust crate version, plugin version, Wirt ABI version, and host product
version are separate values:

- The plugin version describes the plugin release.
- The Wirt ABI version describes the component boundary.
- Rust crate versions describe implementation packages.
- The Arclain version describes the host application.

None substitutes for another. A plugin may release a new version without an
ABI change, but it must still declare and implement the host's exact Wirt ABI.
