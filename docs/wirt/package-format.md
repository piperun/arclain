# Wirt package format

A `.wirt` file is the only public installation artifact. It is a canonical,
deterministic ZIP envelope around one manifest and one WebAssembly Component.
Validation completes before the component is compiled or initialized.

## Canonical layout

The archive contains exactly two entries in this order:

1. `plugin.toml`
2. `plugin.wasm`

Both entries use DEFLATE level 9, the ZIP epoch timestamp, Unix mode `0644`,
and the exact header encoding emitted by the repository-pinned ZIP writer.
They are ordinary files: no directories, links, devices, encryption, data
descriptors, comments, extra fields, alternate filenames, or ZIP64 metadata.

The archive is single-disk, has no archive comment or trailing bytes, reports
exactly two central-directory entries, and has internally consistent local and
central offsets/sizes. Validation expands the two bounded entries, rebuilds
the canonical archive, and requires byte-for-byte equality. A semantically
equivalent but differently encoded ZIP is not a valid `.wirt` package.

Canonical packaging is deterministic for fixed inputs. The package
fingerprint is SHA-256 over the complete canonical archive, rendered as
exactly 64 lowercase hexadecimal characters.

## Byte and expansion limits

| Item | Limit |
| --- | ---: |
| Complete `.wirt` archive | 65 MiB |
| Expanded `plugin.toml` | 64 KiB |
| Expanded `plugin.wasm` | 64 MiB |
| Expansion ratio for an entry at least 1 MiB | 1,000:1 |

Declared expanded sizes must match the bytes actually read. Size arithmetic,
central-directory arithmetic, and read limits are checked for overflow.

## Manifest contract

`plugin.toml` is UTF-8 TOML with these top-level tables:

```toml
[wirt]
abi = "0.1.0"

[plugin]
id = "example-plugin"
name = "Example Plugin"
version = "1.0.0"
author = "Example Author"
description = "Example description"

[capabilities]
network = false
network_domains = []
archive_metadata_read = false
archive_metadata_write = false
archive_modify = false
file_read = false
file_write = false

[rate_limits]
http_requests_per_minute = 10
```

Manifest bounds are part of package validation:

| Field | Limit |
| --- | ---: |
| Plugin ID | 64 ASCII bytes |
| Name | 128 bytes |
| Version | 64 bytes |
| Author | 256 bytes |
| Description | 16 KiB |
| Network domains | 64 entries |
| One network domain | 253 bytes |
| HTTP requests per minute | 600 |

Identity text is non-empty where required and contains no control characters.
The plugin ID is one portable filename component using `[A-Za-z0-9_-]` and is
compared case-insensitively for collision detection. Network domains are
canonical, unique hostnames; IP literals, URL syntax, ports, paths, wildcards,
and empty domain lists for a network-enabled plugin are rejected.

The manifest Wirt ABI must equal the component ABI and the host's exact ABI.
The guest's exported ID, name, version, author, and description must match the
manifest before initialization.

## Component preflight

The component is parsed and type-validated without Wasmtime compilation. Its
top-level contract is restricted to:

- Wirt `host`, `meta`, `rules`, and `ui` interfaces at `0.1.0`; a component
  may import only the canonical members it uses.
- The fixed WASI Preview 2 adapter interfaces required by Rust components:
  `io/poll`, `clocks/monotonic-clock`, `io/error`, `io/streams`, the CLI
  standard streams/environment/exit interfaces, and CLI terminal interfaces,
  all at `0.2.9`.
- The optional fixed-authority pair `clocks/wall-clock` and
  `random/insecure-seed`, both at `0.2.9`.
- Exactly six guest exports: `init`, `get-default-rules`, `get-ui-layout`,
  `on-ui-event`, `get-top-tabs`, and `get-metadata`.
- Exactly the canonical public Wirt types and structural signatures.

Unknown namespaces, sockets, filesystem interfaces, non-allowlisted WASI,
wrong member signatures, fresh resource identities, missing exports, and extra
exports are rejected.

Preflight itself is bounded:

| Component check | Limit |
| --- | ---: |
| Type-graph work items | 100,000 |
| Type-graph depth | 64 |
| Hashed identifier bytes | 64 KiB |
| Owned top-level name bytes | 64 KiB |

## Commands

```console
just wirt build plugins/my-plugin
just wirt validate plugins/my-plugin
just wirt package plugins/my-plugin
just wirt validate plugins/my-plugin/my-plugin-1.0.0.wirt
```

`wirt package` writes with create-new, same-directory temporary publication
and refuses to replace an existing output. `wirt validate` accepts only a Wirt
project or a `.wirt` file. The host repeats package validation during preview,
then requires the approved fingerprint to match during transactional install.
