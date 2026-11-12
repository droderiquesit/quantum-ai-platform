# ADR 0446: A failed product and a spun-off product are states of the historical universe, and a tape carries listing changes

- **Status**: **Accepted 2026-11-12 under the owner's 2026-10-31 delegation
  and decision 28 of 2026-11-11**, session t50-10. It adds a corporate-action
  kind, two history states and a tape section. It widens no authority, adds
  no dependency, and touches no paper-trading layer.
- **Date**: 2026-11-12
- **Rows**: R12-TICK-2090 (stays PARTIAL).

## Context

R12-TICK-2090 reads: "Delisted instruments, venue outages, symbol changes,
corporate actions and failed products remain represented in historical
universes." `qip_financial::history::UniverseHistory` already kept delisted
listings, symbol eras and venue outages, and the kernel wrote delistings and
renamings from `Platform::queue_corporate_action`. Two independent checks
(2026-10-30, 2026-10-31) held the row PARTIAL, among other reasons because:

1. failed products had no state, so a fund wound up worthless was recorded,
   if at all, as an ordinary delisting — indistinguishable from a name that
   left the universe in good order, which is the most flattering form of
   survivorship;
2. spinoff products were never listed, so the value a spinoff carries out of
   its parent left the universe with nowhere to go;
3. no shipped feed emitted a delisting or a renaming: the committed tape
   format carried cash dividends only, so the kernel's membership arms were
   reached from tests alone.

## Decision

1. **`CorporateActionKind::Failed { reason }`** is a new terminal kind
   (`is_terminal` is true; the price series is not adjusted, as for a
   delisting). It is distinct from `Delisting` because the distinction is
   the fact the history must keep. The event's schema version is not
   bumped: the change adds a variant, every reader is the same binary, and a
   log written before it holds no `failed` record to misread.
2. **`UniverseHistory::fail(object_id, at, reason)`** ends a listing exactly
   as `delist` does — every `delist` refusal applies — and keeps the reason
   on `Listing::failure`. An empty reason is refused: a failure nobody can
   explain is a delisting. `UniverseHistory::failed()` lists every failed
   product. `members_as_of` keeps a failed product for the dates it traded,
   by the same span rule as a delisting.
3. **`UniverseHistory::list_spinoff(parent, child, symbol, at)`** lists the
   spun entity from the ex-date and records `Listing::spun_off_from`. It is
   refused when the parent was not a member at the ex-date, when the child is
   the parent, and when the child is already named.
4. **The kernel** matches `Failed` before the generic terminal arm, so a
   failure is filed as a failure, and on `Spinoff` lists the spun entity
   under the action's `spun_entity` as both object id and symbol — the action
   carries no other name for it, and inventing one would be a guess — unless
   the history already names it (a catalogued spun entity, or a re-fed tape).
   Both are recorded at queue time after the idempotency checks, as the
   delisting and renaming arms already were.
5. **A tape carries `listing_changes`**, a defaulted section of
   `{object_id, at, known_at, ex_date, change}` where `change` is one of
   `delisting`, `failed`, `renamed`, `spinoff`. Each is held to the shared
   leak, order and clock refusals, an effective instant before its
   announcement is refused, a blank reason, symbol or spun entity is
   refused, and a spinoff fraction is validated where every price adjustment
   is. Each releases as the matching `CorporateAction` through
   `TapeFeed::poll`, on its own cursor.

## Consequences

- A process started on a tape can now be told an instrument stopped
  trading, failed, was renamed or spun something off, and the historical
  universe records it on the default path.
- Still open, and named in the row's gap: no *licensed* feed emits these
  actions (the tape is synthetic); `record_outage` has no production caller,
  and venue-status halts and cell halts do not reach the history; membership
  is not rebuilt from the event log at resume; the history's past reaches
  back only to the first bar the process saw; and the deep-brain round test
  pins the snapshot, not a refused delisted name.

## What would make this wrong

A feed whose failures cannot be told from delistings would make `Failed`
an unused arm; then the distinction would be documented as unavailable for
that feed rather than inferred from a reason string.
