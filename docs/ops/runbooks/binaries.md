# The binaries

Companion to [the operator runbook](../RUNBOOK.md); it uses the same labels
(**ran**, **repo**, **not run**) and the same date, 2026-10-04. Every "ran" was
done against binaries built in this worktree with
`cargo build -p qip-api -p qip-cli -p qip-fastbrain -p qip-deepbrain -p qip-edge-node -p qip-fabricd -p qip-ledgerd`,
exit 0.

Config keys are the ones the code reads, found by search in each
`src/*.rs`. A key marked *required* stopped the process when absent (**ran**)
unless said otherwise. None of these processes runs in any cloud environment
today ([runbook section 1](../RUNBOOK.md#1-what-runs-where)).

Common to every binary:

- **Storage**: `QIP_STORAGE_TARGET` and `QIP_STORAGE_ROOT`
  (`qip-storage/src/settings.rs`). The nine target names the parser knows are
  `memory`, `file`, `engine`, `bigquery`, `cloud_storage`, `alloydb`, `spanner`,
  `bigtable`, `memorystore`; only `memory`, `file` and `engine` have an adapter
  in this build. An unrecognised name, a durable target with no root, or a root
  set alongside `memory` is refused at start-up, and the store is written and
  read back before the process reports healthy.
- **Autonomy ceiling**: `QIP_AUTONOMY_CEILING`, unset means `paper_trading`;
  `observation`, `advisory`, `paper_trading` are accepted; any live level stops
  the process (**ran**, exit 1).
- **Instrument catalogue**: `QIP_UNIVERSE_PATH`, *required* for api, fastbrain
  and deepbrain. Committed copy: `data/datasets/universe.json`.
- **Risk limits**: `QIP_RISK_LIMITS_PATH`; unset runs the shipped
  `conservative-paper` set of 16 limits (**ran**). A bound reaches a running
  process through this file and nothing else (ADR 0061).
- **Capital envelope key**: `QIP_CAPITAL_ENVELOPE_KEY` or
  `QIP_CAPITAL_ENVELOPE_KEY_FILE`.
- **Secrets**: through `_FILE` variants only in a deployment.
- **Logs**: banner and run log on stdout and stderr. No file is written.
- **Restart**: no binary installs a signal handler (the brains' module docs say
  so; this build has no dependency that could). A `SIGTERM` ends the process
  where it stands; what has not reached the archive is lost. Stop the brains
  with `POST /quiesce` first.

## qip-api

**Purpose.** The API server: REST under `/api/v1` (63 routes at the time of
the run, described at `/api/v1/openapi.json`), five server-sent-event streams,
the read-only operator console HTML (no JavaScript), the mesh endpoints cells
post to, and the central plane that signs envelopes. It is the only binary
that can trip the kill switch from outside the process.

| Key | Meaning | Default / note |
|---|---|---|
| `QIP_API_ADDRESS` | listen address | `127.0.0.1:8080`; the dev manifest sets `0.0.0.0:8080` |
| `QIP_TOKEN_MONITOR`, `_VIEWER`, `_ANALYST`, `_OPERATOR` (each also `_FILE`) | one bearer credential per role | at least the operator token, or exit 1 (**ran**) |
| `QIP_TOKEN_APPROVER`, `QIP_TOKEN_APPROVER_FILE` | retired | **stops the process**, naming the replacement |
| `QIP_UNIVERSE_PATH` | catalogue | *required* (**ran**) |
| `QIP_AUTONOMY_CEILING` | ceiling | `paper_trading` |
| `QIP_CAPITAL_ENVELOPE_KEY` (`_FILE`) | trust root that signs grants | seed-derived if unset, and the banner says so (**ran**) |
| `QIP_RISK_LIMITS_PATH` | limit set | shipped set |
| `QIP_API_TAPE_PATH` or `QIP_CONNECTOR_SOURCE` and `QIP_CONNECTOR_BASE_URL` | the feed `POST /cycle` senses | none: "senses nothing" (**ran**) |
| `QIP_ARBITRAGE_POLICY_PATH` | desk whitelist | none, ships empty |
| `QIP_REGION_DARK_AFTER` | dark-region window (ADR 0079) | unset means the derivation is off |
| `QIP_VENUE_REGISTRATIONS_PATH` | committed registration records | none: sources needing an account stay refused |
| `QIP_WALLET_STATEMENT_PATH`, `QIP_CAPITAL_FABRIC_PATH` | statement and corridor declarations | none |
| `QIP_MESH_CELLS`, `QIP_MESH_REGIONS`, `QIP_MESH_INBOX_CAPACITY`, `QIP_MESH_SPOOL_CAPACITY` | mesh backbone for cells | unset: "not served", cells pointed here are partitioned (**ran**) |
| `QIP_OPENOBSERVE_URL`, `_ORG`, `_AUTHORIZATION`, `_INTERVAL_SECS` | telemetry drain | unset: not draining (**ran**) |
| `QIP_STORAGE_TARGET`, `QIP_STORAGE_ROOT` | store | `memory` unless set (**ran**) |

**Ports.** One: `8080` in the manifest. Cloud Run ingress in the dev manifest
is `INGRESS_TRAFFIC_INTERNAL_ONLY`, with an Envoy egress sidecar `qip-egress`
whose startup probe is `/healthz` on `9900`.

**Health and readiness.** `GET /api/v1/health` (Monitor): `{"status":"ok",
"halted":false,"autonomy":"paper_trading","live_capable":false,
"reconciliation_breaks":0}` (**ran**); `status` becomes `halted` under a halt.
`GET /api/v1/system/status` (Viewer) adds cycles, events and archived counts.
The manifest's probes are `GET /api/v1` on 8080 (**repo**). It reports ready
only after storage is proven writable and the port is bound.

**Metrics.** `GET /api/v1/metrics`, Prometheus text (**ran**). Counters for
cycles, stages, breaches, denials, orders and the kill-switch gauge are
recorded by the kernel's `Platform`; the list is not restated here because
it drifts (`.claude/rules/domains/observability.md`).

**Failure modes.**

| Symptom | Cause | Do |
|---|---|---|
| exit 1, `QIP_UNIVERSE_PATH is not set` | no catalogue | point it at the committed file |
| exit 1, `no credential is configured` | no token | set `QIP_TOKEN_OPERATOR` or `_FILE` |
| exit 1, `will not start there` | live ceiling configured | set `paper_trading`; never lower the guard |
| exit 1, `... is no longer read` | retired token variable | remove the variable and its volume mount |
| 401 `no credential was presented` | no `Authorization: Bearer` header | send one |
| 403 on `DELETE /kill-switch` | by design (ADR 0065) | restart the process; see [runbook 4.2](../RUNBOOK.md#42-halting-the-central-plane) |
| startup banner `NOTHING SURVIVES A RESTART` | `memory` store | expected in every declared deployment |

**Restart and recovery.** Restarting clears a halt, discards the in-memory
chain, and re-reads every credential, which is also how a rotated secret is
picked up. With a durable store, `ChainArchive::open` adopts what is there and
`qip replay` verifies it ([disaster recovery](../../operations/disaster-recovery.md)).

## qip-fastbrain

**Purpose.** The fast path: polls a feed, runs a cycle on a clock, times each
cycle against a ceiling. It hosts only agents with no language-model access
and refuses to start otherwise. Banner (**ran**): one hosted agent,
`microstructure-analyst`, 50 ms budget, 64 tool calls, "no language model".

| Key | Meaning | Default |
|---|---|---|
| `QIP_FASTBRAIN_HEALTH_ADDRESS` | health and metrics bind | `0.0.0.0:8080` (**ran** with `127.0.0.1:8081`) |
| `QIP_FASTBRAIN_CYCLE_INTERVAL_MS` | cycle cadence | 100 ms |
| `QIP_FASTBRAIN_CYCLE_BUDGET_MS` | ceiling per cycle | 50 ms |
| `QIP_FASTBRAIN_BREACH_TOLERANCE` | consecutive breaches before unready | 3 |
| `QIP_FASTBRAIN_ARCHIVE_EVERY` | cycles between archive hand-overs | 100 |
| `QIP_FASTBRAIN_SHUTDOWN_BUDGET_MS` | flush budget on the way out | 5 s |
| `QIP_FASTBRAIN_MAX_CYCLES`, `QIP_FASTBRAIN_MAX_RUNTIME_SECS` | stop by itself | unbounded; the **ran** drill used 20 s |
| `QIP_FASTBRAIN_SEED` | simulation seed | |
| `QIP_FASTBRAIN_TAPE_PATH`, `QIP_FASTBRAIN_REPLAY_PATH` | recorded input | none: `synthetic-exchange`, "NOT production-grade; no capital decision may rest on it" |
| `QIP_CONNECTOR_SOURCE`, `QIP_CONNECTOR_BASE_URL`, `QIP_MARKET_DATA_*` (`_BASE_URL`, `_KEY`, `_KEY_HEADER`, `_PATH`, `_SYMBOLS`, `_VENUE`) | market-data connector | none |
| `QIP_UNIVERSE_PATH`, `QIP_AUTONOMY_CEILING`, `QIP_CAPITAL_ENVELOPE_KEY`, `QIP_RISK_LIMITS_PATH`, `QIP_OPENOBSERVE_*`, storage | common | |

**Ports.** One, the health surface. **Probes.** `GET /health` returns
`{"alive":true,"cycles":30,"cycle_in_flight":false}` HTTP 200 (**ran**).
`GET /ready` returns 200 with the full status (`"ready":true,
"feed_is_production_grade":false`, cycle interval, budget) or **503** when
unready (**ran** for 200; the 503 path is covered by a unit test and was not
provoked). The Cloud Run manifest probes `/health` on 8080 (**repo**).
**Metrics.** `GET /metrics` (**ran**). **Quiesce.** `POST /quiesce` returns
HTTP 202 `{"quiescing":true,...}` and is refused unless it comes from
loopback (**ran** from loopback); the process finished the cycle in flight,
flushed 117 event records and exited 0 (**ran**).

**Failure modes.** Exit 78 for any configuration problem (**ran**, missing
`QIP_UNIVERSE_PATH`); exit 1 for a live ceiling (**ran**); a roster agent
holding `call_language_model` stops it at start. A run of budget breaches
takes it **out of rotation without taking it down**: `/ready` goes 503 while
`/health` stays 200.

**Restart.** Pinned to one instance in the manifest
(`minInstanceCount: 1`, `maxInstanceCount: 1`) because one hash-chained log
must have one writer. Quiesce first so the archive is handed the log.

## qip-deepbrain

**Purpose.** The research loop: discovery, causal reasoning, simulation,
optimisation, learning, a bounded evolutionary search whose candidates "register
at the bottom rung and never promote themselves". It reaches no venue
(banner: `execution-trader — this node reaches no venue`). It hosts 17 agents,
16 of which may consult a language model; with
`QIP_LANGUAGE_MODEL_PROVIDER` unset it uses `deterministic-local-v1` (**ran**).

| Key | Meaning | Default |
|---|---|---|
| `QIP_DEEPBRAIN_HEALTH_ADDRESS` | health and metrics bind | `0.0.0.0:8080` (**ran** with `127.0.0.1:8082`) |
| `QIP_DEEPBRAIN_CYCLE_INTERVAL_SECS` (also `QIP_CYCLE_INTERVAL_SECONDS`) | cadence | 300 s |
| `QIP_DEEPBRAIN_FAILURE_TOLERANCE` | consecutive failures before unready | 3 |
| `QIP_DEEPBRAIN_ARCHIVE_EVERY` | cycles between archive hand-overs | 1 |
| `QIP_DEEPBRAIN_SHUTDOWN_BUDGET_SECS` | flush budget | 30 s |
| `QIP_DEEPBRAIN_MAX_CYCLES`, `QIP_DEEPBRAIN_MAX_RUNTIME_SECS` | stop by itself | unbounded; **ran** with 20 s |
| `QIP_DEEPBRAIN_EVENT_LOG` | event log file | `deepbrain-events.jsonl` under the storage root; in memory otherwise |
| `QIP_DEEPBRAIN_DISCOVER_EVERY`, `QIP_DEEPBRAIN_EVOLUTION_EVERY`, `QIP_DEEPBRAIN_EVOLUTION_CANDIDATES` | cadences | discovery disabled when 0 (**ran**) |
| `QIP_DEEPBRAIN_SOURCE_CANDIDATES_PATH`, `QIP_CENTRAL_HORIZONS_PATH` | inputs | |
| `QIP_DEEPBRAIN_TAPE_PATH`, `QIP_DEEPBRAIN_REPLAY_PATH` | recorded input | |
| `QIP_LANGUAGE_MODEL_PROVIDER`, `QIP_LANGUAGE_MODEL`, `QIP_LANGUAGE_MODEL_BASE_URL`, `QIP_MODEL_PROVIDER_ATTESTATION_PATH` | hosted model | none |
| `QIP_CONNECTOR_SOURCE`, `QIP_CONNECTOR_BASE_URL`, `QIP_UNIVERSE_PATH`, `QIP_AUTONOMY_CEILING`, `QIP_CAPITAL_ENVELOPE_KEY`, `QIP_RISK_LIMITS_PATH`, `QIP_OPENOBSERVE_*`, storage | common | |

**Probes.** `GET /health` HTTP 200 `{"alive":true,"cycles":1,...}` (**ran**);
`GET /ready` carries the roster and reports `warming` until the first cycle
lands (**repo**, disaster-recovery page; the 200 I read was after cycle 1);
`GET /metrics` (**ran**); `POST /quiesce`, loopback only. A long cycle is "research,
not a fault": there is no latency ceiling.

**Failure modes and restart.** As the fast brain: exit 78 on configuration,
exit 1 on a live ceiling, one instance only. The 300 s cadence means a
20-second bounded run executes exactly one cycle (**ran**).

## qip-edge-node

**Purpose.** One region's cell: decides on its own against local limits, journals
every decision and refusal, and trades only inside a signed capital envelope. It
cannot reach a language model (no dependency on `qip-ai`, held by an
architecture test).

Required, all five named at once on failure (**ran**, exit 78):
`QIP_CELL_ID`, `QIP_CELL_REGION`, `QIP_CAPITAL_ENVELOPE_KEY` (or `_FILE`),
`QIP_VENUES` (comma-separated), `QIP_REGION_ALLOCATION` (a positive decimal;
zero, blank or unparseable is refused, and since ADR 0039 it is a *ceiling*,
not a funding).

| Key | Meaning | Default |
|---|---|---|
| `QIP_HEALTH_PORT` | health and metrics port | 8080 (**ran** with 8083) |
| `QIP_VENUE_FEED` | `simulated` runs passes against the in-process venue; any other value stops the process naming ADR 0003 | unset: no passes run |
| `QIP_VENUE_ADAPTER`, `QIP_VENUE_ORDER_ENTRY_ACKNOWLEDGED`, `QIP_VENUE_IDEMPOTENCY`, `QIP_GATEWAY_SEED`, `QIP_VENUE_FEED_ENDPOINT`, `QIP_DROP_COPY_ENDPOINT` | order-entry adapter; `rest` opens a socket to a venue and is refused unless the operator writes the destination out | in-process simulated exchange (**ran**: `reaches_a_socket=false`) |
| `QIP_HALT_FLAG_PATH` | polled halt flag, absolute path | unset: broadcast halt only, said at start |
| `QIP_MESH_PEER`, `QIP_MESH_SEED` | the centre | unset: the cell publishes nothing, receives no capital and stops when its envelope expires (**ran**) |
| `QIP_ARBITRAGE_STRATEGY`, `QIP_DEFAULT_PRICING`, `QIP_STRATEGY_PLAN_PATH`, `QIP_REPRICE`, `QIP_CROSS_REGION_MIRROR_PATH` | desk, strategy pricing, plan, requote threshold, cross-region mirror | each unset one is announced as "awaiting" (**ran**) |
| `QIP_STORAGE_TARGET`, `QIP_STORAGE_ROOT` | journal destination | memory; the template uses `engine` at `/var/lib/qip/journal` (**repo**) |
| `QIP_MIRROR_PATH` | retired | stops the process, naming the replacement |

**Probes.** Every path except `/metrics` answers the same JSON health body:
cell, region, `halted`, `halt_flag` (path and `engaged`), `live_capable:false`,
venues, `region_allocation_free`, and `region_share` (**ran**). A fresh node
reads `"funded":false` with the reason `no region share has been applied: this
node opened unfunded and places nothing until the centre's policy payload names
grants this cell holds` (**ran**): a healthy-looking cell that places nothing is
the expected state until a centre exists. **Metrics.** `GET /metrics`,
`qip_edge_*` (**ran**): `qip_edge_halted{source}` for `kill_switch`, `policy`,
`polled`; freshness, refusals, signals, orders, mesh circuit. The health
server is single-threaded; a scrape renders on the thread that flushes the
journal.

**Halt.** [Runbook 4.3](../RUNBOOK.md#43-halting-a-cell). **Failure modes.**
Exit 78 for configuration; a live-class venue is refused at two seams and
counted at `qip_edge_refusals_total{gate="live_venue"}`; a missing halt-flag
directory halts the cell. **Restart.** Books, features and watermarks are
deliberately not persisted and are rebuilt from the feed; the journal is the
only record and the replacement node starts with an empty one unless a
snapshot is restored ([incidents](incidents.md#a-cell-is-halted)).

## qip-fabricd

**Starts on a complete configuration and on nothing less; not deployed.**
The event fabric's single-node broker (ADR 0100). Ran with no configuration
(**ran**, 2026-10-04): exit 1 and `qip-fabricd: refusing to start. invalid:
QIP_EVENT_FABRIC_DATA_DIR is not set; set it to the directory partitions are
stored in. The event fabric has no default for it`. Ran against the committed
local catalogue on loopback (**ran**, same day): `serving the event-fabric
protocol on 127.0.0.1:17100 and health on 127.0.0.1:17101`, `GET /healthz`
answered 200 `"status":"ready"`, `/metrics` answered the `qip_event_fabric_*`
series, a metadata request with an `operator` token answered 200 and one with
no token answered 401. It is still excluded from the image matrix and every
workload catalogue (`docs/adr/0010-what-gets-deployed.md`): where it runs is
undecided (ADR 0099 C8). Do not deploy it.

Seven settings are required and have no default (ADR 0100 section 5: nothing
rests on a broker default); two are optional.

| Variable | What it is |
|---|---|
| `QIP_EVENT_FABRIC_DATA_DIR` | partitions, the leader epoch, consumer checkpoints, isolations and the operator-action log |
| `QIP_EVENT_FABRIC_CATALOGUE` | the stream catalogue: stream policies and grants. Re-read while running; a changed file that parses replaces the grants in force, and one that does not changes nothing and is reported on the health body as `catalogue_error` |
| `QIP_EVENT_FABRIC_IDENTITIES_FILE` | one `<identity> <sha256 of its token>` per line. Digests, never tokens |
| `QIP_EVENT_FABRIC_ARCHIVE_DIR` | where sealed segments are archived. A directory, refused if it is inside the data directory. The Cloud Storage adapter is not selectable here yet |
| `QIP_EVENT_FABRIC_LISTEN` | the protocol listener, `/v1/event-fabric/...`, bearer token on every request |
| `QIP_EVENT_FABRIC_HEALTH_LISTEN` | health and `/metrics`, on a listener of its own, refused if it is the protocol's |
| `QIP_EVENT_FABRIC_PARTITIONS` | partitions per stream, 1 to 1024. Changing it on an existing data directory changes which partition every key routes to |
| `QIP_EVENT_FABRIC_SEGMENT_BYTES` | optional: the size at which a segment is sealed |
| `QIP_EVENT_FABRIC_HOUSEKEEPING_MS` | optional, default 1000: how often aged segments are sealed and archived and the catalogue is re-read |

**What it is not.** One broker, one disk: no follower, no replication, no
failover (ADR 0100 section 3). Plaintext TCP; the bearer token guards against
misconfiguration, not an on-path reader. A stalled archive delays no append
but stops `archived_through` advancing, so producers on a quorum
acknowledgement stop being released: watch
`qip_event_fabric_archive_lag_segments` and `archive_error` on the health
body. **Restart.** The leader epoch is bumped and persisted at every start;
partitions recover from their segments; an isolation an operator imposed
(`admin/isolate`) survives the restart and has to be released.

**Isolating a partition** (FABRIC-028), on the broker's own host — the
operator token travels in plaintext, so the command refuses any peer that is
not loopback:
`qip event-fabric isolate --peer 127.0.0.1:<port> --stream <name> --partition <n> --operator <identity> --reason <why>`
and `qip event-fabric release` with the same arguments less `--reason`. The
token is read from the file `QIP_EVENT_FABRIC_OPERATOR_TOKEN_FILE` names and
never from the variable itself; `--operator` must be the identity that token
verifies as, and that identity needs an `admin` grant on the stream in the
catalogue. Produce to the partition is refused naming the operator and the
reason until it is released; nothing is deleted and it can still be read.
One partition per command.

## qip-ledgerd

**Refuses to start, always** (**ran**, exit 1, same shape of message, naming
"the ledger's role, sole writer of the chain of double-entry"). Its `store.rs`
(a library module, 955 lines) and the posting logic in `qip-portfolio::ledger`
exist; `config`, `consumer` and `read_api` are stubs. There is no ledger
process, so there is no ledger to back up, restore or stall.

## qip-web

A library with no `main.rs` and no binary: the HTML pages of the operator
console, rendered by `qip-api`. There is nothing to start. The console is
read-only: it can trip the kill switch and has no path that clears one, and the
UI must render `PAPER TRADING` wherever posture is shown.

## qip (the CLI)

The `qip-cli` crate builds a binary named `qip`. Commands (**ran**, `qip help`,
exit 0): `status`, `demo --live [n]`, `cycle [n]`, `agents`, `governance`,
`limits`, `storage`, `registrations [--config <path>]`, `replay --journal <path>
[--config <path>]`, `blueprint render|check [--root <path>]`, `event-fabric grant
...`. There is deliberately no command that raises autonomy. `qip status`
(**ran**) printed autonomy, ceiling, `live: unreachable in this deployment`,
`halted: no`, `log chain: intact`, `store: memory`. Exit codes: 0 ran and found
nothing wrong; 1 could not answer; 3 is a verdict, from `registrations` while
any source is still refused and from `replay` when the chain is broken or a
registry disagrees. `demo --live` binds loopback peers on ephemeral ports and
every fill it prints is made up in the process; it cannot be pointed at a market.
