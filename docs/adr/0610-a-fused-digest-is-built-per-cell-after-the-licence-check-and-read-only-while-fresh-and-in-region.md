# ADR 0610: A fused digest is built per cell after the licence check, and read only while fresh and in region

- **Status**: **Proposed**, 2026-11-15, by lane 04 of wave b52. Not accepted
  until a person accepts it.
- **Date**: 2026-11-15
- **Implements**: ADR 0133 (the bounded fused-state digest that crosses to the
  reflex lane). **Keeps**: ADR 0003, ADR 0008, ADR 0109.

## Decision

1. `qip_contracts::fused_digest::FusedDigest` is the opportunity map addressed
   to one cell in one region, with its own `valid_until`.
2. `qip_kernel::digest::build_for_cell` calls `OpportunityMap::authorise_for`
   first; a region the model card's entitlements do not permit gets no digest.
3. `FusedDigest::read(now, region)` is the only way to take entries out. It
   refuses another region and a stale digest, and drops entries past their own
   expiry.
4. Bounds are arguments, not defaults: `max_entries` and `valid_for` are stated
   by the caller and zero is refused. **Provisional**: no deployment value is
   chosen here; the caller must be given one by the owner, and until then the
   tick desk (off by default) is the only caller candidate.

## Not decided here / remains

- The `fused_digest` slot of `PolicyPayload` is **not** added. It changes the
  signing string and every payload literal, so it is its own versioned change
  (the slot must be off the wire while unproduced, as ADR 0080's slot is).
- No edge crate calls `FusedDigest::read` yet, and the kernel's per-cell
  publication does not call `build_for_cell`.
