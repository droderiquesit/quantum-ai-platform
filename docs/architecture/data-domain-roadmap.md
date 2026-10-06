# DATA Domain — v12.0 Implementation Roadmap

**Status Date:** 2026-10-06  
**Overall Completion:** 2 COMPLETE (2.6%), 32 PARTIAL (42.1%), 21 MISSING (27.6%), 18 BLOCKED (23.7%), 3 NEEDS-VALIDATION (3.9%)  
**Verified:** 6 requirements through command-driven test execution

## Summary

The DATA domain (76 requirements) manages data ingestion, source lifecycle, licensing compliance, and knowledge retention. Verification work completed via systematic command execution proves architectural patterns in place for body-free storage, licensing enforcement, and retention separation. Key gaps center on autonomous source discovery (CRAWL stage), historical re-fetch paths, sandbox isolation for new adapters, and deployment activation of GCS-backed storage.

## Verification Status by Requirement Class

### COMPLETE (2 requirements)

| ID | Requirement | Verified |
|---|---|---|
| DATA-029 | Licensing posture evaluated before source use (family-agnostic gate) | ✓ PASS: `cargo test -p qip-data-finder --test legality` (27/27, 0.00s) |
| DATA-033 | Discovered sources appear in mesh catalogue on registration | ✓ PASS: `cargo test -p qip-kernel --test platform_services` (integration path verified) |

**Status:** Both requirements verified. Implementation is real, tested, reached from production (qip-deepbrain DiscoveryDesk, qip-api/qip-fastbrain admission gates). Deployability blocked by discovery configuration being null in every environment.

---

### PARTIAL (32 requirements) — Verified Subset

#### Core Architecture (VERIFIED)

| ID | Requirement | Command | Result | Gap |
|---|---|---|---|---|
| DATA-001 | No raw external corpus retained; body-free storage enforced | `cargo test -p qip-market-ingestion --test narrative_feed the_article_text_reaches_no_record_the_log_seals_and_a_hashed_reference_stands_in_for_it` | ✓ PASS (1/1, 0.00s) | Headlines retained verbatim in log and world model; needs allow-list ruling (DATA-005) or derived label replacement |
| DATA-003 | Every fetch writes source manifest record (per-fetch + per-source) | `cargo test -p qip-kernel --test references` | ✓ PASS (5/5, 0.02s) | Missing fields: etag, cursor, entitlement, geography, freshness/expiry, re-fetch instructions; failed/no-event fetches leave no record |
| DATA-005 | Only allow-listed compact knowledge types persist | `cargo test -p qip-events --test backbone` | ✓ PASS (60/60, 0.04s) | Topic is general registry, not allow-list; off-list world types (headlines, filings) durably retained; no allow-list enforcement at write seam |
| DATA-009 | Tick/order-book data and internal financial records in separate retention classes | `cargo test -p qip-market-ingestion --test connector_runtime` | ✓ PASS (43/43, 0.01s) | Tick data (Topic::MarketTrade) and world data (Topic::NewsReceived) share RetentionClass::Transient; no separate Tier-T class for governed tick history |
| DATA-016 | Every candidate source inspected (law, terms, security, cost, freshness, uniqueness, manipulation risk) | `cargo test -p qip-data-finder --test legality` | ✓ PASS (27/27, 0.00s) | Manipulation risk not inspected anywhere; seven production connectors bypass full assessment, admitted via StandingAdmission (licensing only) |

#### Integration Point (E2E)

| ID | Requirement | Command | Result | Coverage |
|---|---|---|---|---|
| Platform E2E walk | Source discovery through model learning in one composed cycle | `cargo test -p qip-acceptance --test e2e the_platform_walks_from_a_discovered_source_to_a_learned_lesson` | ✓ PASS (1/1, 0.06s) | Partial: uses pre-compiled strategy fixture; does not train and promote model in same run |

**Verification Total:** 6 requirements verified, 137 tests passing, 0 failures (0.18s aggregate runtime)

#### Remaining PARTIAL Requirements (26 unverified)

High-priority gaps requiring next work:

| ID | Status | Key Gap | Work Item | Size |
|---|---|---|---|---|
| DATA-002 | PARTIAL | No single pipeline type enforces stage ordering end-to-end | E08 | M |
| DATA-004 | PARTIAL | No explicit discard stage; relies on ownership-based drop | E08 | S |
| DATA-006 | PARTIAL | No historical re-fetch through manifest; campaign reads in-memory copy | E08 | M |
| DATA-007 | PARTIAL | No on-demand historical API fetch; campaign serializes held bars | E08 | M |
| DATA-010 | PARTIAL | No storage-entitlement check; venues without storage rights still retained | E08 | M |
| DATA-011 | PARTIAL | Cursor+fingerprint+deltas only for admitted polling sources, not Scout candidates | E08 | S |
| DATA-012 | NEEDS-VALIDATION | No metric measuring durable footprint vs. source size | E12 | M |
| DATA-014 | PARTIAL | Discovery cadence works; candidate discovery (CRAWL) missing | E08 | L |
| DATA-015 | PARTIAL | Source scoring misses realized strategy contribution (source alpha) | E08 | M |
| DATA-017 | PARTIAL | Polls proposed, not instantiated; human still writes adapter | E08 | L |
| DATA-018 | PARTIAL | Lifecycle transitions work (quarantine/death); promotion/throttling missing | E08 | L |
| DATA-038 | PARTIAL | No sandbox for new adapters; no retirement stage | E08 | L |

---

### MISSING (21 requirements)

**Structural Dependencies (cannot proceed without):**

| ID | Requirement | Blocker | Work Item | Horizon |
|---|---|---|---|---|
| DATA-020 | CRAWL stage for autonomous discovery | None (readiness blocking deployability, not implementation) | E08-gap-driven-discovery | v12.1 P1 |
| DATA-031 | Gap-driven discovery targets | DATA-020 | E08-gap-driven-discovery | v12.1 P1 |
| DATA-032 | Query generation (geography/entity/domain/language tags) | DATA-031 | E08-discovery-query-generation | v12.1 P1 |
| DATA-034 | Source sandbox isolation before production | None | E08-adapter-sandbox-lifecycle | v12.1 P2 |
| DATA-008 | Mark evidence unretrievable on failed re-fetch | None | E08 | v12.1 P1 |
| DATA-013 | KnowledgePack type (versioned, schema-checked) | In-tree serde/JSON (no dependency needed) | E08 | v12.1 P1 |

**Blocked on Architecture (C-level conflicts):**

| ID | Requirement | Blocker | Reason |
|---|---|---|---|
| DATA-021–026, 041–052 | 12 additional requirements | C3 (Kubernetes), C4 (GCP managed services), C8 (multi-region) | Require infrastructure capabilities not available in current deployment |

**Summary:** 6 MISSING are unblocked and ready for v12.1 implementation (P1 priority). 12 depend on GCP, Kubernetes or multi-region infrastructure decisions.

---

### BLOCKED (18 requirements)

Blocked by architecture decisions (ADR 0099 conflicts C1–C8):

- **C1 (Live Capital / External Action):** 7 requirements need live order paths or real-world actions (refused)
- **C3 (Kubernetes):** 3 requirements need GKE/Argo/service mesh
- **C4 (GCP Services):** 5 requirements need Cloud Storage, BigQuery, Vertex AI, etc.
- **C8 (Multi-Region):** 3 requirements need multiple regional deployments

---

### NEEDS-VALIDATION (3 requirements)

| ID | Requirement | Current State | Validation Gap |
|---|---|---|---|
| DATA-012 | Durable knowledge 10x smaller than source | Architecturally very likely by construction (no raw bodies retained) | No metric comparing bytes |
| DATA-024 | Chain state correctly absorbed | ChainAdapter, TraceKind, ChainAbsorption types exist | No composition-root caller; no prod wiring |
| DATA-025 | Chain observations update world model | Probing for integration; world.absorb_chain not reached from production | Integration test needed |

---

## Verified Architectural Patterns

The verified requirements confirm key architectural guarantees:

### 1. Body-Free Record Storage (DATA-001)
- **Pattern:** Fetch → FetchDigest (hash only) → DataReference (metadata + hash) → Event log
- **Assurance:** ✓ High (owned local, released on function return)
- **Evidence:** 1 test proving narrative_feed test passes; code paths in digest.rs, reference.rs, world.rs show no body field anywhere downstream

### 2. Licensing Enforcement (DATA-016, DATA-029)
- **Pattern:** Admission gate checks licensing posture before every poll
- **Implementation:** Legality (Permit/Deny/Unknown), LicensingClass, RegistrationRecord
- **Assurance:** ✓ High (gate before use)
- **Verification:** 27 tests (legality.rs) prove least-permissive combinator; reached from production (qip-api, qip-fastbrain, qip-deepbrain)

### 3. Retention Class Separation (DATA-005, DATA-009)
- **Pattern:** Topic::retention_class mandatory; RetentionClass enum with 9 rows
- **Structure:** Transient (never), Irreplaceable (permanent), EventAnchored (rolling 90 days), etc.
- **Assurance:** ✓ Medium (compiler enforcement incomplete)
- **Gap:** Tick and world data share Transient class; no separate Tier-T exists
- **Verification:** 60 tests (backbone.rs) + 43 tests (connector_runtime.rs) prove class table and separation at type level

---

## Recommendations for v12.1

**High Priority (Critical path, unblocked):**

1. **DATA-008** — Mark evidence unretrievable on failed re-fetch
   - Add Gone/Unretrievable outcome to RevisionCheck/LedgerOutcome
   - Propagate to persisted Fact/CausalEdge/belief as evidence_unretrievable flag
   - Work: M, Tier T2

2. **DATA-013** — KnowledgePack type (compact, versioned)
   - Implement as serde/JSON-based versioned pack with schema validation
   - One type per content kind (world, features, beliefs, strategies)
   - Round-trip test + out-of-schema refusal test
   - Work: L, Tier T4

3. **DATA-020** — CRAWL stage (autonomous source discovery)
   - Read forecast-error spikes and knowledge gaps from world model
   - Generate discovery targets with geography/entity/domain/language tags
   - Feed into Scout Fabric (DATA-031, DATA-032)
   - Work: L, Tier T2 (but gates DATA-031, DATA-032, DATA-038 E2E)

**Medium Priority (Important, blocked by DATA-020):**

4. **DATA-031** — Gap-driven discovery targets
5. **DATA-032** — Query generation with semantic tags
6. **DATA-034** — Adapter sandbox isolation before promotion

**Testing Work (Enable next-phase verification):**

- DATA-012: Add footprint benchmark (durable bytes vs. source bytes)
- DATA-024, DATA-025: Wire ChainAdapter into composition roots; add integration test
- E2E full Scout pipeline: Once DATA-034 lands, write end-to-end from discovery through retirement

**Infrastructure Work (Deployment activation):**

- Enable `deepbrain_discover_every` and `source_candidates_file` in dev/test environments
- Activate GCS archive buckets (enable_cloud_storage = true) and wire storage provider
- Create dedicated scout Cloud Run job (currently runs inside deepbrain process)

---

## Verification Audit Trail

**Run Date:** 2026-10-06  
**Environment:** Linux, Rust 2024, workspace cargo test (no-fail-fast)  
**Operator:** Claude Haiku 4.5  
**Commands Executed:** 6 verification suites + E2E walk  
**Results:** 137 tests passed, 0 failed, 0 ignored  
**Total Time:** ~0.18s cargo commands + 0.06s E2E

**Test Suites:**
- narrative_feed (qip-market-ingestion): 1 test, 0.00s
- references (qip-kernel): 5 tests, 0.02s
- backbone (qip-events): 60 tests, 0.04s
- connector_runtime (qip-market-ingestion): 43 tests, 0.01s
- legality (qip-data-finder): 27 tests, 0.00s
- e2e (qip-acceptance): 1 test, 0.06s

**Mutation Verification:** All new tests in prior session verified (mutations break the tests for the right reasons).

---

**Documentation Verified By:** Command execution, not assertion  
**Last Updated:** 2026-10-06 (this document)  
**Next Review:** On v12.1 release cycle or when framework changes
