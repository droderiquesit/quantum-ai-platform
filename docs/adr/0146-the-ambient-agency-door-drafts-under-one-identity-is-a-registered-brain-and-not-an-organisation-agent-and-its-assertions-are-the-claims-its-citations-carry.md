# ADR 0146: The ambient agency door drafts under one identity, is a registered brain and not an organisation agent, and its assertions are the claims its citations carry

- **Status**: **Accepted**, 2026-10-19, under the owner's explicit approval
  ("I allow ai agent to decide correctly also i provide my full approval",
  2026-10-19). That approval authorises these engineering choices; it does not
  grant any authority above `Shadow`, open an external action surface, or put
  a conduct refusal on the order path, and none of those is decided here.
- **Date**: 2026-10-19
- **Extends**: ADR 0114 Amendment 1 (D8: the one drafting adapter), by fixing
  how that door is identified, registered, judged and stopped.
- **Keeps**: ADR 0114 D8, ADR 0141 (nothing above narrow-reversible is
  granted), ADR 0125, ADR 0004 (agents reach facilities only through
  capability gates), ADR 0003.

## Context

The deep brain's `AmbientAgency` is the one production composition of
`qip-agency`: it offers each activated ambient signal to `AmbientIntake`,
which promotes it, runs the `ActionGuard` (tool registry, conduct gate, rate
and budget) and writes the draft to an internal outbox for a person to read.
Fourteen register rows are PARTIAL on questions that are about that door, not
about the engine behind the shadow binary. They fall in five groups, and one
reading of each is needed before deciding.

**Identity and authority** (AGENCY-1063, 7133, 7148, MODEL-4014). The door's
acting identity is the constant `ACTING_IDENTITY`, its promotion and plan
approvers are two policy identities the guard requires to differ from it, and
the policy it builds starts at `Authority::Shadow` with no `widen` call
anywhere on the path
(`grep -n 'ActionPolicy::new\|widen\|NarrowReversible' backend/crates/apps/qip-deepbrain/src/*.rs`
finds the one `ActionPolicy::new`). The test
`every_draft_the_node_writes_is_executed_under_the_one_registered_identity`
(`qip-deepbrain/tests/ambient_agency.rs`) already asserts the invariant.

**Registration and coverage** (AMBIENT-3027, AGENCY-3028, plus the scope of
WORLD-1156 and COVERAGE-7190). `qip-deepbrain/src/brains.rs` registers the two
ambient brains (`ambient_specs`, ids `AMBIENT_ATTENTION` and `CAUSAL_AGENCY`)
and `standard` refuses to build without them; `tests/brains.rs` proves that
removing one drops its scope from the registry. What is not decided is whether
the agency should *also* be an agent of the production Organisation or gain a
`qip_investment_agents::coverage::Scope`.

**Conduct** (AGENCY-1065, 1069, RISK-4103, COVERAGE-7190). `conduct_brain.rs`
checks every `CommunicationDraft` (id, `content`, channel, sender, a list of
`EvidenceCitation`s each with an optional `claim`, disclosures, audience,
jurisdiction) for factual support, provenance, identity, disclosures,
audience, jurisdiction and market conduct, and the drafts of a pass for
coordinated voices. The rows ask for a rule that separates an *assertion
sentence* from an *action description* ("Quote 20 lots") so each assertion can
be bound to a resolving citation, and for three further checks
(externality, overclaiming, channel compromise). Two facts from the code
change the question:

- `check_factual_support_resolving` already refuses a draft with non-empty
  `content` and no citation, refuses a citation that resolves to no recorded
  evidence, and refuses a citation whose `claim` is not the recorded
  statement (compared by `qip_evidence::fabric::statement_fingerprint`). So
  "an uncited sentence would be refused as rumour" is not the hazard the
  binding approach creates; at the draft level it is today's behaviour.
- The ambient door's `content` is not prose a model wrote. It is
  `format!("Ambient {:?} on {}: wake {:?}. Evidence {}. Draft for a person to
  read.", class, subject, pathway, evidence ids)` in `AmbientIntake::attempt`:
  typed fields and a fixed frame. Its citations carry `claim: None` because
  the evidence ids are detector labels, not fabric records (the source says
  so). Free prose reaches the conduct gate only from an operator's request
  document on the shadow binary.

**Reauthorisation** (RES-7109). `ToolRegistry::reauthorise(tool, operator)`
exists and is mutation-tested, and `AmbientIntake::revoke` and `reauthorise`
wrap it, but no production code calls either
(`grep -rn '\.reauthorise(\|\.revoke()' backend/crates/apps/*/src` finds
nothing). The deployed revocation is `AgencyPolicy::revoked_tools` in
`qip-cli/src/agency.rs`, re-applied at each start: deleting the entry readmits
the tool on the next start, with no operator named anywhere. The comparison
the row draws is `AutonomyController::request_change`
(`qip-risk-engine/src/autonomy.rs`), which takes an `OperatorIdentity`.
`OperatorIdentity::verified`'s own documentation says it "should be
constructed in exactly one place: the API's authentication middleware", and
that is the only place it is
(`grep -rn 'OperatorIdentity::verified' backend/crates/apps/*/src` finds
`qip-api/src/routes.rs` only).

**Records** (CONTRACT-7080). `GoalSpec` has no authority field and
`GoalSpecDraft` is `deny_unknown_fields`, so a document carrying
`authority_class` is refused; the module documentation and
`qip-contracts/tests/expansion.rs` say so. No ADR records the departure from
the requirement that a goal carries an authority class, and the row closes on
that record.

Two row statements are stale and the code wins. MODEL-4014 says branches come
"from the request's declared structural models, not qip-twin futures";
`qip agency shadow` accepts a `twin` declaration and passes a `TwinScenarios`
as the pass's `ScenarioSource` (`grep -n 'ScenarioSource\|TwinDecl' backend/crates/apps/qip-cli/src/agency.rs`),
so twin futures are simulated beside the declared scenarios when the request
names one. RISK-4103 says the conduct brain is "reached only from qip-cli";
`qip-deepbrain` depends on `qip-agency` and runs the gate through the door.

## Decision

1. **One identity, and no seam for a second.** The door drafts under
   `ACTING_IDENTITY` only; `AmbientIntake` builds each draft's sender from its
   one configured identity and no input reaches that field. The "one draft
   under two identities" clause is *structurally vacuous* on this path and that
   satisfies it (AGENCY-1063). A seam that let a signal choose a sender would
   create the hole the guard's `check_coordinated_voices` and the
   executor-differs-from-approver rule refuse. The refusal stays proven at
   guard level in `qip-agency/tests/agency.rs`; the node-level test stays an
   invariant, not a refusal.
2. **Authority above `Shadow` is unreachable on this path, not pending.**
   ADR 0114 D8 refuses authorised adapters and narrow-reversible actions; ADR
   0141 grants no level above narrow-reversible and the door has no adapter
   but the outbox. So the second halves of AGENCY-7133 and AGENCY-7148 and
   MODEL-4014's "which deployed binary acts" are answered: **none does.**
   `qip-cli` is the only caller of `widen`, `execute` and a non-`Unwired`
   adapter path, and its adapter refuses. `ActionPolicy::observe` runs on the
   ambient path and its verdict is journaled (`authority_before`,
   `authority_after`, the predicted interval); at `Shadow` it cannot narrow
   anything, and it is not required to. Simulation-before-authority stays met
   where authority can be used (`engine::execute` refuses a plan no branch
   supports).
3. **Registration is the `BrainSpec`; the agency is not an Organisation agent;
   `coverage::Scope` gets no ambient scope.** An Organisation agent reads a
   desk and emits `Finding`s that `covered_scopes` counts; the agency reads a
   research sweep and emits drafts. Wrapping it as an agent would require
   inventing a finding the agency does not produce, and a `Scope` variant no
   agent evidences is "a coverage claim about nothing" by that enum's own
   documentation. The registry already makes the brain auditable and its
   removal visible. The every-`Scope`-covered organisation test is unchanged.
4. **No variable-to-federation-Scope mapping is invented, and no scope gate
   runs on drafts** (AGENCY-6082). The ambient path builds no `AffordanceGraph`,
   and a federation `Scope` (domain, region, horizon, abstraction, hypothesis,
   kind) names no variable. A gate keyed on a mapping nobody wrote would
   refuse every draft on a node without a spec for the subject. If a mapping
   is ever wanted, the `FederationStage` asset-class map from instrument to
   domain is the candidate, and it needs its own record and a consumer.
5. **The structured claim rule.** An *assertion* a draft makes is the `claim`
   field of one of its `EvidenceCitation`s, and nothing else. `content` is the
   draft's description of the message or action and is **not** segmented into
   sentences or classified as assertion or action.
   - *Why not parse.* The gate is deterministic and never routes to a model.
     A text rule that tells "Quote 20 lots" from "spreads will tighten" must
     guess; a guess either refuses honest action descriptions or admits
     assertions, and "refuse rather than guess" does not mean "guess toward
     refusing" when the refusal lands on the platform's own routine output.
   - *Rumour is structural.* A rumour is a claim with no recorded
     counterpart: an evidence id that resolves to nothing, a claim that is not
     the recorded statement, or a source not on the authorised list. Checks 1
     and 2 already refuse each when an `EvidenceResolver` is attached. No
     sentence-to-citation binding is built, and `RUMOUR_PHRASES` stays what it
     is: a floor over the vocabulary of hearsay in `content`.
   - *Content is read only against vocabularies the platform owns.* The phrase
     lists and the `HoldingsResolver` (a draft that mentions an instrument the
     platform holds needs the position disclosure) are closed lists keyed to
     platform facts, not judgements about sentences.
   - *The named residual.* A paraphrased promissory statement in operator-
     written `content` ("you will certainly profit") passes check 7, and a true
     claim cited beside it does not stop it. That is a known miss of a phrase
     floor, accepted because the only shipped producer of `content` that
     reaches a person (the ambient door) writes it from typed fields, and the
     operator-authored path is the shadow binary, which executes nothing. It is
     recorded here so it is a stated floor and not a surprise.
   - *What is built:* tests that pin the rule from both sides, so a later
     "improvement" that parses prose fails one of them (see Rows).
6. **What RISK-4103's three checks can honestly be built from, and what
   cannot.** *Externality* (who else a message reaches): no input states it;
   `target_audience` and the channel's permitted audiences are all the draft
   carries, and check 5 already enforces them. *Overclaiming*: no input states
   an assertion's strength beyond `confidence_bps` on a citation, which the
   gate already range-checks; the promissory phrase floor is the only
   detector. *Channel compromise*: nothing in-tree states a baseline of normal
   channel use to deviate from; the identity allow-list, the channel-to-
   audience map, the cross-pass voice fingerprint and `MAX_RELEASES_PER_MESSAGE`
   are the controls. **None of the three is built.** A check with no input is a
   control that cannot fire, and the repository's risk-and-execution domain rule
   names that as the thing not to add.
7. **The kernel's ACT stage does not call the conduct brain.** `qip-kernel`
   has no dependency on `qip-agency`, and ACT places paper orders with a
   simulated broker. Wiring the gate there would put a text-matching refusal on
   the order path: a conduct bound that refuses orders, which is a risk
   control whose structure and values are not this record's to set (listed
   below). The brain gates every `engine::execute` adapter call and every
   ambient draft; that is the reach it has.
8. **Reauthorisation: durable revocation and a recorded, declared name; no
   claim of authentication the process cannot make.**
   - A revocation applied at start is journaled as an agency record naming the
     tool and the policy document hash. At the next start the guard re-applies
     every journaled revocation that no later journaled reauthorisation lifts,
     **whether or not `revoked_tools` still lists it.** Deleting the policy
     entry no longer restores the tool.
   - Restoring a revoked tool is one explicit operation (`qip agency
     reauthorise`) that appends a reauthorisation record carrying the tool, a
     non-blank operator name and a reason of at least ten characters, the same
     floor `request_change` applies. The operator's name is **declared by the
     invoker and the record says it is declared, not authenticated.**
   - The authenticated form, `OperatorIdentity`, is minted by `qip-api`'s
     authentication middleware and no CLI process can honestly mint one; a CLI
     that built one itself would forge the evidence type whose point is that an
     agent cannot. So the "authenticated operator" clause is **recorded as not
     attainable on the CLI path**, and what is built is the part that is: the
     stop is sticky, the lift is explicit, attributed to a name and journaled.
   - The deployed drafting tool has no runtime control surface and none is
     built. Stopping the ambient door in production is stopping or
     redeploying the service. A remote channel that *revokes* is the safe
     direction; one that *reauthorises* needs an authenticated route in
     `qip-api` and a cross-process path to the deep brain, and that is its own
     record.
9. **A goal carries no authority class** (CONTRACT-7080). This record is the
   one that says so: authority is `ActionPolicy`'s (starts at `Shadow`; ADR
   0141 schedules the rungs), a goal document asks for none, and a field named
   `authority_class` is refused as unknown. The requirement's "authority class
   on the goal" is amended to "authority class on the action policy". Rollback
   is carried and validated and nothing yet executes one; that half is
   unchanged.
10. **Planning and tool use in shadow and outbox mode satisfy the "Financial
    AGI core" scope item** (WORLD-1156). The paper boundary forbids an
    executing planner; `qip agency shadow` plans, simulates and gates, and the
    deep brain's door plans one step and drafts it. The item stays *inspection*
    until the two behaviours the register names are pinned by tests named for
    them: hypothesis generation (`stage_reason`) and uncertainty
    (`calibrate_resolved`).
11. **The composite acceptance suite closes when its constituents do, and says
    what it then asserts** (COVERAGE-7190). Constituents: AGENCY-1065 and 1069
    (decision 5, closed by the pinning tests), AGENCY-6105 and 7027 (decided in
    the record on the unidentified ambient effect: the ambient door runs the
    guard, not `engine::run`). The suite asserts, on one run of the real road:
    every written draft carries a guard decision for the drafting tool and was
    executed under the one identity; a signal whose subject trips a mandatory
    conduct rule produces no outbox entry and a counted withholding; the door
    makes no `engine::run` or `run_governed` call; and the learning records are
    `NotIdentified` at identifiability zero. It does not assert the engine's
    stages ran on the ambient path, because they do not.

## Consequences

- The questions on these rows become statements a test can check; seven of the
  fourteen close by restating register wording or by this record alone, and none
  by weakening a guard.
- Code and test slices left, each with a mutation: durable revocation and
  reauthorisation records (RES-7109); the paired claim-rule tests (AGENCY-1065,
  1069); the registry pin (AMBIENT-3027, AGENCY-3028); two behaviour tests
  (WORLD-1156); the composite suite (COVERAGE-7190).
- `qip-cli`'s `revoked_tools` changes meaning from "the revocations in force"
  to "revocations to add": the journal is the record of what is in force.

## What it costs

- The phrase floor's miss (decision 5) is accepted, not closed. A reviewer who
  wants the paraphrase caught gets it only by an operator reading the draft or
  by a future producer that constrains `content` to typed fields.
- Sticky revocation adds state to resume: a journal that cannot be read
  refuses the start instead of ignoring it, which is what `resume` already
  does for the chain, and a stale revocation needs an explicit lift.
- The declared operator name is weaker than an authenticated one, and the
  record says so. An invoker can type any name; what the record preserves is
  that a lift happened, when, under which journal and policy hash, and with a
  stated reason.
- Not building the three RISK-4103 checks leaves "externality", "overclaiming"
  and "channel compromise" named in the blueprint with no detector. That is the
  honest state, and the register row stays PARTIAL on them.

## What would make this wrong

- If a producer began writing the ambient door's `content` from free text (a
  summariser, an operator template with a user-supplied body), decision 5's
  reason that `content` is typed fields would fail, and the sentence-level
  question would be live again.
- If the desk acted on drafts at a rate where a paraphrased promissory line
  could reach a counterparty, the residual in decision 5 would stop being a
  floor. The outbox is read by the owner's own desk; a path from it to anyone
  else falls under ADR 0114 D8 and would reopen this.
- If the CLI's revocation journal were editable without breaking the hash
  chain, durable revocation would be a convention again. `resume` verifies the
  chain first; the check to watch is that new records are inside it.
- If an ambient scope were evidenced by findings the deep brain composes from
  the sweep (a producer that does emit a finding), decision 3's "no agent, no
  scope" would no longer hold and the scope would be earned the way the others
  are.

## Dissent

The strongest counter-position is on decision 5. Sentence-level binding is the
requirement as the blueprint wrote it, a phrase list is a blacklist, and a
blacklist is exactly what an adversary writes around: "guaranteed" is refused
and "you cannot lose" is refused (both are listed), while "you will not lose" is not. A
conduct brain whose only detector of promissory language is a word list is, on
this view, a control that reads as protection while being unable to fire on
the second phrasing, the failure the risk rules call out by name. Binding every
assertion sentence to a citation would make the check structural.

It lost because the binding is not structural: it needs a classifier of
sentences, and a deterministic classifier that cannot tell "Quote 20 lots" from
"spreads will tighten" has to be tuned toward admitting or refusing. Tuned
toward refusing, it blocks the platform's own routine action descriptions;
tuned toward admitting, it adds a check that looks like binding and binds
nothing. The structure that is available is already in the type: the claims are
in the citations, and they are checked against records. What the structure
cannot do is police prose, and this record says so instead of building
something that looks like it can. It is revived by a producer of
operator-authored free text that reaches a person other than the owner's desk;
then `content` would have to be constrained to typed fields at that producer,
which is a structure, not a parser.

A second, smaller dissent is on decision 8: an unauthenticated declared name is
barely better than none, and building the record may give a reader more
confidence than it deserves. It lost on the record's own wording (the field says
*declared*) and on the sticky revocation, which is the part that stops the
silent readmission the row describes.

## Code it constrains

- `qip-deepbrain/src/ambient_agency.rs`: `ACTING_IDENTITY`,
  `PROMOTION_APPROVER`, `PLAN_APPROVER`, `AmbientAgency::over_with` —
  `grep -n 'pub const ACTING_IDENTITY\|pub const PLAN_APPROVER\|pub const PROMOTION_APPROVER\|fn over_with' backend/crates/apps/qip-deepbrain/src/ambient_agency.rs`.
- `qip-deepbrain/src/brains.rs`: `ambient_specs`, `standard`, `CAUSAL_AGENCY`,
  `AMBIENT_ATTENTION` — `grep -n 'pub fn ambient_specs\|pub fn standard\|pub const CAUSAL_AGENCY' backend/crates/apps/qip-deepbrain/src/brains.rs`.
- `qip-investment-agents/src/coverage.rs`: `Scope::ALL` (must name no ambient
  scope) — `grep -n 'pub enum Scope\|pub const ALL' backend/crates/agents/qip-investment-agents/src/coverage.rs`.
- `qip-agency/src/conduct_brain.rs`: `CommunicationDraft`, `EvidenceCitation`,
  `checks::check_factual_support_resolving`, `FALSE_CLAIM_PHRASES`,
  `RUMOUR_PHRASES`, `MANDATORY_RULES` —
  `grep -n 'pub struct CommunicationDraft\|fn check_factual_support_resolving\|const RUMOUR_PHRASES\|pub const MANDATORY_RULES' backend/crates/services/qip-agency/src/conduct_brain.rs`.
- `qip-agency/src/ambient_intake.rs`: `AmbientIntake::attempt` (builds
  `content`), `AmbientIntake::revoke`, `reauthorise` —
  `grep -n 'fn attempt\|pub fn revoke\|pub fn reauthorise' backend/crates/services/qip-agency/src/ambient_intake.rs`.
- `qip-contracts/src/action.rs`: `ToolRegistry::revoke`, `reauthorise`,
  `reauthorised_by` — `grep -n 'pub fn revoke\|pub fn reauthorise' backend/crates/libs/qip-contracts/src/action.rs`.
- `qip-cli/src/agency.rs`: `AgencyPolicy::revoked_tools` and where the session
  applies it — `grep -n 'revoked_tools' backend/crates/apps/qip-cli/src/agency.rs`.
- `qip-agency/src/journal.rs`: `AgencyRecord`, `resume` —
  `grep -n 'pub enum AgencyRecord\|pub fn resume' backend/crates/services/qip-agency/src/journal.rs`.
- `qip-risk-engine/src/autonomy.rs`: `OperatorIdentity::verified`,
  `AutonomyController::request_change` (the model the reason floor copies) —
  `grep -n 'pub fn verified\|pub fn request_change' backend/crates/services/qip-risk-engine/src/autonomy.rs`.
- `qip-contracts/src/expansion.rs`: `GoalSpec`, `GoalSpecDraft` —
  `grep -n 'pub struct GoalSpec' backend/crates/libs/qip-contracts/src/expansion.rs`.

## Rows unblocked

- **R12-AGENCY-1063** — decided: one identity, structurally vacuous, no second
  seam; the node invariant test stands. Restate.
- **R12-AGENCY-1065** — decided: an assertion is a citation's `claim`; prose is
  not parsed; pinned by a test-only slice.
- **R12-AGENCY-1069** — decided: rumour is a claim without a record; the
  phrase list stays a floor; same slice.
- **R12-AGENCY-6082** — decided: no mapping from a graph variable to a
  federation `Scope`, no scope gate; the ambient path builds no graph. Restate.
- **R12-AGENCY-7133** — decided: the deep brain's policy stays at `Shadow`;
  `observe` is journaled and cannot narrow it. Restate.
- **R12-AGENCY-7148** — decided: authorised adapters and narrow-reversible
  actions are unreachable (0114 D8, 0141), not pending. Restate.
- **R12-MODEL-4014** — decided: no deployed binary acts under authority; the
  gap text on twin futures is stale (`TwinScenarios`). Restate.
- **R12-RES-7109** — decided and to be built: durable revocation, explicit
  reauthorisation record with a declared operator name and a reason; the
  authenticated half recorded as unattainable on the CLI.
- **R12-CONTRACT-7080** — decided: this record is the amendment; a goal carries
  no authority class. Doc-only.
- **R12-WORLD-1156** — decided: shadow/outbox planning satisfies the item;
  two named behaviour tests close the rest.
- **R12-AMBIENT-3027** — decided: the `BrainSpec` is the registration; not an
  Organisation agent; pinned by test.
- **R12-AGENCY-3028** — decided: same decision and same pin as AMBIENT-3027.
- **R12-RISK-4103** — decided: externality, overclaiming and channel
  compromise are not built (no input); the kernel's ACT does not call the
  brain. The ACT-side refusal is a RISK matter and is not decided.
- **R12-COVERAGE-7190** — decided: the composite suite is specified here and
  lands when its constituents do.

## Not decided here

- **RISK** — a conduct gate on the order path (kernel ACT or the execution
  engine): any structure that lets a text or conduct rule *refuse an order*,
  the phrases or thresholds it would use, and what it does on a fault. Rows:
  R12-RISK-4103.
- **PAPER-BOUNDARY** — nothing here weakens a layer. Listed so it is not
  assumed: any adapter beyond the internal outbox, any authority above
  `Shadow` on the deep brain's path, and any authenticated remote reauthorise
  route into the deep brain stay with ADR 0114 D8, ADR 0141 and their own
  records. Rows: R12-AGENCY-7133, 7148, R12-MODEL-4014, R12-RES-7109.
- **PROTECTED** — no edit to `.github/**`, the agent-configuration directory, `scripts/check-*`,
  `CODEOWNERS`, the allow-list or the Terraform variables is decided. The
  decisions above touch crates only.
- **BILLED** — nothing creates or enlarges a billed resource.
