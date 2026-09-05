# Repository Inventory

Counts below are measured, and each names the command that measures it so
the next reader can recount rather than trust. Re-counted 2026-09-05T13:16Z
against the **uncommitted** working tree above `e71c397`.

**Recount before quoting any number here, and treat that as an instruction
rather than a courtesy.** This tree is not quiescent: several lanes were adding
code while the counts below were taken, and three of the figures moved between
two counts twenty minutes apart in the same session — the acceptance-suite
total went 312 → 322, `qip-api`'s route table 46 → 47, and
`qip-kernel/src/platform.rs` shifted one function by roughly 270 lines. A
number in this file is a reading at an instant, not a property of the
repository.

## Frontend

**Status:** Next.js + TypeScript, in two applications with their own
toolchains (`frontend/CLAUDE.md`). An earlier version of this section said no
frontend existed; that was true of the tree it was written against and is
not true now.

- `frontend/portal/` — the authenticated console and installed PWA.
  Navigation is data in `frontend/portal/src/lib/nav.ts`: 10 sections and 41
  destinations (`grep -c '^    label: '` and `grep -c '^        href: '`). The
  forty-first is the venue-registrations page added by the uncommitted
  registration wave.
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

### Venue registrations page (added 2026-09-05, uncommitted at this writing)

| Path | Reads | Writes | Test |
|---|---|---|---|
| `frontend/portal/src/app/(portal)/data-sources/registrations/page.tsx` | `GET /api/v1/registrations` through `useRegistrations` (`src/lib/hooks/useRegistrations.ts`) | `POST /api/v1/registrations/{source_id}/approve` (`src/lib/api/endpoints.ts`, `REST.registrationApprove`) | `tests/registrations-page.spec.ts` (8 tests), `tests/registrations-gateway.spec.ts` (2 tests) |

**This is the first console page that is not read-only, and the sentence "the
console holds no control that acts" is retired here rather than left standing.**
What the one control does: it records that the signed-in operator registered
with a venue under terms they read. It names no instrument, side, quantity or
price; it creates no venue account; its `secret` field carries a deployment
*variable name*, screened by the platform against the manifest's `SecretRef`
shape rule, never a credential value. No control on this or any other page can
submit an order, and no live path exists for one to reach. The gateway's
allowlist gained a path-parameter matcher for it that matches exactly one
non-empty segment (`declaresWrite` and `pathMatches` in
`src/lib/api/endpoints.ts`), so `/registrations/a/b/approve` and
`/registrations/approve` stay undeclared and are refused before the console's
credential is read off disk.

Playwright was **not run** for this entry; the test counts above are
declarations counted with `grep -c '^test(' frontend/portal/tests/*.spec.ts`,
which totalled 87 across 23 spec files at this reading. The last figure anyone
actually ran is 82.

## Codebase Overview
- **Total Crates:** 58 (`find backend/crates -name Cargo.toml | wc -l`)
- **Directory Structure:**
  - `backend/crates/apps/`: qip-api, qip-cli, qip-deepbrain, qip-fastbrain, qip-edge-node, qip-web
  - `backend/crates/libs/`: library crates (storage, quantum, core, market, portfolio, etc.)
  - `backend/crates/services/`: Market ingestion, data-finder, optimization, capital, capital-fabric, etc.
  - `backend/crates/tests/`: qip-acceptance (322 tests across 21 files at 13:16Z on 2026-09-05; see the test inventory below, and recount)

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

### HTTP Routes (48 route entries over 47 paths, at 13:16Z on 2026-09-05)
**Prefix:** `/api/v1`. Source: `backend/crates/apps/qip-api/src/routes.rs`,
`ROUTES`. Count with `grep -c '^        pattern: '`, which gives 48 — **not**
with `grep -c 'pattern: "'`, which gives 47 because one entry's pattern is the
constant `SCRAPE_PATH` rather than a string literal, and which is how this file
came to say `/metrics` was outside the table. `/kill-switch` is declared twice,
once per method, so 48 entries stand over 47 paths. The file is uncommitted and
moving: the literal count was 44 when the table below was last written, 46 an
hour before this reading, and 47 at it.

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
| `/cognition/self-model` | GET — viewer role; shapes in `src/self_model_views.rs`, contract in `ROUTES-COGNITION.md`; `tests/self_model_routes.rs` (6 tests) |
| `/cognition/precedents` | GET — viewer role; same files |
| `/ledger/users` | GET — **analyst** role (`routes.rs`, `required_role: Role::Analyst`), not viewer; contract in `ROUTES-LEDGER.md`. Each row now carries the ledger's eligibility verdict for that user and, when it would refuse funding, the refusal's token and sentence |
| `/ledger/users/{user}/eligibility` | POST — operator role. Records an operator's eligibility decision for one user, journalled before the registry adopts it. **Landing in a parallel lane while this was written**; `api_boundary.rs`'s mutating-route set does not yet name it |
| `/wallet` | GET — `assembled` is whether the fabric journal's state holds a wallet, and it does once a statement has been observed and a cycle reconciled against it (`src/ledger_views.rs`; the `NO_WALLET` sentence is what it answers when none has). This row used to say the kernel held none; that stopped being true when the kernel took one `FabricJournal`. No environment mounts a statement, so a *deployed* `/wallet` still answers `assembled: false` |
| `/corridors` | GET — answers the journal's corridor and destination records; this row used to say `held: false` for both registries, which the same change retired |
| `/transfer-gate` | GET |
| `/registrations` | GET — viewer role. Every catalogued source with its registration requirement, standing (keyless, registered by whom, or pending and why), the terms to read, the deployment variable the credential is read under and the Secret Manager command that fills it — names only, never a value. Contract in `ROUTES-REGISTRATIONS.md`; `tests/registrations.rs` (5 tests) |
| `/registrations/{source}/approve` | POST — operator role. Records that the authenticated operator registered with the venue and read the terms the body cites; journalled before it stands. The body carries a deployment variable *name*, screened against the manifest's `SecretRef` shape rule, and a refusal never repeats what it refused |
| `/metrics` | GET — **monitor** role, and it *is* a `ROUTES` entry: its pattern is the constant `SCRAPE_PATH` rather than a literal, which is why a `grep` for `pattern: "` misses it. Answers the Prometheus text exposition. This file previously said it was outside the table |
| `/stream/{market,signals,orders,positions,health}` | GET — server-sent events, `content-type: text/event-stream` (`src/stream.rs`) |

Line numbers were removed from this table on 2026-09-05: `routes.rs` and
`ledger_views.rs` are uncommitted and were being edited by other lanes, and
every number the table carried had already drifted. Find a route with
`grep -n 'pattern: ' backend/crates/apps/qip-api/src/routes.rs`.

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
Counted 2026-09-05T13:16Z with `grep -c '#\[test\]'` and `grep -c '#\[ignore'`
per file, against an uncommitted tree several lanes were adding to. Six of the
twenty-one figures moved in the twenty minutes before this count was taken
(`compliance_proof` 5→7, `infrastructure` 75→78→81, `performance` 24→25,
`security` 19→22, `truth_loop` 7→8, and `region_share.rs` appeared). Recount.

| File | #[test] | #[ignore] |
|------|---------|-----------|
| acceptance.rs | 16 | 0 |
| api_boundary.rs | 8 | 0 |
| architecture.rs | 26 | 0 |
| chaos.rs | 1 | 0 |
| compliance_proof.rs | 7 | 0 |
| console_route.rs | 4 | 0 |
| documentation.rs | 22 | 0 |
| e2e.rs | 1 | 0 |
| e2e_live.rs | 1 | 0 |
| egress.rs | 23 | 0 |
| gitops.rs | 22 | 0 |
| infrastructure.rs | 81 | 0 |
| manifest_wiring.rs | 12 | 0 |
| paper_boundary.rs | 5 | 0 |
| performance.rs | 25 | 0 |
| region_share.rs | 5 | 0 |
| resilience.rs | 8 | 0 |
| security.rs | 22 | 0 |
| stress.rs | 16 | 0 |
| terraform_contract.rs | 9 | 0 |
| truth_loop.rs | 8 | 0 |

**Total:** 322 tests across 21 files, 0 ignored, at the instant named. All
tests are active. Fourteen of `performance.rs`'s are the in-process execution
measurements recorded in `docs/ops/execution-measurements.md`, and one checks
that document against the file — the line ranges this paragraph used to give
were dropped because the file is under edit.

`region_share.rs` is the newest suite, added by `0829b29`: five tests that
drive a real `CentralPlane` with two cells under one region and apply the
centre's signed payload to two real `qip-edge` cells, which is the cross-crate
property ADR 0039 named as missing. It cost the acceptance crate one in-tree
dev-dependency (`qip-lifecycle`); no third-party crate was added.

**A count of tests is not a claim that they pass.** The only suite this
document's 2026-09-05 pass ran is `documentation`; everything else here is a
declaration counted in a source file.
