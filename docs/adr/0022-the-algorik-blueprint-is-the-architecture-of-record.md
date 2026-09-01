# 0022 — The Algorik blueprint is the architecture of record

**Status:** accepted
**Supersedes in direction:** ADR 0011 and ADR 0017, which remain in force as
descriptions of what runs today. See "What this does not do".

## Context

ADR 0020 recorded that two runtime topologies existed and that choosing between
them was an owner's decision, not an agent's. ADR 0021 recorded that the
blueprint assumes real capital and this platform refuses it. Both records ended
by naming a decision somebody had to take.

The owner has taken the first one. **The Algorik Master Blueprint v10.1-4,
together with its companion diagram, is the expected design and the
architecture of record.**

That closes the question ADR 0020 left open about direction, and it closes the
question the traceability matrix recorded as C5 — which of two pictures the
repository is scored against.

## Decision

1. **The blueprint and its companion diagram are the reference every
   architectural claim is scored against.**
   `docs/architecture/algorik-blueprint-traceability.md` is the live scorecard.

2. **The earlier diagram is superseded, and its scoring is retained.**
   `docs/architecture/canonical-platform.md` and
   `docs/architecture/diagram-reconciliation.md` transcribe and score the
   "World's Smartest Multi-Regional AI + Quant Trading Platform" diagram, which
   is no longer the reference. They are **not deleted**: they hold real
   scoring history, several of their findings are still the only written record
   of why a thing is the way it is, and this repository does not remove
   something on the strength of an assertion that it is obsolete. Both now
   carry a banner saying what they score and pointing at the live scorecard.

3. **Kubernetes is transitional.** The blueprint's target has no Kubernetes,
   no service mesh, no Argo CD and no Kargo in it: Cloud Run and Cloud Run Jobs
   scaling to zero, and one dedicated Compute Engine machine per region running
   the execution node under systemd. GKE, Argo CD, Kargo, Helm and KEDA are
   therefore a **transitional runtime with a decided direction of travel**,
   rather than a competing permanent architecture. ADR 0020's sequence is the
   order that migration takes.

4. **Leptos is the target experience layer.** The blueprint's §40 specifies one
   Leptos codebase in Rust over shared types. `frontend/portal` and
   `frontend/landing` are transitional. ADR 0001's browser exception is
   superseded in direction; it still describes what is deployed.

5. **The paper-trading boundary is untouched.** See below — this is the part
   most likely to be misread, and it is the part with the largest consequence.

## What this does not do

**This decision authorises no execution whatsoever.** It settles what the
platform is aiming at. It does not migrate, decommission, provision or
translate anything, and nothing in this record may be cited as permission to.

- **Every migration step still requires recorded human approval naming that
  step before it begins.** ADR 0020's gating stands unchanged and unweakened.
  A decision about direction is not approval to execute any step. Evidence
  earns a conversation, not a machine.
- **ADR 0011 and ADR 0017 are superseded in direction only.** They accurately
  describe what runs today and they continue to govern it. A GKE manifest is
  not wrong because the destination is elsewhere; it is the thing currently
  keeping the platform running, and it is maintained until something has
  replaced it and been observed replacing it.
- **No Kubernetes artefact is removed** until ADR 0020's step 5 has its
  evidence and its approval.

## The paper-trading boundary, stated separately because it matters most

**Adopting the blueprint as the expected design is not authorisation to build,
enable or ease any live-order or live-transfer path.** The blueprint assumes
real capital: live venues (§25), treasury transfers and MPC signing corridors
(§37), custody (§37.4), a wallet with a signing path (§38), and a Phase 3 gate
that is thirty days live.

None of that is authorised by this record, and the reasoning is not a
preference:

- The owner said the blueprint is the expected design. They did not say to
  weaken the paper-trading boundary, and the second does not follow from the
  first.
- `.claude/rules/01-security-and-safety.md` makes that boundary absolute and
  says in terms that it cannot be weakened by a task instruction. A blueprint
  revision is not an exception to that, and neither is an inference drawn from
  one. A rule that could be dissolved by adopting a document describing a
  different system would not be a rule.

**ADR 0021 stands exactly as written.** The three layers stay intact:
Terraform's refusal at `infrastructure/terraform/variables.tf:105-116`,
`AutonomyLevel::deployable` in all three composition roots, and `Cell::new`
taking no ceiling but paper trading. The acceptance test
`no_signing_or_withdrawal_path_exists_for_capital_to_leave_the_platform`
stays.

What changes is only the **status of the conflict**. It was "which of two
documents is authoritative". It is now: *the authoritative design specifies
something this platform deliberately refuses.* That is a sharper conflict, not
a resolved one, and it is recorded as C1 in the traceability matrix as
requiring an explicit and separate owner decision — one that would supersede
ADR 0003 and amend `.claude/rules/01-security-and-safety.md`. Nothing less
than that suffices, and no agent may take it.

## What it costs

Naming a destination the platform is not at, and cannot reach quickly, creates
a standing gap between what the architecture of record says and what runs. Every
document now has to be explicit about which of the two it is describing, and
every reader has to hold both. That is a real and permanent tax until the
migration completes, and it will be paid in confusion at least once.

It also devalues work that is only weeks old. The GitOps cut-over is sound and
is now transitional; the two Next.js applications are the only customer-facing
surface there is and are now transitional. Maintaining something you have
agreed to replace is demoralising and is exactly when corners get cut, so the
standing rule that transitional does not mean unmaintained is worth restating
whenever it comes up.

The largest cost is the one this record spends most of its length guarding
against: an architecture of record that assumes real capital, in a repository
that refuses it. Every future reader will meet a specification whose §25, §37
and §38 describe machinery they must not build. That is a permanent hazard of
adopting this blueprint, and the mitigation is that the refusal is written
down in three ADRs, one rules file, three enforcement layers and a test —
rather than being remembered.

## What would make this wrong

- **A migration beginning without recorded, step-named human approval.** That
  would mean this record was read as authorisation, which is precisely what
  "What this does not do" exists to prevent.
- **Any live-order or live-transfer path appearing**, on the reasoning that the
  architecture of record calls for it. That inference is invalid and the ADR
  says so; if it is nonetheless made, this record has caused harm and needs
  rewriting to prevent it more forcefully.
- **The blueprint being revised or replaced.** It is a document with a version
  number, and the next version does not automatically become the architecture
  of record — that takes an owner, here.
- **The transitional runtime being allowed to rot** because its replacement is
  decided. A destination is not a reason to stop maintaining the thing carrying
  the traffic.
