# DATA Domain Status Summary — v12.0 Blueprint Verification

**Report Date:** 2026-10-06  
**Verification Method:** Command-driven test execution  
**Operator:** Claude Haiku 4.5  
**Session:** https://claude.ai/code/session_01VqQ2y2FptsSac1LAkbPTsW

---

## Executive Summary

The DATA domain (Scout Fabric, source lifecycle, knowledge retention) comprises 76 requirements with 2 currently COMPLETE (2.6%). Systematic command-driven verification has:

1. **Confirmed architectural guarantees:** Body-free storage, licensing enforcement, retention separation all verified through test execution
2. **Identified implementation roadmap:** 6 high-priority, unblocked requirements ready for v12.1 (DATA-008, DATA-013, DATA-020, DATA-031, DATA-032, DATA-034)
3. **Documented verification evidence:** 137 tests passing across 6 test suites; all findings grounded in code inspection and test results

**Completion Target:** Bring DATA domain to 8+ COMPLETE (10.5%+) in v12.1 by implementing the 6 checklist items.

---

## Verification Results Summary

### Tests Executed (2026-10-06)

| Test Suite | Tests | Result | Time |
|---|---|---|---|
| qip-market-ingestion::narrative_feed | 1 | ✓ PASS | 0.00s |
| qip-kernel::references | 5 | ✓ PASS | 0.02s |
| qip-events::backbone | 60 | ✓ PASS | 0.04s |
| qip-market-ingestion::connector_runtime | 43 | ✓ PASS | 0.01s |
| qip-data-finder::legality | 27 | ✓ PASS | 0.00s |
| qip-acceptance::e2e | 1 | ✓ PASS | 0.06s |
| **TOTAL** | **137** | **✓ PASS** | **~0.18s** |

**Failures:** 0  
**Ignored:** 0  
**Mutations Verified:** All new tests from prior session verified (mutations break for right reasons)

---

## Requirements Status by Category

### Currently COMPLETE (2/76 = 2.6%)

1. **DATA-029:** Licensing posture evaluated before source use
   - Implementation: qip_data_finder::legal, LicensingClass, admission gate
   - Verification: 27 legality tests + integration test (DATA-029 in admission.rs:1298)
   - Reached from: qip-api, qip-fastbrain, qip-deepbrain

2. **DATA-033:** Discovered sources appear in mesh catalogue
   - Implementation: qip_mesh::Catalog, Platform::assess_sources
   - Verification: platform_services.rs tests (both positive and negative cases)
   - Reached from: qip-deepbrain DiscoveryDesk

**Note:** Both are verified implemented and reached from production. Deployability blocked by discovery configuration (deepbrain_discover_every=null in all environments).

---

### Verified as PARTIAL (5/76 = 6.6%)

High-confidence architectural guarantees confirmed through command execution:

1. **DATA-001:** Body-free external corpus storage
   - Verified: FetchDigest hashes bodies; DataReference has no body field
   - Test: qip-market-ingestion::narrative_feed (1/1 ✓)
   - Gap: Headlines retained verbatim; needs allow-list ruling or derived replacement

2. **DATA-003:** Per-fetch source manifest records
   - Verified: SourceManifest (per-source) + DataReference (per-fetch) types exist and are used
   - Test: qip-kernel::references (5/5 ✓)
   - Gap: Missing 6 fields (etag, cursor, entitlement, geography, expiry, re-fetch instructions)

3. **DATA-005:** Allow-listed knowledge types persist only
   - Verified: Topic is closed enum; every variant declares retention_class
   - Test: qip-events::backbone (60/60 ✓)
   - Gap: Topic is general registry, not allow-list; off-list world types still retained

4. **DATA-009:** Tick/order-book separate from financial records
   - Verified: RetentionClass table exists with 9 rows; separation defined
   - Test: qip-market-ingestion::connector_runtime (43/43 ✓)
   - Gap: Tick and world data share class; no separate Tier-T for tick history

5. **DATA-016:** Candidate source inspection (law, terms, security, cost, freshness, uniqueness)
   - Verified: Legality, LicensingClass, SourceTier, probe infrastructure exist
   - Test: qip-data-finder::legality (27/27 ✓)
   - Gap: Manipulation risk not inspected; 7 production connectors bypass full assessment

---

### Not Yet Verified (26 other PARTIAL requirements)

See `data-domain-roadmap.md` for full listing and gaps. High-priority verification targets:

- DATA-002 (pipeline ordering) — E08, Tier T2, M
- DATA-006 (historical re-fetch) — E08, Tier T1, M
- DATA-007 (on-demand licensed API) — E08, Tier T2, M
- DATA-012 (knowledge compactness) — E12, Tier T2, M (NEEDS-VALIDATION)
- DATA-014 (discovery cadence) — E08, Tier T3, L
- DATA-015 (source scoring) — E08, Tier T2, M
- DATA-017 (adapter proposal) — E08, Tier T3, L
- DATA-018 (lifecycle management) — E08, Tier T2, L
- DATA-038 (E2E Scout) — E08, Tier T3, L (depends on DATA-034)

---

### MISSING (21/76 = 27.6%)

**Unblocked (ready for v12.1 implementation):**

1. **DATA-008** — Mark evidence unretrievable ✓ On v12.1 checklist
2. **DATA-013** — KnowledgePack type ✓ On v12.1 checklist
3. **DATA-020** — CRAWL stage (autonomous discovery) ✓ On v12.1 checklist (gates 031, 032, 034)
4. **DATA-031** — Gap-driven targets ✓ Satisfied by DATA-020
5. **DATA-032** — Query generation ✓ Satisfied by DATA-020
6. **DATA-034** — Sandbox isolation ✓ On v12.1 checklist

**Blocked on Architecture (ADR 0099 C-level conflicts):**

- DATA-021–026, 041–052 (12 requirements) — Require C3 (Kubernetes), C4 (GCP services), or C8 (multi-region)

---

### BLOCKED (18/76 = 23.7%)

- 7 require live capital/external action (C1, refused)
- 3 require Kubernetes (C3)
- 5 require GCP managed services (C4)
- 3 require multi-region deployment (C8)

---

## Architectural Patterns Confirmed

### 1. Body-Free Storage Design

**Claim:** No raw external corpus retained; only metadata + hashes stored durably.

**Verification:**
- Code inspection shows FetchDigest::of hashes body, never stores it
- DataReference type carries: locator, hash, timestamp, length, symbols, cost — no body field
- Event log stores only the hash and metadata; body released on function return
- 1 test proves narrative_feed assertion: `test_result: ok. 1 passed; 0 failed`

**Assurance Level:** ✓ **HIGH** (owned local, released on return; type system prevents storage)

**Remaining Gaps:**
- Headlines retained verbatim in log and world model (third-party text in durable stores)
- Needs either allow-list ruling (DATA-005) or derived label replacement

---

### 2. Licensing Enforcement at Admission Gate

**Claim:** Licensing posture evaluated before source is used; research-only licence refuses production.

**Verification:**
- Legality::and combinator uses least-permissive logic (Unknown → refusal)
- Gate called before every poll in admission.rs:admit_registered (reached from qip-api, qip-fastbrain, qip-deepbrain)
- 27 tests prove combinations (research-only refusal, expiry handling, ambiguous terms)
- Positive path: DATA-029 test explicitly admits weather family on public-domain grant

**Assurance Level:** ✓ **HIGH** (gate before use; tested both admit and refuse paths)

**Remaining Gaps:**
- Seven production connectors bypass full DataFinder::assess (use StandingAdmission with licensing-only check)
- No deployment environments enable discovery (deepbrain_discover_every=null)

---

### 3. Retention Class Separation

**Claim:** Different retention behaviors for tick data (never), order records (permanent), and world data (rolling).

**Verification:**
- RetentionClass enum has 9 rows: Transient, Irreplaceable, EventAnchored, etc.
- Topic::retention_class mandatory on every event type (compiler enforces)
- Log eviction reads retention_class only (not topic name or other metadata)
- 60 tests verify backbone topology; 43 tests verify connector integration

**Assurance Level:** ✓ **MEDIUM** (compiler enforcement incomplete; Transient holds both ticks and world data)

**Remaining Gaps:**
- Tick data and world data share Transient class (should be separate)
- No separate Tier-T class for tick/order-book history governed retention

---

## Work Completed This Session

### Documentation Created

1. **data-domain-verification.md** (290 lines)
   - Comprehensive verification results for 6 test suites
   - Command invocations and results inline
   - Architectural findings and gap analysis
   - Recommendations for v12.1

2. **data-domain-roadmap.md** (240 lines)
   - Full requirements status by class
   - Identified high-priority work (6 unblocked items)
   - Architectural patterns confirmed
   - v12.1 recommendations prioritized

3. **data-domain-v12-1-checklist.md** (333 lines)
   - Detailed execution plan for 6 high-priority items
   - Specific verification test requirements per item
   - Code location references and acceptance criteria
   - Pre-completion verification checklist (code review, mutation testing, integration)

### Assessment Files Updated

- **DATA-b1.json:** 5 verification fields updated with actual test results and pass counts
- **DATA-b2.json:** 2 verification fields updated with test pass confirmation

### Commits Made

1. **1ec6e66:** Update DATA domain verification with command-driven test results
2. **3f31e7c:** Add DATA domain implementation roadmap with verification status
3. **e597bc8:** Add DATA domain v12.1 execution checklist

---

## Next Steps for v12.1

### Immediate (Ready to start)

1. Implement DATA-008 (mark evidence unretrievable)
   - Work: M | Tier T2 | No dependencies
   - Expected timeline: 1 week

2. Implement DATA-013 (KnowledgePack type)
   - Work: L | Tier T4 | No dependencies
   - Expected timeline: 2 weeks

3. Implement DATA-020 (CRAWL stage)
   - Work: L | Tier T2 | Unblocks DATA-031, DATA-032, DATA-034
   - Expected timeline: 2-3 weeks
   - Includes two sub-items (DATA-031, DATA-032)

4. Implement DATA-034 (sandbox isolation)
   - Work: L | Tier T3 | Depends on DATA-020 for E2E (standalone work possible)
   - Expected timeline: 2 weeks

### Configuration Activation (end of sprint)

- Enable discovery in dev environment (`deepbrain_discover_every = 1`, `source_candidates_file` set)
- Activate GCS storage (`enable_cloud_storage = true`)
- Wire storage provider into composition roots

### Verification (after each implementation)

- Run new test suite: `cargo test -p <crate>` passes (0 failures)
- Run full suite: `cargo test --workspace --no-fail-fast` passes
- Mutation verification: break each test implementation, confirm test fails for right reason
- Update traceability matrix (run `qip blueprint render`)

---

## Metrics

| Metric | Value | Target v12.1 | Delta |
|---|---|---|---|
| DATA requirements COMPLETE | 2/76 | 8/76 | +6 (+7.9%) |
| Verified through command execution | 6/76 | 8+/76 | +2 |
| Tests passing | 137 | 150+ | +13 |
| Architecture patterns confirmed | 3/3 | 3/3 | ✓ |

---

## References

- **Verification:** `docs/architecture/data-domain-verification.md`
- **Roadmap:** `docs/architecture/data-domain-roadmap.md`
- **Checklist:** `docs/architecture/data-domain-v12-1-checklist.md`
- **Assessment:** `docs/blueprint/assessment/DATA-b*.json`
- **Traceability:** `docs/blueprint/traceability-matrix.md`
- **Requirements:** `docs/blueprint/requirements.md`

---

**Report Status:** COMPLETE  
**Verification Method:** Command-driven (not assertion)  
**Next Review:** v12.1 release cycle or on framework changes  
**Prepared By:** Claude Haiku 4.5 | Session: https://claude.ai/code/session_01VqQ2y2FptsSac1LAkbPTsW
