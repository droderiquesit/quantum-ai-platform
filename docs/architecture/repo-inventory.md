# Repository Inventory

## Frontend
**Status:** None exists. This is a pure Rust backend + Terraform infrastructure project.
- No `package.json`, `next.config.*`, or `.tsx` files found outside test/doc directories.
- No React, Next.js, or frontend framework present.

## Codebase Overview
- **Total Crates:** 59 (libs, services, apps, agents, runtime, edge, quant)
- **Directory Structure:**
  - `crates/apps/`: qip-api, qip-cli, qip-deepbrain, qip-fastbrain, qip-edge-node, qip-web
  - `crates/libs/`: 18 library crates (storage, quantum, core, market, portfolio, etc.)
  - `crates/services/`: Market ingestion, data-finder, optimization, etc.
  - `crates/tests/`: qip-acceptance (179 tests across 13 files)

## False Completion Inventory

### Macro Count (Non-Test Code)
| Macro | Count | Severity |
|-------|-------|----------|
| `todo!()` | 0 | N/A |
| `unimplemented!()` | 0 | N/A |
| `panic!()` | 7 | High |

### Panic Locations (7 instances)
```
crates/libs/qip-storage/src/redis.rs:2022
crates/libs/qip-core/src/testing.rs:53
crates/libs/qip-core/src/decimal.rs:317
crates/runtime/qip-kernel/src/config.rs:410
crates/runtime/qip-kernel/src/config.rs:460
crates/apps/qip-fastbrain/src/health.rs:431
crates/apps/qip-deepbrain/src/health.rs:460
```

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

### HTTP Routes (31 endpoints)
**Prefix:** `/api/v1`

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
| `/metrics` | GET |
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

**OpenAPI:** `crates/apps/qip-api/src/openapi.rs` exists.
**Streaming:** No WebSocket, SSE, or EventSource found.

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
- **File:** `crates/libs/qip-quantum/src/provider.rs`
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

### Test Files (crates/tests/qip-acceptance/tests/)
| File | #[test] | #[ignore] |
|------|---------|-----------|
| acceptance.rs | 12 | 0 |
| architecture.rs | 18 | 0 |
| chaos.rs | 1 | 0 |
| compliance_proof.rs | 4 | 0 |
| documentation.rs | 21 | 0 |
| e2e.rs | 1 | 0 |
| e2e_live.rs | 1 | 0 |
| infrastructure.rs | 63 | 0 |
| performance.rs | 9 | 0 |
| resilience.rs | 8 | 0 |
| security.rs | 18 | 0 |
| stress.rs | 16 | 0 |
| truth_loop.rs | 7 | 0 |

**Total:** 179 tests, 0 ignored. All tests are active.
