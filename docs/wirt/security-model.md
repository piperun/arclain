# Wirt security model

Wirt reduces plugin authority through a validated component contract,
capability-gated host calls, hard resource quotas, and fingerprint-scoped
quarantine. It is defense in depth, not an antivirus product and not a claim
that arbitrary third-party code is harmless.

## Execution model

Plugins currently run as WebAssembly Components inside the Arclain process,
in separate Wasmtime stores. They do not run in a separate operating-system
process. In-process execution keeps the typed host interface small and avoids
an IPC translation layer, but it makes validation, Wasmtime isolation, host
API design, and quotas part of the security boundary.

The executor uses final, validated request/response wrappers. Calls are bound
to the exact live plugin generation and coordinated with enable, disable,
reload, and unload so a stale instance cannot inherit a replacement's
authority or execute after cleanup.

## What a guest starts with

The WASI context is fixed-authority:

- no preopened directories;
- no inherited environment variables or command-line arguments;
- stdin is closed and stdout/stderr are discarded;
- wall-clock time is fixed at the Unix epoch;
- secure random, insecure random, and the insecure seed are deterministic;
- the real monotonic clock remains available for bounded scheduling and
  deadline support.

Component preflight rejects filesystem, socket, and other non-allowlisted WASI
imports. A plugin therefore cannot open an arbitrary host path, start a raw
socket, inspect the user's environment, or write to the terminal through WASI.

## Capability boundary

All useful product access crosses Wirt host functions. The manifest requests
capabilities, the install dialog shows them, and the host enforces them on each
operation:

- Network requests require `network`, an exact approved domain, and a
  rate-limit permit.
- Archive reads, metadata writes, and archive changes have separate grants.
- Data/cache reads and private writes have separate `file_read`/`file_write`
  grants.
- Cache keys and temporary files are scoped to the calling plugin.
- Event authority is bound to the admitted plugin generation through the
  complete guest-and-host-effect phase.

Capabilities are not ambient filesystem handles. For example, `file_write`
can create bounded files only in private plugin temporary storage; a filename
argument is not a host path.

## Package and install boundary

Only canonical `.wirt` packages enter the public install flow. Before showing
approval, the host verifies archive encoding, byte limits, manifest policy,
the exact component type graph, ABI, guest/manifest identity, and SHA-256
fingerprint. Installation then reopens and revalidates the selected package,
requires the approved fingerprint, initializes in staging, and publishes
transactionally without replacing an existing identity.

Package inputs and staging paths reject links/reparse points and preserve
handle-relative roots where supported. Failures roll back staged files and do
not register a partial plugin.

## Resource containment

Important runtime ceilings include:

- 10,000,000 fuel units per export;
- an approximately five-minute epoch dead-man deadline (30,000 ticks at
  10 ms) for guest liveness, while host calls retain their own timeouts;
- 8 MiB hostcall-copy fuel;
- 256 MiB linear memory, four memories, eight tables, 100,000 table elements,
  and 32 adapter-internal core instances;
- 1 MiB serialized executor messages;
- 10,000 UI work items, 1,024 actions, and 10,000 top tabs;
- bounded settings, logs, metadata pages/writes, cache bodies, and private
  temporary storage.

A quota-shaped failure marks the instance terminal and disables it. The
quarantine ledger is keyed by the package fingerprint. An explicit retry can
replace the instance; after three failed retries the exact package is
persistently disabled until Reset. Changing unrelated files or restarting the
application does not silently clear that decision.

## Malware and cryptomining expectations

Wirt does not scan signatures, classify malware families, or promise ESET-like
detection. It instead removes the usual authority a commodity miner needs:
arbitrary sockets, background process creation, arbitrary files, inherited
secrets, unbounded CPU, and silent persistence. Fuel, deadlines, lifecycle
admission, and quarantine bound abusive computation and stop repeated retries.

That containment is not proof of benign intent. A plugin can still compute
inside its allowed budget, misuse legitimately granted data, exploit a bug in
the runtime/host, or socially engineer the user into approving broad access.
Package provenance and human review remain important.

## Marketplace gate

There is no general promise that arbitrary marketplace packages are safe. A
marketplace must not open until a restricted `wirt-host` gate exists outside
the product process to perform canonical validation, identity/provenance
checks, policy review, and adversarial execution/evaluation with no product
secrets or user data. Signing, reputation, revocation, update policy, and
incident response also belong at that distribution layer.

Passing that gate would complement the runtime boundary; it would not replace
capability review or turn Wirt into antivirus software.

## Reporting a boundary issue

Treat any ability to bypass package preflight, obtain undeclared authority,
cross a plugin namespace, execute after disable/unload, escape a quota, or
alter transactional install inputs as a security bug. A regression must prove
the bypass before the fix and remain in the relevant Wirt/plugin test suite.
