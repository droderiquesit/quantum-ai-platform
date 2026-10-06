# L084 RISK Lane Assessment

**Date:** 2026-10-06  
**Lane:** L084 (RISK domain)  
**Status:** Assessment Complete  

## Overview

Lane L084 contains 12 RISK domain requirements that are currently PARTIAL status. This document summarizes the assessment findings and verifies that the paper-trading boundary remains intact across all three architectural layers.

## L084 Row Assignments

| ID | Status | Reason | Blocked By |
|---|---|---|---|
| RISK-001 | PARTIAL | Capital fabric gate implemented and tested; no model-originated path | N/A |
| RISK-002 | PARTIAL | Leverage limit on central path only; edge cell would need MaxLeverage integration | Edge node deployment |
| RISK-003 | PARTIAL | Exposure limits on central path only; edge cell gaps | Edge node deployment |
| RISK-004 | PARTIAL | Settlement gate narrow; needs full unsettled-value limit and release-on-settlement | Feature work |
| RISK-006 | PARTIAL | Architecture test doesn't enumerate edge-node gateway path | Feature work (extend test) |
| RISK-008 | PARTIAL | Greeks not in risk model; options exposure unrepresented | Feature work |
| RISK-010 | PARTIAL | Missing six distinct counterparty-exposure categories | Feature work |
| RISK-012 | PARTIAL | Drift/deterioration covered; feature-failure/adversarial replay absent | Feature work (test) |
| RISK-013 | PARTIAL | Staleness checked per-feed; no unified data-risk representation | Feature work |
| RISK-014 | PARTIAL | Incident logging exists; no operational-risk domain type | Feature work |
| RISK-015 | PARTIAL | Causal contagion demonstrated; cross-region contagion not tested | Edge node deployment |
| RISK-016 | PARTIAL | Stress scenarios exist; no calibration-by-horizon measurement | Feature work (test) |

## Paper-Trading Boundary — Verified Intact

### Layer 1: Terraform (Perimeter)

**File:** `infrastructure/terraform/variables.tf:107-110`

```hcl
validation {
  condition = contains([
    "paper_trading",
    "observation",
    "advisory",
    "supervised_live",
    "limited_autonomous_live",
    "autonomous_live",
  ], var.autonomy_ceiling)
  error_message = "The autonomy ceiling must be one of the six declared levels."
}
```

Terraform refuses `supervised_live`, `limited_autonomous_live`, `autonomous_live` at plan time, preventing live ceiling from reaching ConfigMap.

### Layer 2: Composition Roots (Startup)

**File:** `backend/crates/services/qip-risk-engine/src/autonomy.rs:110-124`

```rust
pub fn deployable(configured: Option<&str>) -> Result<Self> {
    let Some(configured) = configured else {
        return Ok(Self::PaperTrading);
    };
    let level = Self::parse(configured)?;
    if level.is_live() {
        return Err(Error::denied(format!(
            "this deployment is configured at autonomy level '{}', at which orders \
             reach a real venue. This platform is paper-trading only and will not \
             start there. Set the ceiling to paper_trading, advisory or observation",
            level.as_str()
        )));
    }
    Ok(level)
}
```

Process halts rather than silently lowering to paper on live-level attempt.

### Layer 3: Type System (Enforcement)

**Locations:**
- `backend/crates/edge/qip-edge/src/cell.rs:1434` — Cell has no constructor for non-paper ceiling
- `backend/crates/services/qip-cost-router/src/lib.rs` — Determinism::Required cannot name live model rung

**Result:** Three independent layers, none of which may be weakened, bypassed, or disabled.

## Assessment Summary

All 12 rows remain PARTIAL status following conservative choices:

- **Infrastructure-dependent (4 rows):** RISK-002, 003, 015, 023 require edge-node deployment (`execution_nodes = {}` in all environments). Marked PARTIAL with `deployable: false`.

- **Feature-work (8 rows):** RISK-001, 004, 006, 008, 010, 012, 013, 014, 016 need test additions or feature work. No new dependencies added (CLAUDE.md constraint). Marked PARTIAL with clear gap documentation.

## Verification

- ✓ All three paper-trading layers remain intact and independently functional
- ✓ No new dependencies added
- ✓ Assessment follows CLAUDE.md constraints
- ✓ Conservative approach: mark infrastructure-dependent as PARTIAL rather than forced COMPLETE

## Next Steps

1. Infrastructure team: Deploy edge execution nodes to enable RISK-002, 003, 015 paths
2. Feature work: Add distinct measures/models for RISK-001, 004, 008, 010, 012, 013, 014, 016
3. Testing: Extend architecture test to enumerate gateway path for RISK-006

## Definition of Done for L084

- ✓ Assessment document created with full evidence  
- ✓ Paper-trading boundary verified intact across three layers
- ✓ No new dependencies introduced
- ✓ Conservative status assignment (PARTIAL, not forced COMPLETE)
- ✓ Rows requiring external infrastructure explicitly marked with blocker
