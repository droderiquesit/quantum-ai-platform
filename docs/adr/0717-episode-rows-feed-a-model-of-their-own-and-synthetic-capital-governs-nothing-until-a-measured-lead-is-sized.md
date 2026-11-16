# ADR 0717: Episode rows feed a model of their own, and synthetic capital governs nothing until a measured lead is sized

- **Status**: Accepted 2026-11-16 under decision 40 of docs/blueprint/owner-decisions-2026-10-31.md (the owner's delegation of 2026-11-15)
- **Date**: 2026-11-16
- **Rows**: R12-MODEL-4015, R12-MODEL-6022

## Context

R12-MODEL-4015 requires that a learning round's window come from a campaign
whose manifest names each internal episode set and each external reference with
licence and hash, and that an unadmitted source be refused at the door. The
register records that the campaign exists: `campaign::assemble` in
`qip-deepbrain` resolves references to admitted connectors or replayed streams,
verifies hashes and emits a manifest. It records the gap too. The window is
built from bars, and `LearningDesk::learn` fits only bars. An episode set named
in the manifest would describe data the fit never read, which is a false
manifest, and the register already corrected one closure proposal for exactly
that reason.

The register adds a second finding. The deep brain's bar model predicts the
next-bar return. A retained episode's realised outcome is the result of a
decision. These are different targets. Forcing episode rows into the bar model
would mix two targets in one fit.

R12-MODEL-6022 requires that an internal synthetic-capital ledger determine a
model's future influence and compute share. `ForecastMarket` and
`Tournament::compute_allocation` compute both and the kernel journals them.
ADR 0149 decision 13 holds that they stay journaled records that govern
nothing, and that any future use narrows and never enlarges (ADR 0125). Its
condition for a narrowing use is a measurement: the influence-weighted
aggregate must beat the unweighted one on resolved questions, scored out of
sample. The register says that measurement is journaled each LEARN that scores
a question, and that the tally resets on restart. It does not say how far ahead
the aggregate must be, or over how many questions.

ADR 0150 (quantum families unbuilt) is untouched.

## Decision

1. **R12-MODEL-4015: build it a stated way.** Retained episodes feed a model
   of their own, a decision-outcome model whose target is the realised outcome
   of a decision. They do not feed the deep brain's bar model, and no episode
   row enters a bar window. The bar model and its campaign are unchanged.

2. **R12-MODEL-4015: the manifest names only what the fit read.** An episode
   set is named in a campaign manifest only by the campaign that produces the
   decision-outcome model's window, as a `DataReference` whose hash covers the
   exact rows read. Naming an episode set in a bar campaign's manifest is
   refused, because the manifest would then describe data the fit never read.

3. **R12-MODEL-4015: order of work.** First the decision-outcome model's
   learning dataset admits episode-derived rows, then the episode-set reference
   is hashed into its manifest. Neither step is taken in the bar path. The row
   stays PARTIAL until both exist and the integration check in the requirement
   runs.

4. **R12-MODEL-4015: sources are admitted at the door, unchanged.** External
   references stay market streams from admitted connectors or replayed streams.
   An episode set is internal and carries no licence of its own, but it
   inherits the most restrictive licence posture of any source it was derived
   from. A set derived from a source whose licensing posture has not been
   evaluated is refused. No new source class and no licensed extract is
   admitted here. A licensed extract is a purchase and a licence, which is the
   owner's.

5. **R12-MODEL-4015: the new model is advisory.** Its output reaches no limit,
   no sizing and no order path. It enters a decision only through the existing
   signed release path of ADR 0149, as any challenger does, so no capital,
   risk or paper-trading control is touched.

6. **R12-MODEL-6022: leave the governing use unbuilt, and need a figure.**
   ADR 0149 decision 13 stands unamended. The synthetic-capital weights and
   the compute allocation stay journaled records that govern nothing. The
   figure needed is the size of the lead the weighted aggregate must show over
   the equal-weight aggregate, and the minimum number of resolved questions
   that lead must rest on. Neither figure is chosen here. They are the owner's
   to give, with the risk desk's advice.

7. **R12-MODEL-6022: the shape of any later governing use is fixed now.** When
   the figures are given and the measurement meets them, an amendment to ADR
   0149 decision 13 may let the allocation narrow the deep brain's fit budget.
   It may never widen it, never raise a model's weight above what it has now,
   and never lower a risk, capital or budget control. A model whose synthetic
   capital falls loses weight and compute share. A model whose capital rises
   gains nothing beyond its present ceiling.

8. **R12-MODEL-6022: the measurement must survive a restart before it can be
   relied on.** The tally resets on restart because `TournamentStage` is not
   persisted. A measurement that resets cannot show a lead over a minimum count
   of questions. Before any amendment relies on it, the tally is rebuilt from
   the journal, which already records each scored question, so the figure is
   reproducible from the event log alone.

## What it costs

- The row 4015 stays open until a second model exists. The decision-outcome
  model is real work: its own target, its own dataset and its own evaluation.
  Teams who wanted the manifest line now will not get it.
- Refusing episode references in bar campaigns leaves a cheap-to-name episode
  unnamed. That is the price of a true manifest.
- The row 6022 stays PARTIAL and the requirement's verification check, that a
  falling model loses weight and compute share in the next decision, is not
  met. Influence and compute are still set as they are today. This is
  deliberate. Governing a decision from a measure that has not yet been shown
  to beat equal weighting would be a control the platform cannot yet justify.

## What would make this wrong

- If a bar model fed episode-derived rows turns out to predict better than a
  separate decision-outcome model, the target argument in decision 1 was wrong.
  A committed comparison would be the evidence, and a new ADR the remedy.
- If the owner gives figures and the weighted aggregate never reaches them,
  then 6022 should be declined as unwarranted. The ledger would remain as a
  record and the row would be closed as declined, not left PARTIAL forever.
- If episodes prove to carry no usable licence lineage, decision 4 would refuse
  every set and the row could not close. The lineage would then have to be
  recorded at capture, which is a change to the episode store and its own ADR.
- If the journal does not hold enough to rebuild the tally, decision 8 cannot
  be met and the measurement must be persisted another way.
