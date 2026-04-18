# ARCZIP — ZIP with self-healing

> **It's still ZIP.** That's the point.

## The one-line pitch

Standard ZIP on disk, readable by every tool you already have, with a PAR2
recovery tail appended after the EOCD. Arclain recognizes its own and surfaces
verify/repair. Nobody else needs to know the difference.

---

## File layout

```
┌──────────────────────────────────────────────────────┐
│  ZIP body                                            │
│    Local file headers + compressed data              │
│    (zstd by default, deflate in max-compat mode)     │
│    Standard ZIP extra fields for platform metadata   │
│    Optional .arclain/ directory for app metadata     │
│  Central directory                                   │
│  EOCD (PK\x05\x06)                                   │
│    Comment: "ARCZIP/1\0" + reserved bytes            │
├──────────────────────────────────────────────────────┤
│  PAR2 recovery packets                               │  ← starts immediately
│    Standard .par2 packet stream                      │    after EOCD
│    Covers ZIP body from offset 0 through EOCD        │
├──────────────────────────────────────────────────────┤
│  Optional Ed25519 signature packet (v2+)             │
│    32-byte magic "ARCZIP-SIG\0" + 64-byte signature  │
└──────────────────────────────────────────────────────┘
```

Tools without PAR2 awareness scan backwards from EOF, find the EOCD, and stop.
Everything after EOCD is ignored and the ZIP opens cleanly.

Arclain scans from EOF, finds the EOCD, checks the comment for `ARCZIP/1`,
then scans forward past the EOCD for PAR2 packet magic (`PAR2\0PKT`) to
locate recovery data.

---

## Compression

Each entry has its own method (ZIP already allows this):

| Mode | Method | When used |
|------|--------|-----------|
| Default | zstd (method 93), level 19 | Recent archivers (7-Zip 22+, WinRAR 7+) |
| Max compatibility | Deflate (method 8), level 6 | Opt-in flag; opens in Explorer / Finder |
| Stored | Stored (method 0) | Incompressible content (already-compressed files) |

zstd produces archives ~6-10% larger than 7z for typical mod content but keeps
random access, per-entry compression swap, and the EOCD anchor for our
recovery trick. The max-compat mode sacrifices ~10% size for literal
open-with-anything portability.

Per-entry compression detection: files with extensions in `INCOMPRESSIBLE_EXTENSIONS`
(jpg, mp4, zip, 7z, etc.) go straight to `Stored` — compressing already-compressed
data wastes CPU for zero benefit.

---

## Recovery

Appended after EOCD as a standard PAR2 packet stream:

- **Coverage:** byte 0 through end of EOCD.
- **Redundancy:** configurable — Minimal (5%), Balanced (10%, default), Paranoid (20%).
- **Integrity:** PAR2's per-slice MD5 hashes detect any bit-rot. Sufficient for
  damage detection; no separate Blake3 layer needed.
- **Tooling:** external `par2` binaries (par2cmdline, MultiPar, QuickPar)
  operate on these bytes directly if the user extracts the tail as a standalone
  `.par2` file. Arclain ships an embedded PAR2 implementation so users don't
  need an external install.
- **Self-healing:** PAR2 packets internally duplicate critical fields, so
  the recovery section is itself resilient to some damage.

### What arclain does on open

1. Read last ~10 KB → locate EOCD by magic scan.
2. Check EOCD comment starts with `ARCZIP/1\0` → this is one of ours.
3. Scan forward from end-of-EOCD for `PAR2\0PKT` → PAR2 tail begins there.
4. Run PAR2 verify in background → silent OK, or flag "damaged, repair available".
5. If the ZIP body's EOCD itself is damaged, arclain can't locate the central
   directory. Fallback: tail position known to be last ~N% of the file; read
   tail first, run PAR2 repair, then reparse the repaired zip.

---

## Metadata

Two channels:

1. **Standard ZIP extra fields** — for per-file platform metadata.
   Already-supported extensions arclain writes and reads:
   - `0x5455` (UT) — Unix timestamps (mtime/atime/ctime)
   - `0x7875` (ux) — Unix uid/gid
   - `0x000A` (NTFS) — Windows FILETIME triple
   - `0x4453` (in-flight Windows ACL, if we add it later)

2. **`.arclain/` directory inside the ZIP** — arclain-specific blobs, stored
   as regular ZIP entries so they roundtrip through any zip tool untouched:
   - `.arclain/manifest.json` — archive version stamp, creator, preset used
   - `.arclain/metadata.json` — gameta DLsite codes, tags, user metadata
   - `.arclain/presets/*.json` — arclain pipeline presets bundled with the archive

Other zip tools list these as normal files. Arclain reads and hides them from
the user-facing file list.

---

## EOCD comment marker

The EOCD comment field (up to 64 KB of free-form bytes) carries the format
identifier:

```
bytes 0..8    : "ARCZIP/1" (ASCII, no trailing null)
byte  8       : 0x00 (separator)
byte  9       : flags
                  bit 0: has PAR2 tail
                  bit 1: has signature
                  bit 2: reserved
                  bit 3: reserved
                  bits 4-7: reserved
bytes 10..14  : PAR2 tail offset from EOF (u32 little-endian, 0 if no tail)
bytes 14..16  : reserved (zero)
```

Total: 16 bytes. Any tool that shows the EOCD comment will render something
like `ARCZIP/1\x00\x01...` — weird but harmless. Arclain recognizes the magic
and parses the flags.

---

## What's out of scope

These stay in `docs/future/care/` as R&D. Doing them would break ZIP
compatibility:

- **Cross-file deduplication** — would require custom content-store layer
  referencing chunks by hash. Can't be represented as standard ZIP entries.
- **Solid compression across file boundaries** — ZIP compresses each entry
  independently. Solid mode requires a different container.
- **Unified merkle tree across files** — redundant with PAR2's per-slice MD5.
- **Encryption** — ZIP's native AES-256 is available if we ever need it, but
  for now prefer users encrypt the whole archive externally (age, gpg).

---

## Implementation shape

Plan for the Rust side (not binding):

- New crate `crates/arczip` or module under `crates/core/src/formats/arczip`.
- Writer: wraps the `zip` crate's `ZipWriter`, stamps EOCD comment with marker,
  appends PAR2 packets (generated via `par2-rs` or similar).
- Reader: wraps `zip` crate's `ZipArchive`, detects marker, exposes
  `verify()` / `repair()` methods on top of standard read.
- Integrated as an output format in the pipeline (alongside zip/7z).
- CLI flag for max-compat mode (Deflate instead of zstd).

Rough scope: 1-2 weeks for v1, most of which is the PAR2 reader/writer
(standard format, well-documented).

---

## Versioning

| Magic | Status |
|-------|--------|
| `ARCZIP/1` | Initial: zstd + PAR2 tail + arclain metadata |
| `ARCZIP/2` | Planned: add signature support |

Forward compatibility: readers that don't recognize the flags or the
signature packet MUST still read the zip body correctly. Unknown flags =
ignored. Unknown trailing packets after PAR2 = skipped.

---

## Summary

| | Value |
|---|---|
| Extension | `.zip` (not a new extension) |
| Magic | EOCD comment prefixed with `ARCZIP/1` |
| Compression | zstd default, deflate max-compat |
| Recovery | PAR2 packets after EOCD |
| Integrity | PAR2 per-slice MD5 |
| Metadata | ZIP extra fields + `.arclain/` directory |
| Other tools | Open it as plain zip; ignore tail |
| Arclain | Recognizes marker, offers verify/repair |
| Maintenance tax | Low — leans on existing standards |

The whole bet: stand on the shoulders of ZIP, zstd, and PAR2 — three
battle-tested standards — and add the smallest possible arclain-specific
layer to glue them together.
