# Game days

The scheduled drills of blueprint G3 §25 (R12 RES-10186 and the RES drill
rows), run with `scripts/game-day/harness.mjs` against the platform's own
binaries, with every result recorded under `results/` in a committed format.
Labels as in [the operator runbook](../RUNBOOK.md): **ran**, **repo**,
**not run**.

## What a game day is here

A fault injected on purpose into running `qip-fabricd`, `qip-ledgerd` and
`qip-edge-node` processes, with the recovery timed and the data loss counted.
The harness starts each process from its scenario file with nothing
inherited from the shell, waits for its readiness probe, proves the steady
state, injects one fault, checks what must stay true through the hold,
applies the recovery, and records:

| Field | Meaning |
|---|---|
| `detected_ms` | fault → the first probe that sees it |
| `rto_ms` | recovery action → every process ready and every after-recovery check holding. **This is the number a runbook RTO is compared with.** |
| `outage_ms` | fault → that same moment |
| `rpo_records` | records the recovered system holds fewer of than it had acknowledged before the fault, read the moment the recovered process first answers. Null when the scenario names no durable position, or when that position was zero before the fault (nothing to lose proves nothing) |
| `verdict` | `met`, `missed`, `inconclusive` (the steady state never held, or RPO was vacuous), `aborted` |

`node scripts/game-day/harness.mjs validate docs/ops/game-days/results/*.json`
refuses a result whose `met` the record does not support — a failed check,
a missing measurement, or one over its objective — and
`scripts/game-day/harness.test.mjs` runs that validation over every committed
result in CI's `scripts` job.

Every drill is paper, local and simulated. The harness refuses a scenario
that sets a venue adapter, an autonomy level, a cloud endpoint or the venue
order-entry acknowledgement.

## The four scenarios

| Scenario | Fault | What must hold | Objectives |
|---|---|---|---|
| `broker-loss` | SIGKILL `qip-fabricd` under a running cell | the cell keeps running passes and does not halt; the broker restarts from its data directory holding every journal record it acknowledged | RTO 10 s, RPO 0 |
| `ledgerd-loss` | SIGKILL `qip-ledgerd` | the cell keeps running passes; the broker keeps serving without its consumer; the ledger restarts | RTO 10 s, RPO 0 on the outcomes stream |
| `region-isolation` | SIGSTOP `qip-fabricd` — the link to the centre hangs rather than refuses | the cell keeps deciding locally and does not halt; the broker answers again on SIGCONT with what it had | RTO 10 s, RPO 0 |
| `stale-policy` | the cell's polled control wire (the halt flag's directory) vanishes, as a lost mount would | the cell treats the stale wire as engaged and halts (§46.2), and runs again once the wire reads | RTO 10 s |

## Running one

```
cd backend && cargo build -p qip-fabricd -p qip-ledgerd -p qip-edge-node
node scripts/game-day/harness.mjs run \
  --scenario scripts/game-day/scenarios/broker-loss.json \
  --bin backend/target/debug --out docs/ops/game-days/results
```

Pass `--commit <sha>` when the binaries were built from a commit other than
the checkout's HEAD. The process logs are kept in the scratch directory the
last line names.

## Results so far

**Ran** 2026-10-11, against debug builds of the three binaries from
`b6ee74ad` (the backend tree this lane did not change), on one Linux host:

| Scenario | Verdict | `rto_ms` | `rpo_records` | Note |
|---|---|---|---|---|
| `broker-loss` | met | 115 | 0 | journal high watermark 1 before and after |
| `ledgerd-loss` | **inconclusive** | 16 | null | the outcomes stream was empty: the simulated cell deploys no strategy without `QIP_STRATEGY_PLAN_PATH`, so no outcome existed to lose. The restart is timed; resumption is not demonstrated |
| `region-isolation` | met | 10 | 0 | the pause was detected at ~2 s, the probe's own timeout |
| `stale-policy` | met | 845 | — | under one pass interval plus the poller's bound |

The files are `results/2026-10-11-*.json`. These are a **local** baseline:
single host, loopback, debug builds, one partition. They say the recovery
logic works and how long it takes on a laptop; they say nothing about a
deployed node, and no runbook may quote them as a production RTO.

## Schedule

| Cadence | Drill | Where | State |
|---|---|---|---|
| Every change to `qip-fabricd`, `qip-ledgerd` or `qip-edge-node`'s fabric, spool or control code | all four scenarios, locally | the author's machine, results committed with the change | **repo** — not enforced by CI, because CI builds no binaries in the `scripts` job |
| Weekly | all four scenarios, locally, on `main` | an operator, results committed | **not run** beyond 2026-10-11 |
| Monthly (§25 zonal failure, RES-10152) | standby promotion with a zone lost | a deployed dev node pair | **not run**: no node is deployed (`execution_nodes_enabled = false`). Standby fencing is built in part: `qip-edge-node`'s `standby.rs` promotes only when the holder's lease has lapsed by a margin, every venue reports the old session ended and its epoch swap lands, and `modules/execution-node` refuses a standby with no `fencing_lease_path` and writes the lease, role and name on both machines. What is not built is a cross-zone lease store: the binary's only store is a file, correct on one host, so a pair in two zones is not yet fenced and this drill has nothing honest to run against |
| Quarterly (§25 regional loss, RES-10151) | a region lost; healthy regions continue | a multi-region deployment | **not run**: one region is authorised (ADR 0035), and fabric mirroring is refused |
| Quarterly | rebuild from zero (RES-10158) | a scratch project | **not run** |

The local drills are the part this repository can run today. The deployed
rows wait on the deployment they name, and each says what it waits on rather
than being left off the table.

## What is not drilled, and why

- **The fabric-published policy going stale.** The cell's `VerifiedPolicy`
  arrives on `control.local` from a release controller, and no local binary
  publishes one; `stale-policy` drills the polled control wire, which is the
  half the cell reads from its own disk.
- **Replication, leader movement, quorum loss** (RES-7112, 9168, 9169): by
  default the broker is a single writer under ADR 0100 with nothing to fail
  over to. With `QIP_EVENT_FABRIC_MANAGED_LOG=spanner` (ADR 0111) two or three
  instances per region contend for each partition's Spanner lease and the
  others follow the committed log, and `modules/event-fabric-broker` admits
  two or three addresses only beside that managed log. That is failover in
  code and in configuration, not in a drill: no Spanner database is applied,
  no broker is deployed, and the four local scenarios above run the
  single-broker path.
- **Venue loss and the feed playbook** (RES-10132): the simulated venue is
  in-process and cannot be lost separately from the cell.
