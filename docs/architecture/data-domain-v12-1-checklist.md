# DATA Domain v12.1 Execution Checklist

**Owner:** Platform Team  
**Priority:** P1 (foundation phase)  
**Target Completion:** v12.1 release cycle  
**Last Updated:** 2026-10-06

## Overview

This checklist tracks implementation and verification work for 6 high-priority, unblocked DATA domain requirements needed to close the Scout Fabric foundation phase and enable autonomous source discovery.

**Status:** 0/6 complete (0%)

---

## High-Priority Work Items

### 1. DATA-008: Mark Evidence Unretrievable on Failed Re-fetch

**Priority:** P1  
**Complexity:** M  
**Tier:** T2  
**Blocker for:** DATA-006 (historical re-fetch verification), DATA-051 (knowledge durability)

#### Requirements

- [ ] Add `Unretrievable` outcome to `RevisionCheck` enum
- [ ] Add `Unretrievable` variant to `LedgerOutcome` enum
- [ ] Add `evidence_unretrievable: bool` field to persisted `Fact` type
- [ ] Add `evidence_unretrievable: bool` field to persisted `CausalEdge` type
- [ ] Add `evidence_unretrievable: bool` field to persisted belief types
- [ ] Wire failed re-fetch outcome (404/withdrawn/embargoed) into the unretrievable flag

#### Verification

- [ ] Unit test: `a_re_fetch_that_errors_marks_the_reference_unretrievable`
  - Fixture DataReference with manual error response
  - Assert RevisionCheck returns Unretrievable
  - Assert dependent Fact carries unretrievable=true
- [ ] Integration test: `a_source_that_stops_answering_marks_all_evidence_unretrievable`
  - Fixture source withdrawn mid-session
  - Poll fails at re-fetch
  - Assert event log records Unretrievable outcome
  - Assert all downstream knowledge carries the flag

#### Evidence Location

- Code: `backend/crates/services/qip-data-finder/src/reference.rs` (RevisionCheck)
- Code: `backend/crates/services/qip-data-finder/src/ledger.rs` (LedgerOutcome)
- Code: `backend/crates/libs/qip-financial/src/intelligence.rs` (Fact type)
- Code: `backend/crates/libs/qip-contracts/src/causal.rs` (CausalEdge)

#### Acceptance Criteria

- [ ] RevisionCheck::Unretrievable compiles
- [ ] All three tests pass
- [ ] Mutation: delete flag assignment → tests fail (for the right reason)
- [ ] `cargo test -p qip-data-finder --test reference` passes (existing)
- [ ] Unretrievable outcome flows through to world model

---

### 2. DATA-013: KnowledgePack Type (Versioned, Compact Artifacts)

**Priority:** P1  
**Complexity:** L  
**Tier:** T4  
**Blocker for:** DATA-012 (compact knowledge metric), DATA-035 (SourceSpec/FeedPack artifacts)

#### Requirements

- [ ] Define `KnowledgePack` type with version field: `{ version: u32, schema_version: u32, content_kind: ContentKind, body: Vec<u8> }`
- [ ] ContentKind enum: WorldSnapshot, FeatureStore, StrategyWeights, BeliefBase, TickArchive
- [ ] Implement schema validation: refuse packs carrying extra/unknown fields
- [ ] Round-trip test: pack → serde_json → pack (byte-for-byte)
- [ ] Refusal test: corrupted pack with extra field rejected

#### Verification

- [ ] Unit test: `a_knowledge_pack_round_trips_serde_json_byte_for_byte`
  - Create fixture pack
  - Serialize to JSON, deserialize
  - Assert binary identical
- [ ] Unit test: `a_pack_with_an_unknown_field_is_refused_on_deserialize`
  - Valid pack JSON with extra field added
  - Deserialize fails with unknown field error
  - Assert neither partial nor silent acceptance
- [ ] Property test: `every_pack_content_kind_round_trips_with_its_own_schema_version`
  - Fixture for each ContentKind
  - Serialize/deserialize each
  - Assert version preserved

#### Evidence Location

- New file: `backend/crates/libs/qip-core/src/pack.rs`
- Tests: `backend/crates/libs/qip-core/tests/pack.rs`

#### Acceptance Criteria

- [ ] `KnowledgePack` compiles with all ContentKind variants
- [ ] All three tests pass
- [ ] Mutation: delete schema_version field → round-trip test fails
- [ ] Mutation: remove unknown-field rejection → refusal test fails
- [ ] Integration: Pack type can be instantiated for each knowledge class

---

### 3. DATA-020: CRAWL Stage (Autonomous Gap-Driven Source Discovery)

**Priority:** P1 (gates DATA-031, DATA-032, DATA-038 E2E)  
**Complexity:** L  
**Tier:** T2  
**Blocker for:** Scout Fabric autonomy, adaptive source portfolio

#### Requirements

- [ ] Add discovery target generation from forecast-error spikes (DATA-031)
- [ ] Add query generation with semantic tags: geography, entity, domain, language (DATA-032)
- [ ] Wire forecast errors from world model into discovery pipeline
- [ ] Connect output to DiscoveryDesk candidate queue
- [ ] Add configuration: `discovery_gap_threshold` (forecast error σ), `discovery_rate_limit` (queries/cycle)

#### Verification

- [ ] Unit test: `an_uncovered_entity_with_a_forecast_error_spike_generates_a_discovery_target`
  - Fixture: world model with Fact(coverage_gap=true, forecast_error>2σ)
  - Call: Platform::generate_discovery_targets()
  - Assert: DiscoveryTarget generated with entity name, region, domain
- [ ] Unit test: `every_generated_query_carries_all_four_semantic_tags`
  - Fixture: DiscoveryTarget from above
  - Call: query_generator::generate(target)
  - Assert: Query.geography != empty, Query.entity != empty, Query.domain != empty, Query.language != empty
- [ ] Integration test: `gap_driven_discovery_populates_the_candidate_queue`
  - Fixture: 100 queries generated
  - Assert: DiscoveryDesk.candidate_queue has all 100
  - Assert: each candidate carries source metadata (name, access_class)

#### Evidence Location

- New file: `backend/crates/runtime/qip-kernel/src/discovery.rs` (gap reader)
- New file: `backend/crates/services/qip-data-finder/src/query.rs` (query generator)
- Modify: `backend/crates/apps/qip-deepbrain/src/discovery.rs` (wire in)

#### Acceptance Criteria

- [ ] All three tests pass
- [ ] Mutation: remove gap threshold check → first test fails
- [ ] Mutation: skip tag assignment → second test fails
- [ ] Integration: 100 gaps → 100+ candidates in queue (no loss)
- [ ] Configuration: discovery_gap_threshold and discovery_rate_limit read and enforced

---

### 4. DATA-031: Gap-Driven Discovery Targets

**Priority:** P1 (depends on DATA-020)  
**Complexity:** M  
**Tier:** T2  

This is satisfied as part of DATA-020 implementation. See DATA-020's Unit Test #1.

#### Acceptance Criteria

- [ ] Forecast error spike detection working
- [ ] DiscoveryTarget type instantiated per gap
- [ ] No forecast-error→discovery path missed in code review

---

### 5. DATA-032: Discovery Query Generation (Semantic Tags)

**Priority:** P1 (depends on DATA-020)  
**Complexity:** M  
**Tier:** T2  

This is satisfied as part of DATA-020 implementation. See DATA-020's Unit Test #2.

#### Acceptance Criteria

- [ ] Query generator assigns all four tags
- [ ] Tag assignments come from EntityMetadata/RegionConfig/DomainRegistry sources
- [ ] No tag can be empty/null on output

---

### 6. DATA-034: Adapter Sandbox Isolation Before Promotion

**Priority:** P1 (gates Scout E2E)  
**Complexity:** L  
**Tier:** T3  
**Blocker for:** DATA-038 (full Scout pipeline E2E)

#### Requirements

- [ ] Add `LifecycleStage::Sandbox` to adapter lifecycle
- [ ] Create isolated feed target: `FeedTarget::Sandbox { session_id: String, ttl: Duration }`
- [ ] Sandbox feed writes to ephemeral knowledge store (not world model)
- [ ] Add promotion gate: `DataFinder::promote_adapter(adapter_id, sandbox_session_id) → Result<AdapterId>`
- [ ] Promotion requires: (1) sandbox feed ran, (2) no errors, (3) knowledge count > threshold, (4) operator approval
- [ ] Add `PromotionRecord { adapter_id, sandbox_session, evidence_count, promoted_at, promoted_by }`

#### Verification

- [ ] Integration test: `a_newly_registered_adapter_runs_in_sandbox_not_production`
  - Register fixture adapter
  - Assert first fetch uses FeedTarget::Sandbox
  - Assert knowledge writes to sandbox store, not world model
  - Assert sandbox TTL expires after inactivity window
- [ ] Integration test: `an_adapter_promoted_after_sandbox_success_reaches_production_feeds`
  - Run sandbox (above)
  - Approve promotion
  - Assert subsequent fetch uses FeedTarget::Production
  - Assert knowledge writes to world model
- [ ] Security test: `sandboxed_knowledge_is_never_visible_to_decision_paths`
  - Load sandbox knowledge into world model query
  - Query fails or returns empty despite knowledge existing in sandbox

#### Evidence Location

- Modify: `backend/crates/services/qip-data-finder/src/decision.rs` (add Sandbox stage)
- Modify: `backend/crates/services/qip-market-ingestion/src/connector/runtime.rs` (FeedTarget enum)
- New file: `backend/crates/services/qip-data-finder/src/promotion.rs` (PromotionRecord)

#### Acceptance Criteria

- [ ] All three tests pass
- [ ] Mutation: remove TTL check → first test fails (sandbox persists incorrectly)
- [ ] Mutation: skip promotion gate → second test fails (sandbox feed reaches production)
- [ ] Mutation: remove query filter → third test fails (sandboxed knowledge visible)
- [ ] Manual test: operator can view sandbox session logs and decline promotion
- [ ] Configuration: sandbox_ttl and sandbox_knowledge_threshold read and enforced

---

## Supporting Work

### Documentation Updates (run after each checklist item)

- [ ] Update `docs/architecture/data-domain-verification.md` with verification results
- [ ] Update `docs/blueprint/assessment/DATA-b*.json` with e2e flag on each completed requirement
- [ ] Update `docs/blueprint/traceability-matrix.md` (automated on render, but check for changes)

### Configuration Activation (run once at end)

- [ ] Set `deepbrain_discover_every = 1` (discover on every cycle) in `environments/dev/terraform.tfvars`
- [ ] Set `source_candidates_file = "data/discovery/candidate-fixtures.json"` in dev tfvars
- [ ] Set `discovery_gap_threshold = 2.0` (σ) in dev tfvars (new variable)
- [ ] Set `sandbox_ttl = 3600` (seconds) in dev tfvars (new variable)
- [ ] Verify: `terraform plan -var-file=environments/dev/terraform.tfvars` succeeds

### Testing Activation

- [ ] Add `backend/crates/tests/qip-acceptance/tests/scout-fabric.rs` (new cross-cutting test)
- [ ] Test runs: `the_platform_discovers_a_gap_driven_source_and_sandboxes_it_before_promotion`
- [ ] Test runs: `an_operator_can_decline_sandbox_promotion_and_retire_the_adapter`

---

## Verification Checklist

Before marking DATA-020/031/032/034 COMPLETE:

### Code Review

- [ ] Reviewer 1 reads implementation
- [ ] Reviewer 2 reads tests
- [ ] Reviewer 3 reads configuration/wiring
- [ ] No TODOs/unimplemented! left in production code
- [ ] No unwrap() outside tests
- [ ] All error paths named (no generic "Error" types)

### Test Execution

- [ ] `cargo fmt --all --check` passes
- [ ] `cargo clippy --workspace --all-targets` zero warnings
- [ ] `cargo test -p <crate> --no-fail-fast` passes for affected crates
- [ ] `cargo test -p qip-acceptance --test scout-fabric` passes
- [ ] `cargo test --workspace --no-fail-fast` passes (full suite)

### Mutation Testing (per new test)

- [ ] Each new test mutates and fails for the right reason
- [ ] Break implementation → test breaks ✓
- [ ] Restore → test passes ✓
- [ ] Reported in commit message

### Integration

- [ ] Code reaches production: `grep -rn 'promote\|CRAWL\|Sandbox' backend/crates/apps/qip-deepbrain/src/` returns results
- [ ] Configuration read at startup: `QIP_DEEPBRAIN_DISCOVERY_GAP_THRESHOLD` (env var) or tfvars
- [ ] `architecture.rs` acceptance test passes: `the_adapter_lifecycle_enforces_sandbox_before_production`

### Documentation

- [ ] Commit message explains why (not what)
- [ ] ADR updated if architectural decision made (e.g. sandbox store lifetime)
- [ ] `data-domain-verification.md` updated with test results
- [ ] `data-domain-roadmap.md` updated: DATA-020/031/032/034 moved to COMPLETE

---

## Success Criteria (v12.1 Complete)

**Gate:** All 6 checklist items marked done AND all verification checks pass

When complete:
- Scout Fabric autonomy enabled (gap-driven discovery running)
- Adapter lifecycle includes sandbox isolation
- All 4 new requirements (DATA-008, DATA-013, DATA-020, DATA-034) are COMPLETE
- Platform can discover sources, sandbox-test, and promote without operator intervention
- Data domain completion: 6 → 34 COMPLETE (6/76 = 7.9%)

---

## Open Questions

1. **Sandbox knowledge storage backend:** Ephemeral in-process map, or temp Redis key with TTL?
   - Recommendation: In-process BTreeMap; no storage dependency
   - Test: Verify memory is released on TTL expiry

2. **Promotion operator identity:** How does approval flow in automated environment?
   - Recommendation: AutonomyLevel::supervised_autonomy_request with operator_id field
   - See GOV domain requirements for authority envelope

3. **Discovery query bias:** How to avoid flooding candidate list with high-variance entities?
   - Recommendation: discovery_rate_limit AND uniqueness_scored_candidates (deduplicate by entity)
   - Test: 1000 forecast errors → candidate_count < 100 (rate limited)

---

**Last Updated:** 2026-10-06  
**Next Review:** On DATA-020 implementation start  
**Owner:** Platform Team / Scout Fabric Lead
