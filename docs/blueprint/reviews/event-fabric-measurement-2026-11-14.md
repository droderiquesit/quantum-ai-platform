# Event fabric measurement note, R12-FABRIC-7045 (2026-11-14)

Read on main (worktree HEAD ae810895). Nothing was built or run. Every claim cites a file or a command.

## What the decision asks

Row R12-FABRIC-7045 (docs/blueprint/assessment/R12-M7.json:1032) covers async subscriptions and OTel hooks in the Rust SDK. The requirement is at docs/blueprint/requirements/R12-M7.json:1135.

Owner decision 36 in docs/blueprint/owner-decisions-2026-10-31.md restates the O09 ruling (line ~1660): O09 is declined. No OTel exporter crate (ADR 0026 stands). No tokio on the fabric SDK path until a committed measurement shows it is needed. Sync producer/consumer and GroupCoordinator stay. The path is reached by the edge node drain. "Next: Write the measurement note; only then an ADR for async subscriptions and OTel hooks."

So the note must show a committed measurement that the thread-backed subscription falls short. The row's notes field says the same: "until a committed measurement shows the thread-backed subscription falls short."

The bar is also in ADR 0109 condition 2 (docs/adr/0109-*.md:108-115): tokio is refused in the decision core and the Cell::work path "unless a committed measurement shows it helps there."

## What the tree measures today

1. A harness exists, with no recorded output.
   - backend/crates/apps/qip-fabricd/src/bench.rs:1-12 measures produce and fetch latency percentiles and throughput on one broker over loopback, in-memory archive. Its own text says the numbers are "evidence about this code path and nothing wider."
   - backend/crates/apps/qip-fabricd/benches/fabric.rs:1-4 is the entry point (`cargo bench -p qip-fabricd --bench fabric`). Defaults 2000 batches, 4 records, 256 bytes (fabric.rs:21).
   - Row R12-FABRIC-7043 (traceability-matrix.md:216) lists its verification as "bench run output". Check: `grep -rn "p50_us\|produce  n=\|fetch    n=" docs backend --include=*.md --include=*.json --include=*.txt` found no committed run output.
2. The harness test asserts shape only. backend/crates/apps/qip-fabricd/tests/bench.rs:24-35 checks sample counts (24), percentile ordering, max > 0 and throughput > 0.0. No latency or throughput threshold.
3. The only committed numeric thresholds in this area are loose ceilings on the storage engine, not the fabric SDK. backend/crates/libs/qip-storage/tests/fabric_bench.rs: p99 < 100 ms for synchronous writes (lines 37-40 of the sampled range), p99 < 50 ms buffered, p50 < 50 ms for both modes. These time DurableStore::put, not subscribe or fetch.
4. backend/crates/libs/qip-transport/tests/event_fabric_batcher_throughput.rs:129-131 states throughput is not asserted. It checks deterministic batch cuts only.
5. No file under docs/ops records a fabric figure. `grep -n -i "fabric\|subscri" docs/ops/execution-measurements.md` returned nothing.

## The blocking design's stated limits

- Consumer::subscribe (backend/crates/libs/qip-transport/src/event_fabric/consumer.rs:490-525) spawns one std thread per subscription. It feeds a bounded mpsc::sync_channel. The caller blocks only on recv. An empty fetch sleeps `idle_poll`.
- The module doc (consumer.rs:25-44) states that dropping a Subscription does not join the thread. The thread exits on its next send failure.
- Stated cost: one OS thread per subscription, fixed poll sleep when idle. No doc states a subscription count at which this becomes a problem.

## Callers

- `grep -rn "\.subscribe(" backend/crates --include=*.rs` outside qip-transport/src shows only tests (qip-transport/tests/event_fabric_client.rs:391) and the unrelated in-process bus (qip-events/src/bus.rs). No production code calls Consumer::subscribe.
- The edge node drain (backend/crates/apps/qip-edge-node/src/event_fabric/drain.rs:87,375-394) uses Producer on its own std thread. It does not subscribe.
- tokio is absent from the Cargo.toml of qip-transport, qip-fabricd and qip-edge-node (grep returned no lines).

## What is unmeasured

- Any recorded produce, fetch or end-to-end latency percentile for the SDK.
- Subscription delivery latency, including the idle_poll floor.
- Cost of N concurrent subscriptions (threads, memory, scheduling) at the number a deployment would hold.
- Drain-thread behaviour under a slow or stalled subscriber.
- Any comparison of thread-backed and async subscription. No async variant exists to compare.
- Any OTel hook overhead. ADR 0026 chose Prometheus exposition and no hook exists.
- Any need: no consumer in the tree uses subscribe in production, so there is no workload that falls short.

## Conclusion

An ADR for async subscriptions is not justified by a committed measurement today. The decision's condition is a measurement that the thread-backed subscription falls short. The tree holds a harness with no recorded output, shape-only assertions, and no production subscriber. A shortfall cannot be shown from that.

R12-FABRIC-7045 does not qualify to be built now. The OTel half is declined outright by the ruling, not waiting on a number.

The measurement that would justify an ADR:
1. A production consumer of the SDK exists, with a stated subscription count and delivery-latency target.
2. Run `cargo bench -p qip-fabricd --bench fabric` and commit its output (bench run output, which row 7043 already names as its verification).
3. Add a subscribe-specific bench: N concurrent Consumer::subscribe at that count, with delivery latency percentiles and thread and memory cost, on the target host class.
4. Show the thread-backed result misses the target. Only then write the ADR, and it must still pass the ADR 0109 tier test that keeps tokio off Cell::work.
