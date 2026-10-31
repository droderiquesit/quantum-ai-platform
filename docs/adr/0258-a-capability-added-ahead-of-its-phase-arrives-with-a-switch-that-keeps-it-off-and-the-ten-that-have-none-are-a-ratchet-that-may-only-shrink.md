# ADR 0258: A capability added ahead of its phase arrives with a switch that keeps it off, and the ten that have none are a ratchet that may only shrink

- **Status**: **Accepted 2026-10-31 under the owner's delegation**. The
  owner's ruling on R12-ARCH-7134 (ADR_THEN_IMPLEMENT), in
  `docs/blueprint/lanes/wave1031-handoff-2026-10-31.json`, was relayed by
  the integration parent to lane L2-INTEL. The record sets no risk number,
  switches nothing off, and touches no paper-trading layer.
- **Date**: 2026-10-31
- **Extends**: ADR 0143, which says phases are gated by a committed
  register rather than by runtime flags. That ADR's decision 3 (record,
  do not switch off) stands. This record narrows it for capabilities
  added from now on.
- **Rows**: R12-ARCH-7134.

## Context

`docs/architecture/phase-register.json` lists the blueprint's seventeen
build phases. Phase 0 is entered and has not exited, but every later phase
carries capabilities that the tree already holds. Twenty such entries name
the switch that keeps them off. Ten name `none`: the supply chain the dev
deploy needs, the Cloud Run services, the cycle, the agent guard, the
quantum path, the NOW lattice, the tournament, solver routing, the
survival gate, and ambient intake. The requirement says later-phase
features stay disabled until a phase exits.

The owner declined to add switches to those ten. Switching off the
survival gate, or the cycle, to satisfy an ordering clause would remove a
protection rather than add one. The owner also ruled that nothing new
arrives ungated. Before this record, nothing enforced that: the register
test accepted any entry whose switch field was non-empty, and `none` is
non-empty.

## Decision

1. **A capability carried ahead of its phase must name a switch.**
   `phase_register.rs`'s `refusals` refuses a `not_entered` phase entry
   whose switch is `none` unless that entry is in `UNGATED`. The existing
   `every_named_switch_exists_in_the_tree` already requires a named switch
   to be a Terraform variable or a value an app reads, so a named switch is
   one the tree holds.
2. **The ten are a list in the test, keyed by phase and capability.**
   `UNGATED` holds exactly the ten entries committed on 2026-10-31. An
   entry moved to another phase is a new entry and is refused.
3. **The list may only shrink.** An `UNGATED` entry that the register no
   longer carries with switch `none` is refused until it is removed from
   the list. That covers an entry that gained a switch, one whose phase
   was entered, and one that was deleted. Otherwise a freed slot could
   later admit a different ungated capability under the old text.

## Consequences

- `a_capability_added_ahead_of_its_phase_with_no_switch_is_refused` first
  checks that the committed register carries exactly ten ungated entries.
  It then adds an eleventh to phase 2 and requires the named refusal. It
  also copies one of the ten to phase 2 and requires that copy to be
  refused.
- `an_ungated_capability_that_gains_a_switch_must_leave_the_list` gives
  phase 1's ungated entry a switch, and requires the refusal that tells
  the author to remove it from `UNGATED`.
- Mutations, each restored by cmp:
  - Accepting every `none` fails the first test.
  - Disabling the shrink check fails the second test.
  - Keying the list by capability alone fails the first test.
  - Adding an eleventh `none` entry to the committed register fails
    `the_committed_phase_register_is_admitted`.
- R12-ARCH-7134 stays PARTIAL, as the ruling says, until phase 0 exits.
  Phase 0's own exit is still open (R12-ARCH-10159).

## What it costs

- A capability that genuinely has to run before its phase can no longer
  just be written down as `none`. It needs a real switch, which means a
  Terraform variable or an app setting with a default of off, or else an
  owner ruling recorded as a new ADR that widens the list. That is the
  intended friction, and it falls on every lane adding an ahead-of-phase
  entry.
- The list repeats the register's capability text verbatim. Rewording an
  entry in the register fails the shrink check until the list is edited
  to match. The failure names the entry, but it is a second place to edit.

## What would make this wrong

A capability that cannot have an off switch without removing a protection
of its own, the way the survival gate cannot. If such a capability arrives
ahead of its phase, this record forces it either to carry a switch that
protects nothing, or to wait for a ruling. The ruling is the right
outcome. The first is not, and a reviewer should refuse a switch that
exists only to satisfy this test.
