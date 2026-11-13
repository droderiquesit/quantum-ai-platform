# ADR 0488: The World Model Arbitration Brain is an organisation specialist reading the journaled federation review from a kernel-written slot

- **Status**: **Accepted 2026-11-13 under the owner's 2026-10-31 delegation**
  (a session decides the in-repository design question it meets, by ADR),
  session t50-31. The delegation's limits hold: it sets no figure the owner
  has not stated, changes no requirement's wording, adds no dependency and
  touches no paper-trading layer. The specialist is advisory: it sizes,
  vetoes and sends nothing.
- **Date**: 2026-11-13
- **Rows**: R12-WORLD-3025.

## Context

R12-WORLD-3025: "A World Model Arbitration Brain exists as a specialist
covering: scoring and reconciling competing world models, preserving
disagreement, tracking calibration, and deciding branch/merge/abstain/
request-more-data." Its check: the brain "is constructed in the production
organisation and emits findings for its scope; removing it drops its scope
from coverage."

The arbitration itself already ran: the kernel's `FederationStage` scores
every live member each UNDERSTAND pass, keeps every view in a
`Disagreement`, decides `Branch`, `Merge`, `Abstain` or
`RequestMoreInformation`, and journals a `FederationReview` under
`kernel/federation`. What the row's check could not find was a specialist.
The deep brain's registry covered the `world-model-arbitration` scope with a
hand-written `BrainSpec` (`kernel/world-model-arbitration`) describing the
kernel stage. The skeptic refuted that on 2026-10-25: the entry was not an
agent in the production organisation (`qip-investment-agents`'s roster had
none), no finding in the organisation's record named an arbitration, and
removing the entry stopped nothing but start-up.

## Decision

1. **A specialist on the roster.** `manifests::world_model_arbitration_brain`
   (id `world-model-arbitration-brain`, owner `investment-research`, the
   analyst read set and nothing more) is on `manifests::roster`, and
   `chief::standard_agents` constructs `world_model_brain::
   WorldModelArbitrationAnalyst` for it. It is research-side: it cannot reach
   the market and holds no veto.

2. **It reads what the federation journaled, from a kernel-written slot.**
   `Desk::arbitration` is a `Gated<ArbitrationView>` behind
   `Capability::ReadWorldModel` (label `world-model-federation`), in the same
   shape as the chain and company slots. `Platform::stage_understand`'s
   federation pass writes it through `brief_the_desk_on_arbitration` from the
   very `FederationReview` it is about to journal, so the view carries that
   review's `cycle`, `journal_base`, arbitrations, member calibrations and
   uncovered families. REASON's panel therefore reads the pass that ran in the
   same cycle, and a finding is checkable against the record a replay reads —
   it is not a second account of the same pass.

3. **It reports the federation's decisions; it makes none.** The finding
   counts each decision arm (`arbitrations_branch`, `_merge`, `_abstain`,
   `_request_more_data`, total `arbitrations_decided`), keeps every
   disagreeing view in `disagreeing_views_kept`, reports the widest spread,
   and states calibration as the federation scored it (`members_calibrated`,
   `members_unscored`, `members_outside_calibration_claim`, and
   `mean_member_brier` only when at least one member is scored — a mean over
   no scored member would be a number nobody measured). It re-scores nothing
   and merges no probability the federation did not merge.

4. **No review, or an empty one, is no data.** Before any federation pass,
   or when a pass arbitrated nothing, the finding is a no-view naming why.
   `Scope::WorldModelArbitration` is evidenced only by `arbitrations_decided`
   on an informative finding, so neither case covers the scope.

5. **The registry entry comes from the roster.** The manifest's first
   competency is `world-model-arbitration`, so `brains::organisation_spec`
   derives the deep brain's registry entry
   (`organisation/world-model-arbitration-brain`) from the roster, and the
   hand-written `arbitration_spec` is removed. `require_coverage` still
   refuses a node whose registry lacks the scope; the scope is now covered
   only because the production organisation carries the specialist.

## Consequences

- The integration check holds in the organisation:
  `removing_the_world_model_arbitration_brain_drops_world_model_arbitration_from_coverage`
  (qip-investment-agents `tests/organisation.rs`) and, on the platform's own
  desk,
  `understand_briefs_the_desk_from_the_federation_review_it_journals_and_the_arbitration_brain_reports_it`
  (qip-kernel `tests/symbolic_federation.rs`).
- The roster is twenty-eight agents and the standard organisation
  twenty-seven dispatched agents (the chief is on the roster and dispatches).

## What remains (not decided here)

- The registry entry's `calibration` field is still `0.0`, as for every
  `organisation_spec` entry; it is not yet computed from the federation's
  resolved forecasts. ADR 0158 decision 8's calibration tracking for the
  spec is open.
- `qip-fastbrain` and `qip-api` build the same `Platform` (so the same
  organisation and the same slot) but run no brain registry or
  `require_coverage` check; only `qip-deepbrain` does.
- The specialist's finding is advisory and feeds the chief's synthesis like
  any other; nothing in DECIDE reads it. DECIDE's narrowing still reads the
  federation review directly (ADR 0125 decision 4).

## What would make this wrong

A second writer of the arbitration slot, or a writer that copies anything but
the review being journaled: the specialist would then report a federation
pass the log does not hold. The writer is called once, in UNDERSTAND's
federation pass; locate it with
`grep -n 'brief_the_desk_on_arbitration' backend/crates/runtime/qip-kernel/src/platform.rs`.
