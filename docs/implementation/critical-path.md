# Critical path to the first working system (M5)

From `docs/implementation/packets/slice.json` (57 packets, 10 waves; validated
2026-09-25: no dangling dependency, acyclic, waves consistent, zero ownership
collisions between packets that can run concurrently).

| Step | Packet | Wave | Tier | Size | Work |
|---|---|---|---|---|---|
| 1 | SLICE-49 | 1 | T1 | S | Scaffold `qip-events::event_fabric` as doc-only stubs |
| 2 | SLICE-06 | 2 | T3 | M | CRC32C, the record and batch frame, which stage sets each header field |
| 3 | SLICE-16 | 3 | T3 | L | Segment log: append, fsync-before-ack, roll, seal, recover, retain |
| 4 | SLICE-27 | 4 | T3 | L | Broker core: partitions over segment logs, metadata, HW, `archived_through` |
| 5 | SLICE-30 | 5 | T3 | M | Consumer groups, QoS admission, quotas and isolation |
| 6 | SLICE-35 | 6 | T4 | L | Stream grants, key-scoped produce, the broker's protocol handler |
| 7 | SLICE-37 | 7 | T3 | M | `qip event-fabric verify \| inspect \| lag \| isolate \| release` |
| 8 | SLICE-41 | 8 | T4 | L | Slice suite A: happy path, replay, spool back to baseline (ADR 0100 §8 tests 1, 7, 8) |
| 9 | SLICE-46 | 9 | T2 | M | `make slice` and the slice runbook with its deferral register |
| 10 | SLICE-48 | 10 | T4 | M | Final integration: the eight real-process tests together and every gate |

Waves and parallel width: 9, 10, 8, 8, 7, 3, 4, 5, 2, 1 packets. Waves 1–5
keep the rung-8 ladder busy; waves 6–10 narrow onto T4 integration, which is
the lead's work by design (ADR 0098 §3 reserves integration, the paper
boundary, authentication and the ledger).

**Sequential by necessity:** the fabric spine (steps 2–6) — each consumes the
previous step's types — and the final integration. **Parallel by design:** the
reflex hand-off, the ledger, the operator verbs and the harness, each on files
no other concurrent packet owns.
