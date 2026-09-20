# ADR 0088: A belief is a distribution with evidence, a causal path and its own TTL, and the slot that carries it is versioned

- **Status**: Accepted, under the authority the owner delegated on 2026-09-19
  to the lanes deciding in-territory records, **as a specification for three
  owners; nothing is built by this record.** The lane that wrote it owns
  neither the slot contract nor its producer nor its consumer, and a record
  that reaches three territories is written first and built by each in turn.
- **Date**: 2026-09-20
- **Supersedes**: nothing. Corrects one premise the lane was briefed with:
  that a cell *consumes* the belief priors. It consumes the slot's
  freshness; the values reach no cell (Context, fact 2).
- **Related**: ADR 0005 (confidence is arithmetic, never assigned), ADR 0007
  (attribution), ADR 0063 (evidence posture and the reasoning engine's
  distinct handling of absence and conflict), ADR 0080 (the thirteenth
  policy item and how an item is added), the payload digest rule in
  `qip-contracts/src/policy.rs`.

## Context

Blueprint §56.5 rule 47: "Every belief carries a distribution, evidence, a
causal path and a TTL. A point estimate presented as a belief is a defect."
The register's rule-50/47 cell has called rule 47 "the load-bearing miss" since
2026-09-15, and it is right. The wire form of a belief is
`qip_contracts::policy::BeliefPriors { priors: BTreeMap<String, f64> }`
(`grep -n -A5 'pub struct BeliefPriors' backend/crates/libs/qip-contracts/src/policy.rs`):
one number per subject, no distribution, no evidence, no causal path, and
one TTL for the whole slot rather than one per belief.

Three facts fix what can be decided.

1. **The producer is a point estimate today.** `Platform::issue_belief_priors`
   calls `BeliefIssue::derive` over `pending_episodes`, and each prior is
   `episode.claim.confidence` — a single `f64` the reasoning engine's
   log-odds posterior collapsed to when the episode was formed. The
   distribution exists upstream: `qip-reasoning-engine`'s `Hypothesis` holds
   a Bayesian posterior over an `EvidenceSet` whose members carry origins and
   stances, discounted by concentration under the single-origin ceiling
   (rule 56). It is lost at the episode boundary, not never computed.
2. **No cell reads the values.** `qip-kernel/src/central/belief.rs`'s own
   module doc says it and gives the command:
   `grep -rn 'belief_priors' backend/crates/edge backend/crates/apps/qip-edge-node --include=*.rs | grep -v /tests/`
   finds nothing. What reaches a cell's decision is the slot's *freshness*,
   through `PolicyItem::capability` → `Capability::BeliefState` →
   `DegradationState::sizing_multiplier`. A stale or unproduced slot halves
   size; a fresh one does not. The record's shape therefore changes no cell
   behaviour by itself, and that is what makes it safe to decide now.
3. **The digest is over the bytes.** `PolicyPayload::slot_digests` hashes
   `serde_json::to_vec(slot)` per item, every slot type is
   `#[serde(deny_unknown_fields)]`, and the signing string is the join of the
   per-slot digests. Any change to `BeliefPriors`' fields changes the bytes
   of every payload that carries it, so every digest computed before the
   field — and every cell verifying against the old type — fails. A slot
   cannot gain a field in place.

## Decision

### 1. The record

A belief on the wire is:

```text
Belief {
    subject:      String            — the instrument or entity the claim is about
    claim:        { class, direction }   — the episode's claim, as today
    distribution: BeliefDistribution
    evidence:     Vec<EvidenceRef>  — { id, origin, stance, observed_at }
    causal_path:  Vec<String>       — edge identifiers spelled exactly as the
                                      causal digest spells `active_edges`
                                      ("cause->effect:mechanism"), so a cell
                                      holding both slots can join them
    ttl:          Duration          — this belief's own, never longer than
                                      PolicyItem::Beliefs.time_to_live()
    formed_at:    Timestamp
}
```

`BeliefDistribution` is an enum with **no point variant**. Its first arm is
`Beta { alpha, beta }` — the conjugate form of a probability-of-claim
belief, which the reasoning engine can derive from its log-odds posterior
and the effective evidence count without inventing anything: the posterior
mean is what the point estimate was, and the pseudo-count is the evidence
the posterior actually rests on after the concentration discount, so a
belief on one origin is *wider*, not merely capped. Other arms are added by
amendment when an engine can produce them. A producer that has only a point
for a subject leaves that subject out; it does not wrap the point in a
degenerate `Beta`, because that is the rule's defect with a new name. Money
does not appear: every field is a statistic or an identifier, and the
crossing to `Decimal` stays where it is today, at the cell's sizing
multiplier.

The evidence is references, not payloads. Origins are carried so the
single-origin ceiling is visible on the wire rather than only in the
posterior; stances are carried so absence and conflict (rule 49) survive
the crossing. The causal path is the edges the hypothesis was formed along
— the reasoning engine's `causal_context` today — and an empty path is
legitimate and means "no edge", never "unknown".

The per-belief TTL is derived by the producer from the hypothesis horizon
and the staleness of its newest evidence, and is refused above the slot's
own time to live. The slot's freshness stays the outer bound: a belief may
expire before its slot, never after, so a cell that reads only freshness is
never *less* narrowed by the new record than it is by the old.

### 2. The slot-versioning rule

**A slot type never gains, loses or retypes a field in place.** A changed
record ships as a new `PolicyItem` beside the old one, and the old one is
retired by a later record once no verifier depends on it. Concretely:

1. `PolicyItem::Beliefs` is added after `Dispositions`, on ADR 0080's path:
   the exhaustive match forces its `as_str`, `time_to_live` (the same five
   minutes as `BeliefPriors`) and `capability` (`BeliefState`, the same) to
   be decided by compiler error rather than default.
2. `PolicyPayload` gains `beliefs: Slot<Beliefs>` with `#[serde(default)]`,
   so a pre-field payload deserialises with the new slot unproduced.
3. The slot enters the signing string **only when produced**, on the
   precedent already in `slot_digests` for `Dispositions`
   (`if !dispositions_unstated(...) { digests.push(...) }`). An unproduced
   new slot contributes no part, so every digest computed before the field
   is byte-identical after it, and a payload signed before the field
   verifies after it.
4. Rollout order is consumer, then producer: a cell must know the item
   before the centre produces it, because a produced slot a cell cannot
   name changes the digest the cell computes and fails verification. That
   failure is the correct, fail-closed outcome — the cell keeps its last
   verified payload and narrows as its slots age — and it is also an outage,
   so the order is part of the decision.
5. `BeliefPriors` keeps being produced, unchanged, until every cell reads
   `Beliefs` for freshness; a further record retires it. Two slots carrying
   overlapping facts is a cost (under "What it costs") accepted for the length of the rollout
   and no longer.

### 3. The three owners, and the fourth party

- **The contract** — `qip-contracts/src/policy.rs`: the `Belief` and
  `BeliefDistribution` types, `PolicyItem::Beliefs`, the payload field and
  the produced-only digest arm. Owner: whichever lane holds `qip-contracts`
  when this is scheduled; none did in the wave that wrote this.
- **The producer** — `Platform::issue_belief_priors` and
  `central/belief.rs` in `qip-kernel`: a sibling `issue_beliefs` that derives
  the record per open episode, stamped like the priors on the oldest current
  belief and never on `now`, journaled produced or not. Owner: the kernel
  lane (Y1 in the wave that wrote this).
- **The consumer** — `qip-edge`: `PolicyItem::Beliefs` in the cell's
  freshness reading beside `BeliefPriors`, and nothing else. Whether a cell
  ever reads a distribution is a separate decision; this record gives it
  nothing to size on and forbids sizing on a belief's mean. Owner: the edge
  lane (W5 in the wave that wrote this).
- **The source of the distribution** — `qip-reasoning-engine`'s
  `Hypothesis`, which must expose its posterior as a `Beta` and its evidence
  as references with origins and stances, and must carry the causal path
  from formation to the episode. Owner: this lane, built when the contract
  lands and not before, because a producer-side type with no wire form is a
  type nothing reads.

## What it costs

- Two belief slots on the wire for the length of the rollout, both derived
  from the same episodes; the digest string grows by one part per payload
  once the new slot is produced.
- A subject whose engine has only a point is absent from the new slot. If
  every subject is, the slot ships unproduced and a cell reading it narrows
  — correctly, and visibly, which is the point of not wrapping a point.
- Three lanes and a fixed order. A lane that builds the producer before the
  consumer breaks verification at every cell.
- The reasoning engine's posterior is a log-odds clamp with a concentration
  discount (rule 56's mechanism), not a calibrated Bayesian update; a `Beta`
  derived from it is an honest *shape* for what the engine knows, not a
  claim that the engine's uncertainty is well calibrated. Rule 57's Brier
  scoring is what measures that, and it is unchanged by this record.

## Alternatives rejected

- **Change `BeliefPriors` in place.** Breaks every pre-field digest and
  every cell on the old type at once, with no rollout order that avoids it.
- **Wrap the point in a degenerate distribution.** Rule 47 names this as
  the defect; giving it a type does not change what it is.
- **Carry the record inside the episodic digest.** Slot 4 is a digest of
  episodes, not a belief state, and the two have different freshness facts;
  merging them makes one slot's staleness lie about the other.
- **A global payload schema version.** Re-signs every slot at once and
  narrows every old-version cell to nothing simultaneously; the per-item
  rule fails one slot at a time and only where a change was made.
- **Let a cell size on the belief's mean.** Money stays `Decimal` at the
  cell and the cell's sizing is a multiplier on freshness; a mean on the
  wire is a second source of the same fact the priors already carry.

## What would make this wrong

1. If a cell begins to read belief values rather than freshness, the
   consumer half of this record becomes a sizing decision and needs its own
   record naming what a distribution changes at the edge.
2. If the reasoning engine gains a genuinely calibrated posterior, the
   `Beta` derivation is replaced by the engine's own form under a new
   `BeliefDistribution` arm, by amendment.
3. If the payload contract adopts a per-slot version field in place of the
   item-per-change rule, the versioning rule above is superseded by that record.

## Evidence

- The type: `grep -n -A5 'pub struct BeliefPriors' backend/crates/libs/qip-contracts/src/policy.rs`.
- The producer: `grep -n 'pub fn issue_belief_priors\|pub fn derive' backend/crates/runtime/qip-kernel/src/platform.rs backend/crates/runtime/qip-kernel/src/central/belief.rs`.
- The absent consumer: the command quoted in §Context, from `central/belief.rs`'s module doc.
- The digest rule and the produced-only precedent:
  `grep -n 'fn slot_digests\|dispositions_unstated' backend/crates/libs/qip-contracts/src/policy.rs`.
- Nothing built: `grep -rn 'BeliefDistribution\|PolicyItem::Beliefs\b' backend/crates --include=*.rs` prints nothing at this record's commit, and must.
