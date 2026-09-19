# ADR 0082: `unsafe` stays forbidden, and core pinning is retired until a deployed node measures the need

- **Status**: Accepted, under the authority the owner delegated to the
  policy lane on 2026-09-19 over legal, business and policy decisions.
- **Date**: 2026-09-19
- **Supersedes**: nothing
- **Related**: ADR 0002 and ADR 0009 (two dependencies), ADR 0012 (where a
  library earns its place), ADR 0024 (one Compute Engine execution node per
  region), ADR 0035 (one node, shadow mode, accepted and not applied), ADR
  0043 (the three gaps no in-tree code may close — the same shape of
  argument, applied to a syscall), ADR 0069 (capability nothing consumes is
  not declared)

## Context

Blueprint §41.3 assigns sixteen threads to named cores: cores 0–1 for the
OS, telemetry drainer, control-plane client and plan compiler; cores 2–15
isolated, one busy-poll receive slot per venue group, the book engine, the
strategy executor, the quote engine, the risk gate, the leg coordinator that
"owns the timer wheel", inference and the adversary monitor. §41.4 backs it
with `isolcpus` for cores 2–15, a performance governor and disabled C-states.

Pinning a thread to a core on Linux is `sched_setaffinity(2)`. Reaching it
from Rust means either an `unsafe extern "C"` declaration with a hand-written
`cpu_set_t`, or a crate — `libc`, `nix`, `core_affinity` — that carries the
declaration for you. The workspace forbids the first
(`grep -n unsafe_code backend/Cargo.toml` prints `unsafe_code = "forbid"`)
and the dependency policy forbids the second (ADR 0002, ADR 0009;
`scripts/check-dependencies.sh` permits serde's closure and nothing else).

The register row for §41.3 says half of it exists in Terraform and the table
is absent from every binary. Every premise of that row was re-run in this
worktree on 2026-09-19:

- `grep -n isolated_cpus infrastructure/terraform/modules/execution-node/main.tf`
  derives `2-${local.vcpus - 1}` from the machine-type allowlist, and
  `grep -n 'isolcpus=' infrastructure/terraform/modules/execution-node/templates/startup.sh.tftpl`
  refuses to start a node whose kernel command line lacks it.
- `grep -rn sched_setaffinity backend/crates` and
  `grep -rn core_affinity backend/crates` each return nothing.
- **`qip-edge-node` has one thread.** `grep -n 'thread::spawn' backend/crates/apps/qip-edge-node/src/main.rs`
  prints one line, and it is inside `mod tests`, in a test whose own doc
  comment names the design: "the one thread this node polls its halt flag,
  ships its journal, talks to the centre and runs `Cell::work` on". The
  binary's serve loop turns once per health probe (`grep -n 'listener.incoming()' backend/crates/apps/qip-edge-node/src/main.rs`)
  and has, in its own words, "no scheduler".
- **No node exists.** `grep -h execution_nodes infrastructure/environments/*/terraform.tfvars`
  prints `execution_nodes = {}` four times. The boot image has never been
  baked (ADR 0035's correction). Nothing this record could pin has ever run.

### The consequence nobody had written down

`isolcpus` removes the named cores from the kernel's general scheduler. A
process that does not set its own affinity is scheduled on the cores that
remain — here, cores 0 and 1. So the Terraform half of §41.3, applied to the
binary as built, produces a machine on which the execution node's one thread
runs on a housekeeping core beside the OS, the telemetry drainer and the
egress proxy, while six to twenty isolated cores idle. The startup script's
refusal message says "a node whose hot path shares cores with the OS is not
the machine this architecture costs money for", and on the binary as built
the check admits exactly that machine and refuses the one that would have
worked by accident.

That is not an argument for pinning. It is an argument that the two halves of
§41.3 were scored as halves of one thing when they are not: the kernel
parameter is capability with no consumer, in ADR 0069's sense, and the
consumer would cost a workspace-wide guarantee.

## Decision

1. **`unsafe_code = "forbid"` stays, workspace-wide, with no per-crate or
   per-file exception.** The guarantee is one line in one file and every
   reader can check it in one command; a `forbid` with an allow-list is a
   `deny` with a list to maintain, and the first entry on the list is the
   one the next lane cites as precedent. No `libc`, `nix` or `core_affinity`
   crate is admitted; ADR 0012's three-part test is not met, because the
   problem is not specialist — it is one syscall — and the thing that makes
   the syscall dangerous is the hand-declared ABI struct around it, which is
   precisely the "reviewed by nobody who writes this for a living" class ADR
   0009 refuses.

2. **§41.3's thread-and-core table is retired as a non-goal for
   `qip-edge-node` as built.** The table describes a sixteen-thread process.
   This binary is a one-thread process by design, and the design is
   load-bearing: the cell is handed `now` and reads no clock, every pass runs
   on the thread that polls the halt flag so a halted node cannot send by
   any path, and the health server, the journal flush and the mesh exchange
   share that thread deliberately. A table that assigns threads the process
   does not have is not half-met by a kernel parameter; it is a description
   of a different process.

3. **If a thread is ever pinned, the supervisor pins it, not the binary.**
   `CPUAffinity=` in `qip-execution-node.service` pins the process from
   outside with no `unsafe`, no dependency and no code change, and for a
   one-thread process a process-level pin *is* the per-thread pin. This is
   decided now so that the first lane to measure a need does not reach for
   the crate. Per-thread pinning from inside the binary is not reachable
   under decision 1 and would be its own ADR, argued from a second thread
   whose core requirement differs from the first's.

4. **The Terraform half is not changed by this record**, and the finding
   above is handed to the infrastructure owner rather than fixed here,
   because either remedy is a plan-proven change under ADR 0069: either
   `qip-execution-node.service` gains `CPUAffinity=` naming the first
   isolated core, so that the `isolcpus` refusal guards something, or the
   shadow node's `isolated_cpus` is narrowed so that a machine is not
   provisioned with cores the binary cannot reach. Until one of those lands,
   the register says which cores the one thread would actually run on.

## Consequences

- §41.3 stays `PARTIAL` in the register: the Terraform half is present and
  the table half is retired, not absent, and the row says why. A retired
  clause is not `REACHED`, on the same reasoning ADR 0069 used for the three
  §45.1 rows it refused to declare.
- §41.4's "isolcpus for cores 2–15" is unaffected in code and now carries a
  known consequence in prose: nothing consumes it.
- No file under `backend/`, `frontend/` or `infrastructure/` changes.
- The observability domain's existing note — the edge-node health server is
  single-threaded, so a scrape is rendered on the thread that flushes the
  journal — is the same fact seen from the other side, and stays true.

## What it costs

**Latency the blueprint promises is not available.** §41.3 exists because a
busy-poll receive slot, a book engine and a risk gate on isolated cores have a
tail latency that a shared core cannot promise. §30.2 budgets path 1 at 2–6
milliseconds and path 2 at 5–15. This record does not claim those budgets are
met; it says they cannot be measured on a process nobody has run, and that a
guarantee the whole workspace holds is not spent on a figure nobody can check.

**A provisioned node would idle most of its cores.** On the machine shape
ADR 0035 chose, decision 4's finding means paying for isolated cores the
binary never touches until the unit file pins it or the isolation is
narrowed. That is stated so it is not discovered on the first invoice.

**The one thread does everything, and this record makes that permanent for
now.** A slow health probe, a slow journal flush and a slow pass all contend
on one core. The `bound` test in `main.rs` exists because that was already
true and already bit once.

**The reversal condition needs a scrape that does not exist.** Decision 2 is
reversed by a measurement, and the observability rules record that nothing
has been shown to scrape any process. So the condition below cannot fire
before the collection gap closes, and a reader should not take "measured to
need it" as a bar that will be met soon.

## What would make this wrong

- **A deployed node measured to need it.** Specifically: a pass-duration or
  fill-time series scraped from a running `qip-edge-node` (the recording
  sites exist; the ingestion does not) showing tail latency attributable to
  preemption on cores 0–1 rather than to the pass's own arithmetic — with
  the attribution shown, not asserted, because a slow pass has several
  causes and pinning cures one. Then decision 3's route: `CPUAffinity=` in
  the unit, proven by a harness, the measurement repeated. Not a crate.
- **A second thread with a different core requirement.** If a real venue
  gateway ever needs its own receive thread (ADR 0084 describes where a
  release thread would sit), one process-level pin no longer expresses the
  intent, and per-thread pinning becomes a live question. It is answered by
  a new ADR that argues the `unsafe` exception on its merits, names the one
  file it lands in, and shows the acceptance test that holds every other
  file to `forbid`.
- **`std` gaining a safe affinity API.** The standard library does not
  expose `sched_setaffinity` today. If it does, decision 1 is untouched and
  decision 3 is revisited, because the cost this record refuses would have
  gone.
- **The infrastructure owner narrowing `isolated_cpus` to nothing.** Then
  decision 4's finding is resolved in the other direction and §41.4's
  isolation row should be re-scored as retired too, rather than left
  reading as a promise.

## Alternatives considered

**A narrow `unsafe` allowance in one apps crate for one `extern "C"`
declaration.** Rejected. The workspace's `forbid` is asserted by the
acceptance suite and cited by §56.1 rule 6 as holding with "no
reviewed-primitive exception at all"; the first exception changes what that
sentence means everywhere. The declaration itself — `cpu_set_t` is a
1024-bit mask whose layout is glibc's, not the kernel's — is a hand-copied
ABI that a wrong constant turns into a silent no-op, which is the failure
mode this platform refuses in every other domain: a control that reads as
protection and cannot fire.

**`core_affinity` or `libc` under ADR 0009's edge tier.** Rejected. The tier
exists for clients guarding real money where the in-tree alternative would be
reviewed by nobody; a syscall wrapper is not that, and the tier test in
`architecture.rs` holds every crate to two until the first client lands.

**`taskset` from the startup script.** A process-level pin, like decision 3,
but a shell command racing the unit's start rather than a unit directive the
supervisor applies before `exec`. Rejected in favour of `CPUAffinity=`, which
is the same effect with no race and no second place to look.

**Declare the table met by the kernel parameter.** Rejected: that is the
scoring this record exists to correct.

## Dependency-direction argument

No crate is added and no edge moves. `qip-edge-node` stays an `apps/` crate
depending inward on `qip-edge` and the libs; nothing depends on it. The only
place this record permits an affinity to be set is a systemd unit rendered
by Terraform, which is outside the Cargo graph and below the binary rather
than inside it.
