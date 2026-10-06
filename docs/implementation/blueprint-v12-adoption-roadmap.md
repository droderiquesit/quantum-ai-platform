# Blueprint v12.0 Adoption Roadmap

**Status**: Planning Phase  
**Initiated**: 2026-10-06  
**Architecture of Record**: ADR 0107 (Proposed)

## Overview

This roadmap tracks the transition from Blueprint v11.6 (current) to v12.0 (target). ADR 0107 establishes the adoption framework; this document is the implementation plan.

The roadmap is organized by:
1. **Phases** — sequential milestones
2. **Critical Path Items** — gating dependencies
3. **ADR Requirements** — architectural decisions needed before code changes
4. **Component Implementation** — new systems to build

## Phase 0: Blueprint Preparation (Pending)

**Condition**: v12.0 and v3.0 blueprints received, SHA-256 verified, and locked

### Deliverables

- [ ] v12.0 blueprint document (PDF + text extract)
- [ ] v3.0 blueprint document (PDF + text extract)
- [ ] Interactive architecture diagram (if provided)
- [ ] Stored in `docs/blueprint/source/` with verified SHA-256 hashes
- [ ] Updated `docs/blueprint/README.md` with new source entries

### Acceptance Criteria

- Both documents and SHA-256 hashes recorded
- Blueprint README updated and committed
- Difference analysis completed against v11.6

## Phase 1: Traceability Establishment (Blocking on Phase 0)

**Owner**: Architecture Team  
**Duration**: 2-3 weeks estimated

### Deliverables

- [ ] Decompose v12.0 and v3.0 into atomic requirements
- [ ] Assign `DOMAIN-NNN` IDs per component
- [ ] Generate `docs/blueprint/requirements/*.json` files
- [ ] Render `docs/blueprint/requirements.md`
- [ ] Create initial `docs/blueprint/traceability-matrix.md`

### Acceptance Criteria

- Every blueprint section has an associated requirement ID
- Matrix rows created for all requirements
- Initial status: all requirements marked `PENDING` (not yet started)

## Phase 2: Conflict Resolution Planning (Parallel with Phase 1)

**Owner**: Architecture Review Board  
**Duration**: 1-2 weeks estimated

### Deliverables

- [ ] Compile conflict register: v12.0 standing decisions vs. ADRs 0001-0100
- [ ] Categorize conflicts:
  - **Owner-only** (requires new policy amendment)
  - **Dependencies** (ADR 0002/0009 class, blocked by C2)
  - **Architecture** (new core decisions needed)
  - **Platform** (GCP/v3.0 specific, blocked by C3/C4)

### Known Conflicts (From v11.6 v12.0 Structure)

| Component | Conflict | Status | ADR Needed |
|-----------|----------|--------|-----------|
| Scout Fabric | New active intelligence sensor | Architecture | ADR-SCOUT-001 |
| Evidence/Truth | Knowledge verification subsystem | Architecture | ADR-EVIDENCE-001 |
| Tick Lake | Temporal market state archive | Storage (C4 blocker) | ADR-TICKLAKE-001 |
| World Model | Distributed reasoning federation | Architecture | ADR-WORLDMODEL-001 |
| Causal Agency | Autonomous decision publication | Autonomy (C1 concern) | ADR-AGENCY-001 |
| Expansion Engine | Opportunity search system | Architecture | ADR-EXPANSION-001 |
| NOW Brain | Temporal reasoning | Architecture | ADR-NOWBRAIN-001 |
| Forecast Lattice | Temporal reasoning structure | Architecture | ADR-LATTICE-001 |
| Model Tournament | Ensemble benchmarking | Architecture | ADR-TOURNAMENT-001 |
| Reflex Mesh (P2P) | Cell-to-cell communication | Architecture | ADR-REFLEXMESH-001 |
| Federated Brains | Distributed expertise | Architecture | ADR-FEDBRAINS-001 |
| Async Quantum | Deferred quantum compute | Architecture | ADR-QUANTUM-001 |
| Risk Controls | Regional autonomy systems | Autonomy (C1/architecture) | ADR-RISKCTRL-001 |
| Event Fabric Consolidation | Unified streaming migration | Migration | ADR-FABRICONV-001 |
| Warm Tier | Service orchestration layer | Platform (GCP-specific) | ADR-WARMTIER-001 |

## Phase 3: Critical ADR Decisions (Gating)

**Owner**: Architecture + Engineering  
**Duration**: 3-4 weeks estimated  
**Blocker**: Phase 2 must complete first

### ADRs to Write and Accept

| Priority | ADR ID | Title | Estimated Effort | Dependencies |
|----------|--------|-------|------------------|--------------|
| **P0** | 0102 | Warm-Tier Coordination Layer | 2d | GCP v3.0 blueprint |
| **P0** | 0103 | Peer-to-Peer Reflex Mesh | 3d | Cell communication model |
| **P0** | 0104 | Federated Specialist Brains | 3d | Brain placement policy |
| **P1** | 0105 | Async Quantum Compute | 2d | Quantum job model |
| **P1** | 0106 | Independent Risk Control Systems | 3d | Autonomy boundaries |
| **P1** | 0107 | Event Fabric Consolidation | 2d | Current fabric assessment |
| **P1** | 0108 | NOW Brain & Forecast Lattice | 3d | Temporal reasoning model |
| **P2** | 0109 | Model Tournament Infrastructure | 2d | Ensemble strategy |
| **P2** | 0110 | Scout Fabric (Active Intelligence) | 2d | Sensor architecture |
| **P2** | 0111 | Evidence/Truth Subdomain | 2d | Knowledge verification |
| **P2** | 0112 | Tick Lake (Temporal Archive) | 2d | Storage backend (waits on C4) |
| **P2** | 0113 | World Model Federation | 2d | Federation protocol |
| **P2** | 0114 | Causal Agency Surface | 2d | Autonomy gates (waits on C1) |
| **P2** | 0115 | Expansion Engine | 2d | Opportunity search |

### Acceptance Gate for Each ADR

- [ ] Architecture Review Board consensus (2-0 minimum)
- [ ] Paper-trading boundary verified intact
- [ ] Conflict resolution explicit (named in ADR)
- [ ] Cost and reversal conditions clearly stated
- [ ] No live-order path introduced

## Phase 4: Architecture Documentation Update (Parallel with Phase 3)

**Owner**: Technical Writers  
**Duration**: 2 weeks estimated

### Deliverables

- [ ] Update `docs/architecture/current-state.md` with v12.0 components
- [ ] Update `docs/architecture/target-state.md` with v12.0 placement
- [ ] Update `docs/architecture/lane-placement.md` with new lanes
- [ ] Update `docs/architecture/current-state/*.json` component models
- [ ] Add `.claude/rules/architecture/v12-adoption.md` with transition rules

### Current State Additions

For each new v12.0 component:
- Implementation status (not started, in progress, complete)
- Current equivalent in v11.6 (if any)
- Required ADR decision
- Test coverage status
- Deployment readiness

## Phase 5: Component Implementation (Follows Conflict Resolution)

**Owner**: Engineering Teams  
**Duration**: 12-16 weeks estimated  
**Blocker**: All P0 ADRs must be accepted

### Implementation Sequencing

#### Wave 1 (Weeks 1-4): Coordination Infrastructure
- Warm-Tier Coordination Layer
- Event Fabric Consolidation prep
- Service mesh updates

#### Wave 2 (Weeks 5-8): Core Reasoning
- Federated Specialist Brains
- Peer-to-Peer Reflex Mesh
- NOW Brain & Forecast Lattice

#### Wave 3 (Weeks 9-12): Specialized Systems
- Model Tournament Infrastructure
- Scout Fabric
- Expansion Engine

#### Wave 4 (Weeks 13-16): Advanced Features
- Async Quantum Compute
- Evidence/Truth Subdomain
- Tick Lake
- World Model Federation
- Causal Agency Surface

### Per-Component Acceptance Criteria

For each component, before scoring `COMPLETE` in the traceability matrix:

1. **Design Review** — ADR accepted + implementation spec approved
2. **Code Review** — All gates pass (`cargo fmt`, `clippy`, tests, dependencies)
3. **Test Coverage** — Unit tests + acceptance suite for that component
4. **Integration Test** — Cross-cutting suite includes this component's behavior
5. **Documentation** — README, ADR reference, operational guidance
6. **Mutation Verification** — Every new test breaks when implementation is broken
7. **Performance Baseline** — Latency and throughput measured and recorded

## Phase 6: Blueprint Traceability Validation (Rolling)

**Owner**: QA + Architecture  
**Duration**: Continuous through Phase 5

### Deliverables (Updated After Each Wave)

After each wave completes:
- [ ] Run `qip blueprint render` to regenerate traceability matrix
- [ ] Verify completed requirements are marked `COMPLETE` with test evidence
- [ ] Update `docs/blueprint/traceability-matrix.md`
- [ ] Commit with evidence (test run, coverage report)

### Metrics to Track

- Requirements completed per week
- Test coverage trend
- Blockers resolved
- ADR decisions made
- Components in production

## Phase 7: Migration Complete (Target)

**Success Criteria**:
- All v12.0 requirements either COMPLETE or BLOCKED (with stated reason)
- At least 85% of requirements scored COMPLETE
- Paper-trading boundary verified intact across all new components
- All production acceptance tests passing
- v11.6 components deprecated or migrated
- Zero dangling conflicts without assigned ADRs

## Critical Path Sequence

```
Phase 0: Blueprints Received
    ↓
Phase 1: Traceability ←→ Phase 2: Conflict Analysis
    ↓
Phase 3: ADR Decisions (P0/P1/P2)
    ↓
Phase 4: Architecture Docs (Parallel with Phase 3 back-half)
    ↓
Phase 5: Implementation (Waves 1-4)
    ↓
Phase 6: Traceability Validation (Rolling)
    ↓
Phase 7: Migration Complete
```

**Estimated Total Duration**: 20-24 weeks (5-6 months)

## Resource Allocation

| Role | Estimated Effort | Duration |
|------|-----------------|----------|
| Architecture Review Board | 4 weeks | Phases 2-3 |
| Engineering Teams | 60 weeks | Phases 4-5 |
| QA/Test | 8 weeks | Phases 5-6 |
| Technical Writing | 2 weeks | Phase 4 |
| Platform/DevOps | 4 weeks | Phase 5 integration |

**Total**: ~80 person-weeks of effort

## Gating Criteria

### Before Phase 1 Starts
- [ ] v12.0 and v3.0 blueprints received and locked
- [ ] SHA-256 hashes verified
- [ ] Stored in `docs/blueprint/source/`

### Before Phase 3 Starts
- [ ] Traceability matrix generated
- [ ] Conflict register complete
- [ ] Initial decomposition validated

### Before Phase 5 Starts
- [ ] All P0 ADRs accepted
- [ ] Paper-trading boundary re-verified
- [ ] Resource allocation confirmed

### Before Phase 7 Closes
- [ ] 85% traceability achieved
- [ ] All acceptance tests passing
- [ ] Production deployment readiness verified

## Risks and Mitigations

| Risk | Impact | Probability | Mitigation |
|------|--------|-------------|-----------|
| v12.0 blueprints delayed | Entire roadmap slips | Medium | Start ADR framework now (ADR 0107) |
| Conflict count exceeds 15 | Timeline extends 4-6 weeks | Medium | Parallel ADR writing, large review panel |
| C2 dependencies (async, TLS) needed | Blocks consensus decisions | High | Escalate C2 resolution to owner early |
| Storage blocker (C4) impacts multiple systems | Implementation delays | Medium | Plan storage ADRs pre-Phase 3 |
| Paper-trading boundary violation discovered | Full rework of component | Low | Extensive review, mutation testing on gates |

## Success Metrics

- [ ] On-time delivery (within 2-week variance)
- [ ] Zero paper-trading boundary violations
- [ ] 100% of accepted ADRs reflected in code
- [ ] 85%+ blueprint traceability achieved
- [ ] All acceptance tests passing
- [ ] Zero tech debt introduced by migration
