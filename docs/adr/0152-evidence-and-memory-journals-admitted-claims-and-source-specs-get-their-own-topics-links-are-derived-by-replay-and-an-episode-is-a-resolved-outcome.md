# ADR 0152: Evidence and memory journals — admitted claims and source specs get their own topics, links are derived by replay, and an episode is a resolved outcome

- **Status**: **Accepted**, 2026-10-19, under the owner's explicit approval
  ("I allow ai agent to decide correctly also i provide my full approval",
  2026-10-19). It assigns two topics and their retention classes, which the
  rows called an owner call on `qip-events`, and defines what an episode is.
  It admits no crate, sets no financial limit, creates no billed resource and
  touches no protected path.
- **Date**: 2026-10-19
- **Extends**: ADR 0123 (one topic per class of fact, booked once by id),
  ADR 0132 (admission header and source manifest), ADR 0133 (what crosses
  between brains).
- **Keeps**: ADR 0005 (confidence as arithmetic), the hash-chained log as the
  one source of truth, bounded retention.

## Context

Eleven rows ask what the platform remembers about evidence and how. They
share two blockers the registers describe as "a new Topic whose retention
class is an owner call", and one undefined word, "episode".

The existing record is more settled than the rows say.
`qip-events/src/retention.rs` already has the classes the table needs
(`DerivedState`, `Episodic`, `Semantic`, `Referenced`), and
`EvidenceAttached` is already `Semantic`. `Platform::absorb_evidence` takes
`Vec<CheckedEvidence>`, admits each into the world fabric, and journals one
`KnowledgeDeltaRecord` per admitted claim on `Topic::WorldModelUpdated`. A
`KnowledgeDelta` carries ids, ops and a confidence, **not the claim**, so a
restart cannot rebuild the fabric from deltas; and `EvidenceFabric` is
in-memory, bounded and evicts. The packet form
`CheckedEvidence::packet` is built for every admitted claim and pushed onto
`PollReport.packets`, which nothing reads.

The principle in force is the one the repository states about its own log: no
second source of truth for a fact the log already holds. A journal of
*links* between claims would be exactly that — links are a deterministic
function of the admitted claims in order, at their recorded instants.

Locate them with
`grep -n 'fn absorb_evidence\|struct KnowledgeDeltaRecord' backend/crates/runtime/qip-kernel/src/evidence_intake.rs`,
`grep -n 'EvidenceAttached' backend/crates/libs/qip-events/src/topic.rs`,
`grep -n 'pub enum RetentionClass' backend/crates/libs/qip-events/src/retention.rs` and
`grep -n 'pub fn record_episode\|pub fn record_procedure' backend/crates/libs/qip-agents/src/memory.rs`.

## Decision

1. **A fact gets a topic only if the log does not already hold it, and the
   change that adds a topic names its retention class.** `Topic`'s exhaustive
   matches force the class and the group at compile time; this record does
   not add a class.
2. **Admitted claims are journaled; links are not.** A new topic,
   `Topic::EvidenceAdmitted` (`evidence.admitted`, group `Understand`,
   retention `DerivedState`), carries the checked claim — the evidence
   record, its document hash and the admission instant — written by
   `absorb_evidence` for each claim the fabric admits. `DerivedState` and not
   `Semantic`, because the fabric is a bounded cache and its journal should
   expire with the window the fabric could have held. Corroboration and
   contradiction links, contests and their resolutions are **derived**: a
   replay (`EvidenceFabric::rebuild`) re-admits the retained claims in journal
   order at their recorded instants, and an acceptance test proves the links,
   contests and resolutions equal those before the restart. No
   `EvidenceLinked` topic is created.
3. **`KnowledgeDeltaRecord` stays the journal of the world-model change; the
   packet is not journaled.** `EvidencePacket` is a derived export of a
   `CheckedEvidence`. `ConnectorRuntime::poll` stops pushing a packet per
   claim onto `PollReport.packets`; `CheckedEvidence::packet` remains for the
   first cross-brain consumer that names itself (ADR 0133's rule: nothing is
   built ahead of a consumer).
4. **A full, all-contested fabric refuses; it does not evict.** Eviction keeps
   taking the oldest claim in no open contest. When every held claim is
   contested and the fabric is at capacity, a new claim is **refused**,
   counted under a new `refused` outcome label, and may be polled again.
   Evicting a contested claim resolves a contest by capacity, silently.
5. **An ordering violation stays flagged-and-released.** A print numbered later
   and dated earlier fails the check and is released with the verdict on the
   record. Making it inadmissible was tried and reverted because it quarantines
   out-of-order prints against the kernel test's premise and the quarantine
   bound in `soak.rs`; reversing it needs its own record naming that bound.
6. **Source specs journal on `Topic::SourceRegistered`** (`source.registered`,
   group `Understand`, retention `Semantic`: they are entities and relations
   that stay true). `DiscoveryDesk` writes the `SourceSpec` from
   `RegisteredSource::source_spec` at registration. `FeedPack` is not built;
   no consumer exists. Discovery itself stays off in every committed
   environment; turning it on is a tfvars switch (PROTECTED, below).
7. **A new source is Sandboxed until promoted, and the gate is zero weight, not
   a new execution environment.** A source's first `SANDBOX_MINIMUM_POLLS`
   polls (a named constant set in code with its test) are admitted to the
   fabric at **zero weight**, never forwarded as a `KnowledgeDelta`, and
   counted. Promotion needs the minimum polls, zero timeline or ordering
   violations, and either one independent corroboration from a promoted source
   or a licence record declaring the source primary (ADR 0157). Promotion is
   journaled as a `SourcePromoted` body on `source.registered`. No sandbox
   process or isolation is built; the claim was never to run untrusted code,
   it was to keep untrusted *facts* from moving beliefs.
8. **An episode is a resolved outcome.** `MemoryKind::Episode` is written
   when a prior expectation resolves (the output of `calibrate_resolved`), not
   when a claim is acquired: an acquired claim is evidence in the fabric, not
   an episode (R12-DATA-2012). Its importance is the absolute calibration
   surprise, `|outcome − predicted probability|` in `[0,1]`. A below-threshold
   episode is **kept compressed** (counted into the key's summary), never
   dropped, and `ResearchMemory::hit_rate` is computed over all resolved
   episodes before any consult filtering, so a refusing consult cannot bias
   the calibration analysts are scored on. A `MemoryRegistry` refuses
   construction without a positive threshold: a default of zero is a gate that
   cannot fire.
9. **"Semantic memory" is not a new type.** In this tree it is the research
   memory's lessons promoted on support plus the world fabric. A contradiction
   record is a property of belief-bearing kinds — the world graph, the
   reasoning engine and the fabric's contests. Episodic memory holds outcomes
   and the self-model holds estimates; both are refused or re-estimated, not
   contradicted. An acceptance test pins which kinds carry the field so the
   set cannot widen by accident.
10. **No procedure is distilled yet.** `record_procedure` requires
    `MINIMUM_PROCEDURE_SUPPORT` resolved-correct episodes citing one ordered
    step list. The only step list on the running path is the panel's fixed
    dispatch order, which is code and not learning; recording it would file a
    constant as a lesson. The producer waits for a step sequence the platform
    chooses (a campaign's stage order). The composite acceptance suite asserts
    the three stores and the absent producer.
11. **A research campaign is identified by the digest of its normalised
    question,** and an experiment by `<campaign id>/<stage>`. The registry
    records experiment references, not a free-text `benchmark_suite`; an
    unresolved-contradictions list needs a variable-to-subject map that
    ADR 0151 declines to invent, so it is not added.

## Consequences

- The fabric survives a restart as a deterministic replay of its retained
  claims, and the replay is the test that links persist.
- Journal growth is bounded by `DerivedState` retention, not by every poll
  for ever.
- A burst of contested claims against a full fabric now shows up as refusals
  an operator can chart, not as silently lost hypotheses.
- Two topics are added (`evidence.admitted`, `source.registered`); every
  `Topic` match arm and the schema inventory change with them.

## What it costs

- A second journal write per admitted claim, and a rebuild function whose
  determinism must stay true as the fabric changes: any wall-clock or
  nondeterministic input in admission breaks the replay test, which is the
  intended tripwire.
- Refusing at a full contested fabric loses a claim until its next poll.
- A sandboxed source contributes nothing for its first polls.
- Compressed below-threshold episodes cost summary space and lose detail.

## What would make this wrong

- If replay cannot reproduce links because admission reads state outside the
  claim and its instant, links must be journaled after all and decision 2 is
  superseded.
- If refusals at capacity are frequent, the bound is too small for the poll
  rate; raise the bound, do not restore eviction of contested claims.
- If `DerivedState` expiry drops claims a still-open contest depends on, the
  fabric's retention and the journal's disagree and the class must move.

## Dissent

*Journal the links and the contests directly; replaying the whole fabric to
learn what it held is slow and fragile.* Strongest form: a restart should read
state, not re-derive it, and a deterministic replay is a promise every future
change to admission must keep. It lost because a journaled link is a second
claim about the same fact, and the day the two disagree the louder one is
wrong. It would be revived by a measured replay time that blocks start-up.

## Code it constrains

- `Platform::absorb_evidence` and `KnowledgeDeltaRecord` in
  `qip-kernel/src/evidence_intake.rs`; `EvidenceFabric::admit` in
  `qip-evidence/src/fabric.rs` (the refusal and `rebuild`).
- `CheckedEvidence::packet` and `ConnectorRuntime::poll` /
  `PollReport.packets` in `qip-market-ingestion/src/connector/`.
- `Topic`, its groups and `retention_class` in `qip-events/src/topic.rs`.
- `RegisteredSource::source_spec` and `DiscoveryDesk` in `qip-deepbrain`.
- `ResearchMemory` (`record_episode`, `record_procedure`, `hit_rate`) and
  `MemoryRegistry` in `qip-agents/src/memory.rs`; `ResearchCampaign` in
  `qip-agents/src/research.rs`.

## Rows unblocked

- R12-EVID-2032 — decisions 1, 2: the topic, the class, the replay test.
- R12-EVID-2038 — decisions 2, 4: contests persist by replay; all-contested
  refuses.
- R12-CONTRACT-7062 — decision 3: no packet journal; production of
  `PollReport.packets` stops.
- R12-EVID-2036 — decision 5: restated, nothing built.
- R12-EXPAND-6026 — decision 6: `source.registered`, `Semantic`.
- R12-DATA-2021 — decision 7: zero-weight sandbox and journaled promotion.
- R12-DATA-2012 — decision 8: an acquired claim is not an episode; typed facts
  update the world model through the delta that already exists.
- R12-EXPAND-6057 — decision 8: kind, importance, compressed-not-dropped,
  positive threshold required.
- R12-WORLD-1016 — decision 9: restated; no new type.
- R12-COVERAGE-7153 — decision 10: restated; no producer by decision.
- R12-EXPAND-6061 — decision 11.

## Not decided here

- **PROTECTED** — turning discovery on (`QIP_DEEPBRAIN_DISCOVER_EVERY` in
  `infrastructure/environments/*/terraform.tfvars`), which R12-EXPAND-6026
  needs before `source.registered` records anything in a deployment.
- Nothing is BILLED, RISK or paper-boundary.
