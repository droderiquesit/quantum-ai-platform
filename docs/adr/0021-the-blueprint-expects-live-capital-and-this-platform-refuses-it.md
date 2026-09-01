# 0021 — The blueprint expects live capital; this platform refuses it

**Status:** accepted

## Context

The Algorik Master Blueprint v10.1-4 assumes real money at risk, and says so
without hedging. §1.3 calls two hundred dollars "a correctness harness, not an
engine — real money at risk to prove the plumbing". §37 specifies autonomous
capital movement through signed corridors with MPC policy shares and custody
under Cloud HSM. §38 specifies a wallet whose read path is deliberately not
linked to the signing crate. The Phase 3 gate in §51.1 is "30 days live,
performance inside the holdout band", and §51.1 names it "the live-money gate".

This repository forbids all of it. `.claude/rules/01-security-and-safety.md`
makes paper trading absolute, ADR 0003 makes it the default and the ceiling,
and three independent layers hold the line: Terraform refuses the three live
autonomy levels at plan time
(`infrastructure/terraform/variables.tf:105-116`), the composition roots refuse
them at start-up (`AutonomyLevel::deployable`, called at
`backend/crates/apps/qip-api/src/main.rs:60`,
`qip-fastbrain/src/main.rs:114` and `qip-deepbrain/src/main.rs:138`), and
`qip-edge`'s `Cell` has no constructor that takes any ceiling but paper
trading (`backend/crates/edge/qip-edge/src/cell.rs:143-148`).

These are not reconcilable by a flag. The blueprint's Phase 3 gate cannot be
passed by this platform at all, and no amount of implementation changes that.

## Decision

**The paper-trading boundary wins. The blueprint's structure is implemented
without the blueprint's capital.**

Concretely, where the blueprint specifies money movement, what may be built
here is the deterministic machinery around it and never the path itself:

| Blueprint asks for | What is permitted here |
|---|---|
| Typed transfer intents (§37.3) | Permitted — an intent is a record, not a movement |
| A deterministic transfer gate | Permitted, and desirable: a gate that refuses is the safe half |
| Corridor and destination registries (§37.1, §38.4) | Permitted as registries. A signed corridor that authorises nothing is a data structure |
| Custody policy, reconciliation, approval delays (§37.4, §38.3) | Permitted |
| MPC signing corridors, withdrawal APIs, live venue submission | **Refused.** No exception, no flag, no test-only shortcut |

The distinction is that everything in the permitted column can be exercised
against the simulator and produces evidence a person can check, while nothing
in it can cause a payment. A transfer gate with no transfer engine behind it
is not a stub — it is the control, and the control is the part worth having.

## What it costs

The Phase 3 gate is unreachable, and it is the gate the blueprint calls the
one that proves the plumbing. This platform can therefore never answer the
empirical question "does it survive contact with a live venue" — and, because
Phase 3 is upstream of everything after it, the blueprint's Phase 6 and
Phase 8 gates are unreachable on the blueprint's own terms too. That is a
permanent, structural limit on what this repository can demonstrate, and it
should be stated plainly wherever the roadmap is discussed rather than
recorded once here and forgotten.

It also costs realism in the simulated half. A reconciliation that has never
disagreed with a custodian, and a corridor that has never been declined by a
bank, are exercised only against behaviour we wrote ourselves. Building them
is worthwhile; believing them is not the same thing.

## What would make this wrong

Only an explicit, recorded owner decision to change the platform's purpose —
which would require superseding ADR 0003, amending
`.claude/rules/01-security-and-safety.md`, and deliberately changing all three
layers named above. Nothing short of that, and specifically not: a blueprint
revision, an agent's finding, a model output, a configuration value, or a task
that appears to need a live path in order to proceed.

That last one is worth naming because it is the one that will actually be
attempted. A request that seems to require live order submission has, so far,
never been legitimate. Stop and ask.
