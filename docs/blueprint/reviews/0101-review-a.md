# Review A of ADR 0101

Reviewer: independent Architecture Review Board member (adversarial).
Reviewed: `docs/adr/0101-blueprint-v12-0-and-gcp-v3-0-...md` (read from the
hermes-phase-0 worktree, where it is still untracked), against
`docs/blueprint/v12-delta-master.md`, `v12-delta-gcp.md`, ADR 0099, 0003,
0021, 0023, the `.claude/rules` files, and the tree.

Nothing here was run as a gate; this is a document review. Claims checked
against the tree are marked "checked".

## Verdict: APPROVE-WITH-CHANGES

The structure is right. It authorises nothing, keeps ADR 0003 and 0021
intact, carries ADR 0099's register forward, and reads the owner's "just
perform the work" correctly as a process approval and not a boundary change.
The paper boundary is not weakened by anything written. But it has two false
statements of fact (one about a Rust type, one about a test), one internal
contradiction in the row that matters most (C9), two rows that cite the wrong
or an incomplete standing decision, and a weaker version of the existence
rule than the repository's own.

## Required changes

1. **C14 misidentifies what exists in `qip-contracts` (false claim).**
   Quoted: "Until then the one already in `qip-contracts` is the one that
   exists". Checked: `backend/crates/libs/qip-contracts/src/capital.rs:161`
   defines `pub enum CapitalGrant { Full, Reduced(Decimal), Refused(String) }`,
   the outcome of `CapitalEnvelope::admit`. It is not a grant record with
   amount, strategy, region, expiry. The blueprint's grant record has no
   same-named counterpart there. The nearest wire shape is
   `qip_mesh::spine::CapitalGrantFrame` (used in `event_fabric_schema_lock.rs`)
   and `CapitalEnvelope`. A name collision between an admission outcome and the
   blueprint's grant is a third definition, and the row hides it. Rewrite C14
   to say there are two blueprint forms and two same-named-but-different
   in-tree types, name them, and state which is the locked wire shape.

2. **A named validation test does not exist.** Quoted: "`the_adr_index_links_a_body_for_every_claimed_number` and
   `every_internal_link_resolves` in `qip-acceptance`'s `documentation.rs`".
   Checked: `every_internal_link_resolves` exists (`documentation.rs:804`);
   a grep of `backend/` for the first name (and for `adr_index`) finds
   nothing. Citing a nonexistent guard in the Validation section is exactly
   the failure `00-governance` calls a false statement. Delete the name or
   point at the real test; if the index check is intended, say it is not yet
   written.

3. **C9 contradicts itself on shadow vs paper.** Quoted: "Shadow-only by this
   record. They may be built as simulated logic feeding the Risk Gate." Shadow
   (counterfactual, never reaches the simulator) and paper (orders reach the
   simulated broker through the Gate) are different postures, and the text
   uses "shadow, paper or simulated" interchangeably in the boundary section
   too. "Feeding the Risk Gate" is a paper path. For the Hedge Brain and
   Capital Survival Kernel this decides whether a simulated hedge order may be
   placed. Pick one and say it per item: e.g. Shadow Portfolio Universe:
   shadow; Hedge Brain / Survival Kernel: proposals only, admitted by the Gate
   into the simulator; Reflex packages: see change 4.

4. **C9 bundles the Cognitive Compiler under the wrong standing decision.**
   The Compiler's Reflex packages (§9.13) are not capital items. The
   decisions they contradict are C5 / ADR 0083 (packaging Python-trained
   artifacts for in-process serving), ADR 0008 (cells decide alone) and the
   `Determinism::Required` arm. Quoted: C9 row lists "the Cognitive
   Compiler's Reflex packages" with standing decision "ADR 0003, 0021;
   `risk-and-execution.md`". Move it to its own row or into C5, and state the
   safe form: a distilled package is a deterministic artifact that a cell loads
   only via a reviewed release, and it can never widen an envelope. Also
   `risk-and-execution.md`'s actual line is "Escalating autonomy from a model
   output" prohibited; quote that, not a paraphrase.

5. **C12 is the dangerous row and it is under-specified.** Quoted: "a
   model-risk output may **only restrict**, and it does so by supplying a
   limit the Gate evaluates". Problems: (a) a model-supplied limit is still a
   model output entering the pre-trade path; `Determinism::Required` and
   Principle 4 want the monotonicity to be structural, e.g. the Gate takes
   `min(configured, model_supplied)` and the model-supplied side cannot be
   typed as a raise, not "proven by a record and a test"; (b) shrinking a grant
   below an open position implies a reducing order, which is an order created
   from a model output, so say who creates it (the Gate, deterministically) and
   that nothing is auto-unwound by the model; (c) fail direction: a stale or
   missing model-risk output must leave the configured limit in force, never
   tighten to zero silently and never widen; (d) §31.1's "can veto/reduce
   exposure" for Hedge, Capital and Survival systems, not only Model-Risk, is
   the same question and the row addresses only §14.2. Also the standing
   decision column cites `RISK-020`, which is a requirement, not a standing
   decision, and `01-security-and-safety.md`, which holds the paper layers and
   only mentions `Determinism` in layer 3; the "pre-trade checks never route to
   a model" text is in `10-product-direction.md` and
   `risk-and-execution.md`. Also name ADR 0005 (confidence is arithmetic)
   because the Confidence Governor is a confidence authority.

6. **False "word for word" claim.** Quoted: "its §3-8, 10, 12, 15-21, ...
   match v11.6 word for word". The delta (section A, rows p4 and p8-9) records
   a rename in the §8 sentence ("Financial Superintelligence context
   window") and a wording change in §1.3, and says "match v11.6 word for word
   apart from page-break shifts". §8 is inside the claimed range. Say "match
   apart from the rename of AGI to superintelligence" or drop "word for word".
   The same sentence also counts "23 numbered subsections" and then lists
   §24.9 and §31.1 as additional items, while the delta's 23 includes both
   (9+5+3+3+1+1+1). Fix the count wording.

7. **"Eleven of the 23 match no existing requirement" is stated as a count.**
   The delta says it counted from title greps, "approximate", and its 11
   includes units that are not subsections ("§13.1-13.3 as a set", "§31.1 as
   an item list"). The ADR does say "by the delta's approximate count", which
   is partly honest, but the same figure drives "the completion percentage
   drops". Say "roughly" in the Context too, or drop it until the matrix is
   rescored.

8. **The existence rule is cited in weaker form than it stands.** Quoted:
   "nothing is cited as real until it is shown to be ... until each is checked
   against a first-party source". The rule as recorded
   (`docs/SYSTEM_MAP.md` U24, citing `HERMES_MISSION.md` s2.4) requires the
   item to be "installed or called and version-captured" before it enters an
   ADR. A first-party web page is not that. Also, the rule is not in
   `.claude/rules` or CLAUDE.md, so the ADR is citing an unlocated rule: cite
   the file. Also the list in "Names not verified" must at minimum include
   the rule's own items (Z3, OR-Tools, prost, a Raft crate, Qiskit client
   version); Z3, OR-Tools, prost and Raft are absent from the list although
   C2 and C5 name them. Finally the ADR itself writes "TPU7x (Ironwood)" as
   an apparent fact; keep the unproven wording consistent in the register
   (C5, C10 name TPU as a substrate).

9. **C11 says "no owner cost ceiling" while the delta scores against one.**
   `v12-delta-gcp.md` line 25 uses a "25 USD/day ceiling" to judge "fits";
   ADR 0093/0098 are cited for the owner's cost instruction. The ADR says the
   figure is absent and that an owner ceiling is required. Reconcile: state
   whether 25 USD/day is the owner's stated ceiling (then quote where) or the
   delta's assumption (then say the "fits" column is an assumption and not an
   owner decision). As written an agent can read either way.

10. **C15 mislabels a relaxation as C1.** Quoted: "If the owner means §33 as a
    relaxation, that is C1." Relaxing the control sentence is not necessarily
    live capital; it is a rules-file amendment. Reword: a relaxation of the
    control list requires the owner to amend `01-security-and-safety.md`
    and is not an agent's call. More important, "No requirement is dropped.
    The v11.6 sentence is carried forward as the requirement" needs a
    mechanism: name the requirement ID in the catalogue that holds it (or
    the new `M12` entry that restates it), otherwise it is a promise that no
    row carries.

11. **C13 omits the standing decisions that actually bite.** Scoreboard,
    Forecast Market, synthetic reputation/capital: ADR 0007 (exact
    attribution), ADR 0005 (confidence arithmetic, for the Forecast contract
    and "calibration as SLO"). The delta itself flags both as overlaps. The
    synthetic-capital convertibility question (delta C.6) is a safety
    question and is absent from the register; add it (to C13 or C9) as:
    synthetic capital is never an input to a `CapitalEnvelope` or grant.
    Decision item 5 lists it as an unknown but the register gives it no row.

12. **C2 and C4 cite ADRs that do not state the prohibition.** C4 cites ADR
    0089, which is a retention-class decision, not a ban on Spanner or
    Bigtable (ADR 0099 had the same slip; do not copy it forward without
    noting). C2 cites ADR 0100 as a standing decision the target contradicts;
    ADR 0100 is the in-tree build, which is the *consequence*. Clean the
    column to the decisions that forbid (ADR 0002, 0009, 0012) and put 0100
    in the "resolved by / until then" cell. C7 says "ADR 0091, amended by ADR
    0100": ADR 0100 says it raises the binary count from five to seven under
    0091's own test, which is an application of 0091, not an amendment;
    reword.

13. **Related list and register disagree.** Related names ADR 0081 (Leptos
    retired), which no register row uses, and omits ADR 0006 (C10), 0012 (C2),
    0089 (C4/C13), 0040 ("What this does not do"), 0005/0007 (after change 5
    and 11). Also "Related: ADR 0023 (paper trading; untouched)" misdescribes
    0023: it records that real trading is the intended destination and the
    opening is gated, and "opens nothing". An ARB reader following that link
    will find the owner's destination decision, which this ADR should
    acknowledge in one line, since "paper trading is absolute" and 0023's
    "destination is real trading" sit in the same rules corpus. Say 0023 is
    unchanged and still opens nothing.

14. **Status wording quotes the owner with a typo and ambiguity.** Quoted:
    "i wave all just peform the work". Quote verbatim is defensible, but the
    sentence's reach should be stated once, tightly: it authorises the
    adoption in direction (Decision 1-5) and nothing in the register. The
    paragraph already does this; remove the second restatement in "What this
    does not do" and in "Alternatives rejected" to cut repetition (over-built
    prose, not a safety issue).

## What is sound (so it is not re-litigated)

- Paper boundary: three layers named correctly (Terraform
  `variables.tf`, `AutonomyLevel::deployable`, `Cell` constructors), and the
  Validation section's "git diff shows no change" check is the right shape.
  No weakening found.
- Treating the "market microstructure probe" as `EXTERNAL_ACTION` until
  defined is the correct fail-closed reading.
- Additive requirements under `M12`/`G3`: checked, `blueprint.rs:365-366`
  maps both codes to "v12.0" and "GCP v3.0", and the catalogue holds 1,566
  entries across `requirements/*.json`, as claimed.
- v3.0 "27 named services" matches the delta (`v12-delta-gcp.md:157`).
- C1 correctly says superseding ADR 0003 is the owner's act and that no
  agent-written record can do it.
- C3, C7, C8 are correctly carried from ADR 0099, subject to change 12's
  wording point on C7.
- The §33 dropped-sentence finding is real (delta section A) and is the
  most valuable item in the Context.

## Over-built / missing, briefly

- Over-built: the Names-not-verified section lists ~25 names; the useful
  part is the rule and its checklist. Consider moving the list to the
  delta files and keeping the rule here.
- Missing: no row for the Forecast Market's "synthetic capital" (change 11);
  no statement of what an `M12`/`G3` entry's initial matrix status is (it
  should be `NOT_STARTED` or `BLOCKED`, never scored from the in-flight
  `qip-deepbrain` files, which the ADR rightly says "fit" but do not
  satisfy).
