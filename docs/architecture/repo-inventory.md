# Repository Inventory

Counts below are measured, and each names the command that measures it so
the next reader can recount rather than trust. Re-counted 2026-09-05 against
the working tree above `b42214b`.

## Frontend

**Status:** Next.js + TypeScript, in two applications with their own
toolchains (`frontend/CLAUDE.md`). An earlier version of this section said no
frontend existed; that was true of the tree it was written against and is
not true now.

- `frontend/portal/` — the authenticated console and installed PWA.
  Navigation is data in `frontend/portal/src/lib/nav.ts`: 10 sections and 40
  destinations (`grep -c '^    label: '` and `grep -c '^        href: '`).
- `frontend/landing/` — the public landing application; not a workspace
  member, deliberately (ADR 0015).
- `frontend/packages/` — shared browser packages.

### Cognition pages (added 2026-09-05)

| Path | Reads | Test |
|---|---|---|
| `frontend/portal/src/app/(portal)/cognition/self-model/page.tsx` | `GET /api/v1/cognition/self-model` through `useSelfModel` (`src/lib/hooks/useCognition.ts:84`) | `tests/cognition-self-model.spec.ts` (2 tests) |
| `frontend/portal/src/app/(portal)/cognition/precedents/page.tsx` | `GET /api/v1/cognition/precedents` through `usePrecedents` (`src/lib/hooks/useCognition.ts:92`) | `tests/cognition-precedents.spec.ts` (2 tests) |

Both pages are read-only; the hook file declares no non-GET fetcher
(`useCognition.ts:18-21`). Nav section "Cognition" at `src/lib/nav.ts:149`.

## Codebase Overview
- **Total Crates:** 58 (`find backend/crates -name Cargo.toml | wc -l`)
- **Directory Structure:**
  - `backend/crates/apps/`: qip-api, qip-cli, qip-deepbrain, qip-fastbrain, qip-edge-node, qip-web
  - `backend/crates/libs/`: library crates (storage, quantum, core, market, portfolio, etc.)
  - `backend/crates/services/`: Market ingestion, data-finder, optimization, capital, capital-fabric, etc.
  - `backend/crates/tests/`: qip-acceptance (304 tests across 20 files; see the test inventory below)

## False Completion Inventory

### Macro Count (Non-Test Code)
| Macro | Count | Severity |
|-------|-------|----------|
| `todo!()` | 0 | N/A |
| `unimplemented!()` | 0 | N/A |
| `panic!()` | 7 | High |

### Panic Locations (7 instances)
```
backend/crates/libs/qip-storage/src/redis.rs:2022
backend/crates/libs/qip-core/src/testing.rs:53
backend/crates/libs/qip-core/src/decimal.rs:317
backend/crates/runtime/qip-kernel/src/config.rs:410
backend/crates/runtime/qip-kernel/src/config.rs:460
backend/crates/apps/qip-fastbrain/src/health.rs:431
backend/crates/apps/qip-deepbrain/src/health.rs:460
```

(The macro table and the line numbers above were not re-counted on
2026-09-05.)

### Comment Markers
- **TODO/FIXME/PLACEHOLDER:** 0 instances
- **MOCK/STUB/demo:** 0 instances

**What that measures, and what it does not.** No `todo!()`, `unimplemented!()`,
`TODO`, `FIXME`, `PLACEHOLDER`, `MOCK` or `STUB` appears anywhere outside test
code, so no function in this workspace is a stub waiting to be written. That is
a claim about completeness of implementation and nothing more. It says nothing
about whether the platform has run against a live venue, sustained load, or
survived a real outage — none of which it has yet been asked to do. A count of
absent markers is the weakest evidence a codebase can offer about its own
readiness, and reporting it as a readiness verdict is how a repository talks
itself into a deployment it has not earned.

## API Surface

### HTTP Routes (44 route entries over 43 paths)
**Prefix:** `/api/v1`. Source: `backend/crates/apps/qip-api/src/routes.rs`,
`ROUTES` (`grep -c 'pattern: "'` gives 44; `/kill-switch` is declared twice,
once per method).

| Endpoint | Methods |
|----------|---------|
| `/health` | GET |
| `/system/{status,metrics,governance}` | GET |
| `/mesh` | GET |
| `/portfolio` | GET |
| `/opportunities` | GET |
| `/proposals` | GET |
| `/orders` | GET |
| `/agents` | GET |
| `/cycle` | POST |
| `/kill-switch` | POST, DELETE |
| `/autonomy` | GET |
| `/system` | GET |
| `/regions` | GET |
| `/markets` | GET |
| `/assets` | GET |
| `/arbitrage` | GET |
| `/strategies` | GET |
| `/models` | GET |
| `/capital` | GET |
| `/risk` | GET |
| `/fills` | GET |
| `/pnl` | GET |
| `/data-sources` | GET |
| `/training` | GET |
| `/quantum` | GET |
| `/predictions` | GET |
| `/regimes` | GET |
| `/correlation` | GET |
| `/backtests` | GET |
| `/news` | GET |
| `/cognition/self-model` | GET — viewer role; `routes.rs:338`, dispatched at `:798`; shapes in `src/self_model_views.rs`, contract in `ROUTES-COGNITION.md`; `tests/self_model_routes.rs` (6 tests) |
| `/cognition/precedents` | GET — viewer role; `routes.rs:347`, dispatched at `:809`; same files |
| `/ledger/users` | GET — contract in `ROUTES-LEDGER.md` |
| `/wallet` | GET — answers `assembled: false` with its reason; the kernel holds no wallet (`src/ledger_views.rs:332`) |
| `/corridors` | GET — answers `held: false` for both registries (`src/ledger_views.rs:431`) |
| `/transfer-gate` | GET |
| `/stream/{market,signals,orders,positions,health}` | GET — server-sent events, `content-type: text/event-stream` (`src/stream.rs:286`) |

Note: `/metrics` is not in `ROUTES`. It is matched as a GET arm at
`routes.rs:754` (`SCRAPE_PATH` at `:44`, `scrape` at `:1278`) and answers the
Prometheus exposition as text.

**OpenAPI:** `backend/crates/apps/qip-api/src/openapi.rs` exists.
**Streaming:** server-sent events on the five `/stream/*` routes
(`backend/crates/apps/qip-api/src/stream.rs`); no WebSocket. An earlier
version of this line said no SSE existed.

## Data Connector Abstraction

### Trait Definitions
- **SourceProbe** (qip-data-finder/src/probe.rs:92)
  - Implementations: `InMemoryProbe`, `NetworkProbe`
  - Generic probe interface for market data sources
  
- **LiquiditySource** (qip-arbitrage/src/liquidity.rs:38)
  - Generic liquidity provider abstraction
  
- **TokenSource** (qip-storage/src/gcp/auth.rs:148)
  - GCP authentication token provider

### Data Connectors
- **qip-market-ingestion:** Adapters for alternative data, depth, narrative, replay, REST, synthetic sources
- **qip-data-finder:** Ingestion probe, endpoint discovery, schema, quality scoring, robot detection

## IBM Quantum Integration

### Real HTTP Adapter (Not Simulated)
- **File:** `backend/crates/libs/qip-quantum/src/provider.rs`
- **Implementation:** Full HTTP client adapter for IBM Quantum Platform
  - Uses `qip_transport::HttpClient` for TLS-terminating proxy communication
  - `HostedTransport` pattern: TLS proxy over `http://` on cluster network
  - `submit_job()` function at line 651 submits QUBO to IBM backend
  - API token authentication via environment variable
  
- **Quantum Solvers:** QAOA (Quantum Approximate Optimization Algorithm) on IBM Qiskit Runtime
  - IbmQuantumConfig: Channels, backend selection, token management
  - Full integration with IBM's `ibm_quantum_platform` channel

### Local Fallback
- **Steepest-descent local search** (qip-numerics): the in-process solver used whenever IBM is unavailable, so an unreachable vendor degrades the answer rather than losing it
- Both report unavailable state; IBM is primary, local is fallback

## Test Inventory

### Test Files (backend/crates/tests/qip-acceptance/tests/)
Counted 2026-09-05 with `grep -c '#\[test\]'` and `grep -c '#\[ignore'` per file.

| File | #[test] | #[ignore] |
|------|---------|-----------|
| acceptance.rs | 16 | 0 |
| api_boundary.rs | 8 | 0 |
| architecture.rs | 26 | 0 |
| chaos.rs | 1 | 0 |
| compliance_proof.rs | 5 | 0 |
| console_route.rs | 4 | 0 |
| documentation.rs | 22 | 0 |
| e2e.rs | 1 | 0 |
| e2e_live.rs | 1 | 0 |
| egress.rs | 23 | 0 |
| gitops.rs | 22 | 0 |
| infrastructure.rs | 75 | 0 |
| manifest_wiring.rs | 12 | 0 |
| paper_boundary.rs | 5 | 0 |
| performance.rs | 24 | 0 |
| resilience.rs | 8 | 0 |
| security.rs | 19 | 0 |
| stress.rs | 16 | 0 |
| terraform_contract.rs | 9 | 0 |
| truth_loop.rs | 7 | 0 |

**Total:** 304 tests, 0 ignored. All tests are active. Of `performance.rs`'s
24, fourteen are the in-process execution measurements recorded in
`docs/ops/execution-measurements.md` (`performance.rs:1172-1807`) and one
(`:1898`) checks that document against the file.
