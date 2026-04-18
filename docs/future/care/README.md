# CARE — Deferred / R&D

These specs describe an ambitious archive format (CARE) designed from scratch:
block-level Reed-Solomon, cross-file deduplication, solid compression across
file boundaries, per-block algorithm selection, platform-specific metadata
layers, AEAD encryption, Ed25519 signing, formal verification via Kani/Verus.

**Status: not on the roadmap.**

The cost-benefit for a sole-developer project didn't add up: every shipped
archive locks us into the format forever, and no other tool on the planet
reads `.care`. Users extracting on a phone, on a friend's PC, or with whatever
they have pre-installed would be stuck.

Instead, the shipping archive feature is built around ZIP + zstd + a PAR2
recovery tail — see [`docs/ARCZIP_FORMAT.md`](../../ARCZIP_FORMAT.md). That
captures ~90% of CARE's user-facing value (self-healing, modern compression,
per-entry metadata) at ~5% of the maintenance cost, and stays universally
readable.

These CARE docs remain here as the research record. Pick them back up only
if:

- A clear product reason makes owning a format worth the lifetime tax.
- Cross-file dedup or solid-across-files compression becomes a must-have
  differentiator — those are the two things ZIP genuinely can't do.
- Resources exist to maintain a real library, CLI, tests, fuzzing,
  verification, and ecosystem docs long-term.

Until then: ship ARCZIP.
