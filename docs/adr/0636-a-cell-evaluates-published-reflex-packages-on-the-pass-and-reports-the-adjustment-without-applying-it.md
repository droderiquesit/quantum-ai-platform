# ADR 0636: A cell evaluates published reflex packages on the pass and reports the adjustment without applying it

- **Status**: Proposed by wave b52 lane 18, 2026-11-15. A slice of
  R12-REASON-3063. It adds no dependency or figure and touches none of the
  three paper-trading layers.
- **Date**: 2026-11-15
- **Rows**: R12-REASON-3063 (stays PARTIAL).

## Context

ADR 0535 and 0536 settled that the deep brain drafts a compiled
`ReflexPackage` unsigned and a two-person `model-pack` publish signs it. The
published form, `PublishedReflexPackage::load`, and `LoadedReflex::evaluate`
had no production caller: a package could be compiled, signed and never
change anything.

## Decision

1. `qip_edge::reflex_rules::ReflexRuleBook` admits a published package only
   through `PublishedReflexPackage::load` against the cell's own `TrustRoot`.
   A refusal leaves the book unchanged. At most 64 packages are held; a
   duplicate id or a 65th is refused, never evicting.
2. `Cell::work` evaluates every unexpired package for each configured venue
   (input `venue`, the name the compiler writes) and carries the result out as
   `WorkReport::reflex_adjustments`. Expiry is rechecked on every pass.
3. `qip-edge-node` reads `QIP_REFLEX_PACKAGES_DIR` at the composition root;
   any file that does not verify stops the process naming the file.
4. The adjustment is **reported, not applied**. The compiled output
   `twin_entry_bps` corrects the central paper fill model, which the cell does
   not hold, and ADR 0195 decision 1 forbids the cell a second adjustment of
   its own numbers. Applying it at the centre is the remaining work.

## What would make this wrong

A rule whose output a cell sizing path should read. That needs its own ADR and
the reflex independence guard's agreement.
