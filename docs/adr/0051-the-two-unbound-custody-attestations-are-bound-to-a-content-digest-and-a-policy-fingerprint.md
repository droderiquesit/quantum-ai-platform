# 0051 — The two unbound custody attestations are bound to a content digest and a policy fingerprint

**Status:** *proposed*, 2026-09-07. Written as *accepted* by the lane that implemented it and downgraded on review: every ADR from 0046 to 0050 is proposed, two of fifty-one are accepted, and ratifying a schema change to a type that travels on the hash-chained log is the desk's call, not the implementer's. The code is in the tree and the gates are green; what is outstanding is the decision, not the work.

**Relates to:** [ADR 0021](0021-the-blueprint-expects-live-capital-and-this-platform-refuses-it.md)
(the custody table and the veto-only gate are built; the policy engine behind
them is refused), [ADR 0002](0002-two-dependencies.md) and
[ADR 0009](0009-tiered-dependency-policy.md) (SHA-256 in `qip_core` is authorised by
name; no dependency is added here),
[ADR 0007](0007-exact-attribution.md) (a decision must be attributable after
the fact, which is what an attestation is for).

**Does not touch:** the paper-trading boundary. This record only adds refusals.
No path by which capital could leave the platform is created, enabled or eased;
`qip-capital-fabric` still contains no signing, no withdrawal and no call out
of the process, and
`no_signing_or_withdrawal_path_exists_for_capital_to_leave_the_platform` in
`qip-acceptance` is unchanged and still applies.

---

## Context: three points spoke, and only one of them said anything

Blueprint §37.4 closes with a rule about *who* may agree to a movement of
capital: three independent enforcement points — the venue's own allowlist
configured out of band, the platform's corridor gate, and the custody policy —
must all agree, and trading authority and transfer authority never share an
identity.

`qip-capital-fabric`'s `custody.rs` implements that as
`EnforcementPoints::all_agree`, which proves that all three points attested
under three distinct identities, and
`Agreement::disjoint_from_trading_authority`, which proves none of the three is
the identity that trades. `TransferGate::assess` asks both on every assessment,
live and replayed alike.

An `Attestation` carries a `reference`: "what it agreed to — a gate decision
id, a policy version, an allowlist entry reference". `Attestation::new`
validated it for being non-empty. Nothing else read it.

That gap has been found and closed once already, for one of the three points.
A venue-allowlist attestation filed against one address satisfied a corridor
running to any other, because the reference was never compared to the
destination; `CustodyPolicy::mirrors_the_venue_allowlist` now holds it to the
parsed `DestinationKey` for every class whose row sets
`venue_allowlist_mirrored`, and `CustodyPolicy::conforms` refuses a table that
offers a venue-side withdrawal with that flag clear, so the check cannot be
switched off by a row rather than by a review.

The same defect remained, unfixed, in the other two points. A record could
name three identities as having agreed to a movement while two of them had
agreed to nothing in particular. That is worse than two points, because two
points is visibly two; three points where two are unbound reads as three.

`.claude/rules/domains/risk-and-execution.md` names the shape by its other
instance: `MaxExpectedShortfall` shipped in every default limit set against a
figure that was always empty. **A control that cannot fire reads as
protection and is not.**

## The two problems that kept it open

A previous lane examined this and deliberately stopped, recording why. Both
reasons are real and both are addressed here rather than worked around.

### 1. The gate's reference had no identity to bind to

The natural identity for a transfer-gate attestation is the id of the record
the decision is written under. It cannot be used, for an ordering reason
rather than a stylistic one: `FabricJournal::decide` mints the `EventId`
**after** `TransferGate::assess` has run and returned its verdict. The value an
attestor would have to name does not exist at the seam that would check it. An
attestation is also filed *before* an assessment, not after; binding it to a
number minted downstream is not merely awkward, it is impossible in the
direction the data flows.

### 2. `CustodyPolicy` had no version or digest

The custody point attests that a policy agreed. Which policy is not written
down anywhere. Binding the reference therefore means giving the table an
identity — and `CustodyPolicy` travels inside `GateCommand` on the hash-chained
event log, so any field added to it is a wire-shape change to a type that is
sealed by a chain.

## Decision

**Both references are bound. Two new values are introduced and one schema
version is spent.**

### The gate's reference binds to a content digest of the movement

A new type, `qip_capital_fabric::assessment::AssessmentId`, is the SHA-256 of
the assessment's own content: the corridor, the source location, the
destination, the amount, and the instant the assessment is made at. All five
exist before `assess` is called, all five are carried on the `GateCommand`, and
a replay re-derives the digest from the record rather than reading it.
`Agreement::binds_to_assessment` refuses a transfer-gate attestation naming
anything else, and `TransferGate::assess` asks it inside check 1.

Those five fields are *the movement*, which is what an attestor agrees to. The
balances, the carried history, the velocity breaker and the kill switch are
deliberately excluded: they are what the platform knows, not what the attestor
agreed to, and folding them in would invalidate an attestation every time a
balance moved. A control nobody can satisfy is removed rather than fixed.
Whether the platform can afford the movement is checks 2 through 7's question.

Two assessments agreeing in all five are the same question asked twice at the
same instant on the same corridor. The gate is deterministic, so they receive
the same answer, and sharing an identity is honest rather than a collision.
This is recorded as a property rather than discovered later.

The digest material is length-prefixed field by field, and the numeric fields
are digested from their underlying integers — `Decimal` is a scaled `i128`,
`Timestamp` a nanosecond `i64` — so equality of the value and equality of the
digested bytes are one relation rather than a property of a `Display` impl in
another crate. Delimiter-joining was rejected for the reason the venue mirror
already documents in `asset@address`: a separator is a separator only until a
field contains one.

### The custody point's reference binds to a derived fingerprint

`CustodyPolicy::fingerprint` is the SHA-256 of the whole table, iterated over
its `BTreeMap` of rows and each row's `BTreeSet` of corridors, so the material
is built in one order on every machine and every run.
`CustodyPolicy::attested_against_this_table` refuses an attestation naming
anything else, and check 1 asks it.

**The fingerprint is derived, not stored, and this is the substantive choice in
this record.** `CustodyPolicy` gains no field. There is nothing new on the
wire inside `GateCommand`, nothing for an editor to forget to update, and
nothing that can disagree with the table, because the fingerprint *is* the
table reduced. The replay derives it from the policy the record carries, so a
replayed record proves the attestation was made against the policy that was
actually in force rather than against today's.

It covers every row and not the row an assessment is about. §37.4's rules are
cross-row — `conforms` refuses a single-party self-custody row and a
transferable collateral row whatever class is being assessed — and what the
custody point attests to is the policy in force, which is the table.

### The schema version goes to 3, and version 2 is refused by name

`FabricRecord::SCHEMA_VERSION` becomes 3, and `replay` refuses a version 2
record explicitly, naming the version written and the version read.

The refusal is not cosmetic and it is a different case from version 1. A
version 1 record is refused by serde whatever else happens, because
`GateCommand::funding` has no `#[serde(default)]`; the explicit check improves
its message. **A version 2 record decodes perfectly.** Every field is present
and every type still matches; what changed is the *meaning* of two reference
strings, which version 2 filled with filing notes because nothing read them.
Re-running check 1 over one produces
`gate_attestation_names_another_assessment` — a finding about an attestor who
agreed to the wrong thing, which is not what happened. In the log that reads as
a movement the gate declined, not as a record this build cannot judge. Those
are different findings and an operator acting on the first would go looking for
the wrong problem. Refusing by version says the true thing.

`a_version_two_record_is_refused_by_version_rather_than_re_judged` asserts
that the record still decodes, so the version check is proven to be carrying
the weight alone rather than sitting behind a serde accident.

## Alternatives considered and rejected

**The record's `EventId` as the gate's reference.** Rejected on the ordering
grounds above: it is minted after the control that would check it has already
run. This is not a preference; the value does not exist at the seam.

**The prior chain head as the gate's reference.** It exists before `assess`
runs, so the ordering objection does not apply. Rejected because it identifies
*when* an assessment was made and not *what* was assessed: one attestation
would bind to whatever the gate happened to be asked next at that chain
position. That is the property being replaced, moved one step along. It also
couples the gate to the journal, and the gate deliberately reads nothing it was
not handed.

**A caller-supplied intent id on `GateCommand`.** Rejected because it is a
second source of truth for a fact the command already holds. Two claims about
which movement is being assessed will disagree — `CLAUDE.md`'s sixth principle
— and the louder one would be the one the attestor wrote, which is the one
under attack. A digest of the movement cannot disagree with the movement.

**A `version` field on `CustodyPolicy`.** Rejected for the same reason: a
version an editor sets by hand certifies a table it may no longer describe. A
row edited without bumping the version carries a version that is false, and the
attestation would verify against it. This alternative is also the more
expensive one — it changes the wire shape of a type on the hash-chained log,
which the derived fingerprint does not.

**Leaving the references validated for non-emptiness and documenting the
limitation.** Rejected. Three enforcement points where two are unbound reads to
an operator as three, and the platform's own rules name that shape as the
defect class rather than as an acceptable interim state.

**Refusing a version 2 record by letting the control veto it.** Rejected: see
above. A veto and an unreadable record are two different findings and the log
must not conflate them.

## What it costs

**Every existing attestation reference string is invalidated.** Any
`transfer_gate` reference that is not this assessment's `AssessmentId`, and any
`custody_policy` reference that is not the in-force table's fingerprint, is now
refused at check 1. Every fixture in the repository that filed
`transfer_gate-record-1` or `custody-policy-v3` has been changed to derive its
references, in `qip-capital-fabric`'s `corridor_and_gate.rs`,
`custody_mirror.rs`, `journal.rs`, the new `attestation_binding.rs`, and
`qip-kernel`'s `ledger.rs`. A fixture that hard-codes a digest is not accepted,
because the edit that is easiest to make is the edit that turns a control back
into a constant.

**Nothing is deployed, so no sealed version 2 record exists to migrate.** The
capital fabric's journal is constructed in tests and in `qip-kernel`'s
in-memory platform; no execution node exists (`execution_nodes = {}` in every
environment) and no Cloud Run service writes a fabric record to durable
storage. There is no migration path in this record because there is nothing to
migrate, and that is stated rather than assumed: if a sealed version 2 log ever
turns up, it must be read by a build pinned at version 2 and not re-judged by
this one.

**An operational cost is accepted.** An attestor must now compute a digest to
file a transfer-gate attestation, and must re-file if the movement's amount or
instant changes. That is the control working: an attestation about a movement
that no longer exists is not an agreement to the one that replaced it.

**`Approved` records the identity it bound to.** The value the control compared
against is visible in the record rather than only inside the check, so an
operator reconciling an approval against the attestation beside it does not
have to take the gate's word that the two matched.

**Two new refusal reasons** —
`RefusalReason::GateAttestationNamesAnotherAssessment` and
`RefusalReason::CustodyAttestationNamesAnotherTable` — carry stable tokens, so
a refusal, a log line and a metric can name the same thing.

**Still not a capability.** An `AssessmentId` and a `PolicyFingerprint` unlock
nothing. ADR 0021 leaves this platform with no path an agreement could unlock,
and an `Approved` still carries no way to execute. What these two values decide
is whether the gate refuses.

## What would make this wrong

**A sealed version 2 log turning up.** Nothing is deployed and no such log
exists today. If one ever does, this record's refusal is correct but
incomplete: the log must be read by a build pinned at version 2, and that
build's existence would need recording here.

**An attestor that cannot compute a digest.** The transfer gate's reference is
now a value only something with the movement in front of it can produce. If
the point that attests turns out to be a human process filing paperwork rather
than a service, the binding is right and the *encoding* is the wrong shape for
it — the answer would be a rendered form of the five fields the attestor can
read and type, digested by the gate, not a looser check.

**A second seam that assesses.** The digest identifies a movement, not an
execution. If anything ever ran an assessment twice at one instant on one
corridor for two different purposes, the two would share an identity and one
attestation would bind to both. Nothing in this crate can do that today —
`TransferGate::assess` is called from exactly one place — and a second caller
is the thing to look at before trusting this record.

**A `CustodyPolicy` that grows a field serde reads and `fingerprint` does
not.** The fingerprint enumerates the row's fields by name. A field added to
`ClassConstraints` without a line in `CustodyPolicy::fingerprint` is a field an
attestation certifies nothing about, and
`the_policy_fingerprint_moves_with_every_field_of_the_table_and_is_stable`
enumerates the fields it knows rather than deriving them, so it will not catch
one by itself. Adding a field to that struct means adding it in both places.
