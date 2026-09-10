# Arclain archive import

This archive-only package exposes `ArchiveClient`, `ArchiveSession`,
`WorkerOptions`, `Cancellation` and typed `ImportError`. The default feature is
client-only. Enable `worker` for `arclain-archive-worker` or call
`arclain_archive::worker::main()` from a product's trivial helper entrypoint.
It has no GUI, profile, database, plugin host or metadata service dependency.

`ArchiveClient::open` receives an explicit helper path and source, then returns
a complete physical `Unboxer::Inventory`. `ArchiveSession` implements Unboxer's
`SequentialArchive`: validate the whole inventory before writing, consume each
entry once, and require the final successful `None`. Keep the session alive
through consumption. Drop cancels, kills and reaps unfinished work. `take_error`
recovers the precise worker failure if a format-neutral stream reported I/O or
corruption; destination storage failures do not set that cause.

The worker accepts only the input source. It never receives destination paths.
Native ZIP, sevenz-rust2 and UnRAR engines perform payload decoding and CRCs;
Unboxer owns path, collision, size and destination validation. The existing
Arclain application decoder versions are independent of this package.

## Bounds and source identity

The memory default is half the machine's physical memory, clamped to 1 to
8 GiB; the worker accepts a cap from 32 MiB to 64 GiB and refuses anything
outside that range with exit status 2, before it reads anything else. There
is no total deadline. The other defaults are a 60-second stall timeout and a
1 MiB/s decode-rate floor: a worker that sends no frame for the timeout
while a request is outstanding is killed, as is one that streams entry data
below the floor over a window of the timeout, after a decode's first. Time
between entries and the source verification before COMPLETE owe no rate; the
worker's heartbeats keep them under the stall rule, and the worker writes a
HEARTBEAT frame after five silent seconds. OS limits are established before
reading the source request: a process memory Job Object on Windows,
RLIMIT_AS on Linux. The helper disables its own crash dialogs/core dumps.
Frames are at most 64 KiB, the reader queue holds two frames, metadata is
capped at 64 MiB and physical count at 200,000; caller limits can be lower.
Unboxer also charges retained destination and path metadata against its
index budget. These controls bound decoder resource use; they are not a
general security sandbox. Allocation termination may report WorkerFailed
because the OS does not reliably distinguish it from another native crash.

Windows opens the source with a share mode that refuses every other writer
and deleter for as long as the handle is held, so it does not hash: length
and modification time are checked at open and again before COMPLETE. Linux
cannot keep a writer out, so it hashes the whole source with SHA-256 at open
and again before COMPLETE and reports a difference as the source having
changed; that is two full read passes. Linux uses the held `/proc/self/fd`
inode for UnRAR, including after pathname replacement, and does not claim
immunity to hostile in-place writes followed by restoration.

## Supported formats

- ZIP stored/deflate, including strict standard ZIP64 framing. Complete central
  identities and matching local names/flags/sizes/offsets are attested before
  native indexing, which can otherwise collapse duplicate names. Split files,
  ambiguous ZIP64 extra values and unsupported methods are refused.
- 7z data streams use strict upstream UTF-16 names, physical entry identities
  and actual packed blocks. Solid blocks decode once. A compact data-only
  decoder view handles interleaved streamless files without losing their
  original inventory positions. BCJ2 and LZMA2 with a 256 MiB dictionary have
  fixture coverage. Passwords, unknown methods, anti-items and links are refused.
- RAR5 accepts strict UTF-8 original names with known Windows/Unix host fields;
  RAR4 initially accepts ASCII names with DOS/Windows/Unix host schemes and
  modern file headers. Narrow bounded header attestation proves framing,
  header CRCs, names, kind/attributes and physical counts against native listing.
  It does not decode payloads. Encryption, volumes, service/extra records,
  unproven host/name semantics and native rewriting/truncation are refused.

RAR callbacks stream RAR_TEST data directly in bounded chunks; no whole-entry
buffer or extraction-to-directory API is used. All backends require exact
declared stream lengths and explicit worker completion before success.

## Source integration

The new Unboxer serde/sequential API is still unpublished alongside this package.
Development uses an explicit Cargo source patch to the matching Crateyard
checkout. Its old pinned revision alone does not contain those new features.
Publish reachable upstream changes before claiming a fresh checkout is ready.

Run package tests with `cargo test -p arclain_archive --features worker` and the
appropriate development patch configuration. `tests/client_faults.rs` is a
test-only helper simulator and is never a normal or bundled binary target.
The package inherits Arclain's GPL-3.0-or-later license. Bundlers must retain
the native UnRAR redistribution text and dependency notices.
