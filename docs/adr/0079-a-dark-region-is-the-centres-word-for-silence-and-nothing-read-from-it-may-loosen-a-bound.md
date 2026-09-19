# 0079 — A dark region is the centre's word for silence, and nothing read from it may loosen a bound

**Status:** *accepted*, 2026-09-19. Implemented the same day across the four
crates named under "The change, crate by crate" — `42c68d5` (the
subtract-only slot), `c0f957f` (the derivation, `issue`'s refusal, the share
freeze and both journal events), `4b209fa` (the invariant test, the
`/regions` rendering and the ACT-exit journaling), `b7558d6` and `bcb5619`
(the edge half, at the extension) — and given the composition root this
record required by lane C1: `43aa251` (`qip-api` reads `QIP_REGION_DARK_AFTER`
as whole seconds and refuses a malformed value, zero, a negative and a window
past the envelope ceiling at start-up, naming the variable and surfacing the
plane's own refusal rather than restating its bounds; the deep brain
deliberately does not read it, because it ingests no cell report and a
window there would be a number over an empty `last_heard`), `c7784ac`
(`region_dark_after` as a root variable with no default, rendered on the
API's catalogue entry alone through a conditional arm and planned both ways
in `infrastructure/terraform/tests/region-dark-window.tftest.hcl`), and the
manifest-wiring and record commits that follow under
`git log --oneline --grep='QIP_REGION_DARK_AFTER' claude/compassionate-cray-jvx8jt`.

Two limits stand, stated rather than rounded up. Every environment leaves the
window null and says why in its own tfvars: no Cloud Run API serves a mesh
(`THE_MESH_HAS_NO_PORT_ON_CLOUD_RUN` in
`backend/crates/tests/qip-acceptance/tests/manifest_wiring.rs`), so a window
today would arm a derivation over a centre that can hear no cell, and no
deployed process derives anything yet. And `CentralPlane::issue` has no
production caller, so Decision four's refusal is built, proven and unreached
until the operator route its own doc names exists.

One claim in the Context below has become false since it was written and is
left as written because the decision does not turn on it:
`grep -rn 'region_outlook\|is_region_dark' --include=*.rs backend/crates/`
now prints the cell's `apply_region_outlook`, `region_outlook` and their
callers, which the region-wire lane landed. That reading is the cell's own,
of its peers, off a local mount — which a crashed node still cannot send —
so the centre's derivation from silence remains a second, independent source,
and the two are kept distinct on purpose
(`grep -n 'pub const GATE_CENTRE_DARK_REGION\|pub const GATE_DARK_REGION' backend/crates/edge/qip-edge/src/cell.rs`).

**Relates to:** blueprint §36.3 (rows two and three, "a region's node
crashes" and "whole cloud region fails"), §36.2 (the twelve-slot payload),
§6.2 (narrowing on staleness); ADR 0008 (cells decide alone; its second
reversal condition), ADR 0039 and ADR 0044 (region shares over the mesh), ADR
0062 (`withdrawn_venues` may only subtract), ADR 0073 (a venue's region is
the cell's configuration; the wire may subtract and never add).

**Does not touch:** `qip-edge::mesh::CellStateDelta`, `qip-mesh`'s
`WireDelta`, any risk limit, any capital envelope's bounds, any autonomy
ceiling, and the paper-trading boundary in all three layers. Nothing here
issues an order. Every effect it has is a refusal, a retention or a journal
entry.

---

## Context

`docs/DELIVERY-STATUS.md` scores §36.3 `PARTIAL` with the dark-region arm
absent: `RegionState` in `qip-routing` names four inventory-band postures and
"none of them is 'this region has gone dark', and nothing suspends an
arrangement on one"
(`grep -n 'pub enum RegionState' -A 18 backend/crates/edge/qip-routing/src/mirror.rs`).

This lane was briefed that the cell already publishes the fact
(`Cell::region_outlook`, `is_region_dark`) and that it stops at
`CellStateDelta`, which has no field for it. **The first half is false in the
tree this record was written against:**
`grep -rn 'region_outlook\|is_region_dark' --include=*.rs backend/crates/`
prints nothing. The second half is verified, and so is the chain it opens:
`CellStateDelta` has no dark field
(`grep -n 'pub struct CellStateDelta' -A 55 backend/crates/edge/qip-edge/src/mesh.rs`);
`qip-mesh` decodes it through a private `WireDelta` that mirrors every field
by hand and the compiler holds nothing between the two
(`grep -n 'struct WireDelta' -B 10 backend/crates/services/qip-mesh/src/delta.rs`);
`qip-api` builds the kernel's `CellReport` from the decoded standing in
`report_from`, and `CellReport` has no `region` at all
(`grep -n 'fn report_from\|pub struct CellStandingSummary' backend/crates/apps/qip-api/src/mesh.rs`,
`grep -n 'pub struct CellReport' -A 12 backend/crates/runtime/qip-kernel/src/central/plane.rs`).
The region a cell reports is kept in `qip-api`'s `CellStandingSummary` for
the status page and never reaches the plane.

The kernel lane's finding is verified in the code and is the load-bearing
fact here. `CentralPlane::ingest` replaces a cell's positions and recomputes
`AggregateExposure::of(all)`; `crowded(minimum_cells)` keeps an instrument
only when at least that many cells hold it; `recall_for` names the cells
behind a concentration finding
(`grep -n 'pub fn ingest\|fn recall_for\|fn cells_behind' backend/crates/runtime/qip-kernel/src/central/plane.rs`,
`grep -n 'pub fn crowded' -A 8 backend/crates/services/qip-capital/src/exposure.rs`).
So "global exposure recomputed without it" — §36.3 row three, read literally
— **can only loosen**: dropping a silent cell's book lowers gross, lowers
every per-axis concentration it contributed to, takes it out of the cell count
`crowded` needs, and removes it from the set a recall would name. A region
going dark would then read at the centre as the platform having become
*safer*, which is the `MaxExpectedShortfall` shape with the sign reversed.

Two more facts shape the decision. A crashed node publishes nothing, so the
fact "this region is dark" cannot come from the region: a cell that can say
it is dark is not. And the centre already has one structural answer for a
cell it cannot reach — `MAXIMUM_ENVELOPE_VALIDITY`, twelve hours, "the only
revocation mechanism there is" for an unreachable cell
(`grep -n 'pub const MAXIMUM_ENVELOPE_VALIDITY' -B 8 backend/crates/services/qip-capital/src/envelope.rs`)
— and the recall register's acknowledgement window, which is how a recall
learns a cell is unreachable. What the centre does today with a silent cell
is keep its last book, unreplaced, indefinitely and unmarked. That is the safe
half by accident. This record makes it deliberate and adds the other half.

## Decision

**One. "Dark" is the centre's derivation from silence, never a cell's
claim, and no field is added to the cell-to-centre wire.**

A region is dark at `now` when the centre has heard from at least one cell of
that region at some time, and from none of them within
`CentralConfig::region_dark_after`. The window is a `Duration` an operator
states; `CentralPlane::new` refuses a zero window as it refuses a zero
`recall_acknowledgement`, and refuses one longer than
`MAXIMUM_ENVELOPE_VALIDITY`, because a darkness the centre would notice only
after every envelope in the region had already expired is a control that
fires after the fact it exists to catch. A region the centre has never heard
from is not dark; it is unknown, and unknown already receives nothing.

The determination is derived on every read from `last_heard` — a
`BTreeMap` of cell to its region and the instant of its last report — and is
never a stored flag that a later code path could clear. The *transition* is
journaled, in both directions, as its own event in the `Act` group with
permanent retention, so an operator can read when a region went dark and when
it spoke again; the event is the record of a derivation, not a second source
of truth, because replaying the reports re-derives it.

A cell's own vocabulary for its own degradation is already `halted` and
venue quarantine, both on the wire today. Neither is "dark", and this record
adds no third word to `CellStateDelta` — chosen partly because every field on
that delta is declared twice (`CellStateDelta` and `WireDelta`) with nothing
holding the two together, and a fact that is derived from *absence* needs no
field at all.

**Two. The centre learns a cell's region from the region the delta already
carries.** `CellReport` gains `region: String`, `#[serde(default)]` so a
journaled report written before the field replays; `report_from` fills it
from `CellStanding::region`. That is the whole of the cell-to-centre change,
and it crosses no wire: the region has been on the delta since the delta
existed.

**Three. The last book is held and marked stale, never dropped, and the
deviation from §36.3 row three is stated rather than quietly taken.**

A dark cell's positions stay in the plane's `positions`, in the aggregate, in
every concentration, in `crowded`'s cell count and in `cells_behind`. The
exposure the centre reasons on while a region is dark is *at least* what the
region last reported. "Global exposure recomputed without it" is refused;
the exposure is recomputed with the dark region held constant, because a
position does not vanish when its reporter does, and ADR 0008's second
reversal condition is exactly this case: aggregate exposure that cannot be
kept accurate means capital is being granted blind, and the honest reading of
"cannot be kept accurate" is the worst case, not zero. Recalls already issued
stand; new recalls to a dark cell are still issued, because the recall
register is the record of the centre's intent and its unacknowledged window
is the existing wire by which "silent" becomes "unreachable".

**Four. Nothing new enters a dark region.** `CentralPlane::issue` refuses an
allocation whose cell is in a dark region, naming the region, the last-heard
instant and the window in the refusal. ADR 0039's region share for a dark
region is frozen at its last value: it is neither counted as free nor
redistributed, so no other region's bound moves because a region went quiet.
The policy payload for a dark cell still ships — it cannot be received, and a
payload that is not shipped is indistinguishable from one that was lost — but
it carries no grant that did not exist before the darkness.

**Five. Mirrors involving a dark region suspend, through the one wire that may
subtract.** `FeasibilityConstraints` gains `dark_regions: BTreeSet<String>`
under exactly `withdrawn_venues`' serde discipline — `#[serde(default,
skip_serializing_if = "BTreeSet::is_empty")]`, so every payload signed before
the field keeps its digest, and with the same deploy-order consequence that
field documents: cells upgrade before the centre. A cell refuses at
`check_extension_for` any path-3 leg, and any transport edge of a
cross-region cycle, whose counterpart venue's region — from
`CellConfig::venue_regions`, ADR 0073 decision six — is named in the set; a
venue with no region annotation is refused too, since a missing fact is a
refusal (ADR 0073 decision five). The refusal lands under a bounded gate
literal so `qip_edge_refusals_total{gate}` stays bounded.

Not `withdrawn_venues`, and the distinction is the reason this is a new
field. A venue is not a region: a global venue traded from two regions would
be withdrawn at the healthy one. And `withdrawn_venues` is ADR 0062's
evidence window, whose reinstatement takes two signatures; a region that
comes back must not need two humans to say so.

**Six. Resumption is the first report, through the same door.** A region
leaves dark on the first decodable report from any of its cells. That report
goes through `ingest` like every other, and is halted on reconciliation
breaks like every other. "Reconciles against every venue before resuming"
(§36.3, affected column) is the cell's duty and its existing drop-copy
reconciliation; the centre adds no second resumption gate, because a second
gate here would be a second claim about one fact.

**Seven. The invariant, stated so a lane can test it rather than infer it.**
*No quantity the centre derives from a dark reading may be smaller than the
same quantity derived from the region's last report, and nothing may be
created by a region going dark.* Concretely: gross, every per-axis
concentration, `crowded`'s cell count, recalls outstanding, the withdrawn
venue set and every other region's share bound are monotone under darkness;
and no grant, whitelist entry, share or mirror eligibility exists after a
region goes dark that did not exist before. The test shape: a book crowded
across `minimum_cells_for_crowding` cells; silence one past the window;
assert `crowded` names the same instrument with the same cells, `recall_for`
still names the silent cell, `issue` refuses it, the produced constraints
name its region, and a cell holding that constraint refuses a path-3 leg into
it; then ingest a report from the region and assert it clears with none of
the above having moved in the meantime. Each assertion mutation-verified
against the corresponding "drop it" edit.

## The change, crate by crate

- **`qip-contracts` (lib):** `FeasibilityConstraints::dark_regions`. No new
  dependency; a `BTreeSet<String>` beside the one already there.
- **`qip-kernel` (runtime):** `CellReport::region`;
  `CentralConfig::region_dark_after` with its two refusals at construction;
  `CentralPlane::last_heard` written in `ingest`, `dark_regions(now)` derived
  from it, the refusal in `issue`, the share freeze, the two journal events,
  and the constraints producer naming dark regions.
- **`qip-api` (app):** `report_from` carries `region`. `CellStandingSummary`
  keeps rendering the status page; the `/regions` surface may render "dark"
  from the plane's derivation rather than compute one of its own, so the two
  never disagree.
- **`qip-edge` (edge):** `check_extension_for` reads `dark_regions` against
  `venue_regions`; one new refusal literal.
- **`qip-mesh` (service):** untouched. **`CellStateDelta` / `WireDelta`:**
  untouched.

**Dependency direction.** `qip-contracts` ← `qip-kernel`, `qip-contracts` ←
`qip-edge`, `qip-kernel` ← `qip-api`. Every arrow points inward. No lib gains
a dependency on a service; no service depends on the runtime; nothing depends
on an app. The one fact that has to travel from the app layer down —
the cell's region — travels as data on a type the runtime already owns.

## What it costs

- **A slow region reads dark.** A region that is merely late past the window
  is refused new grants it would otherwise have received; its existing grants
  run to expiry. That is the bound, and the direction is deliberate.
- **Held risk is overstated while dark.** The aggregate carries a book that
  may in fact have been flattened by the cell before it went silent. The
  centre cannot know, and the direction of the error is the one that refuses
  rather than grants.
- **The window is a number an operator picks**, and there is no measurement
  in this tree to pick it from. Too short refuses healthy regions; too long
  delays the refusal. The journal events are how a wrong choice becomes
  visible.
- **Deployment skew.** Cells upgrade before the centre, or an old cell
  refuses the whole payload the first time the field is present — the
  fail-closed half `withdrawn_venues` already argued for.
- **No node is deployed** (`execution_nodes = {}` in every environment), so
  every seam here reaches a deployed process only in test until one is.

## What would make this wrong

- A plane-side path that removes or zeroes a silent cell's positions:
  `grep -n 'positions\.remove\|positions\.clear' backend/crates/runtime/qip-kernel/src/central/plane.rs`
  should print nothing outside a test.
- `dark_regions` acquiring any reading a cell treats as permission, or a
  cell-side field by which a region's darkness is *cleared*.
- `region_dark_after` given a default rather than stated by an operator, or
  admitted at zero or beyond the envelope's maximum validity.
- A stored `dark` flag mutated by code rather than derived from `last_heard`.
- A region share redistributed to other regions on a dark reading — the
  test in Decision seven is what catches it.

## Alternatives considered

**Journal only.** Rejected: it charts a fact nobody acts on, which the
observability rule names as worse than the gap it replaces, and it leaves
`issue` able to grant into a region that cannot hear.

**Drop the region and recompute** — §36.3 row three, literally. Rejected for
the loosening the kernel lane demonstrated and this record verified.

**A cell-side dark report on the delta.** Rejected: a crashed node cannot
send it, a cell that can send it is not dark, and it would add a field
declared twice on a wire with nothing holding the two declarations together.

**Withdraw the dark region's venues through `withdrawn_venues`.** Rejected
for the two reasons in Decision five: wrong scope, wrong reinstatement rule.

**Halt every cell in the region through the kill switch.** Rejected: the
centre cannot reach them, and a halt the cell cannot hear is a journal line
wearing a control's name.
