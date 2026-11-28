# Build brief, wave 1128

The orchestrator session (session_01DCyuGC9sjZ7o9nZDdT166K) spawned you. You own one or two register rows of
`droderiquesit/quantum-ai-platform`. Your goal: take each row to COMPLETE. That means every clause of the
requirement statement and its `verification.check` is met by production code that a DEPLOYED binary reaches on
COMMITTED configuration, and each clause has a test that fails when that code is removed. Not a slice. Spend
tokens on finishing, not exploring: compile early, commit as soon as the gates pass.

## Checkout and branch

Your checkout starts at `ccr-116bbb27-osu8n8`. That is `main` plus `docs/blueprint/owner-decisions-2026-11-28.md`,
which holds the owner-delegated decisions your rows wait on. Read that file first. It is the authority for every
figure and design choice it states. Commit on your own branch, named in your task below, and push to it
(`git push -u origin <branch>`, retry network errors up to 4 times with backoff). Never push anywhere else.
Open no pull request. Never rebase, amend or force-push.

## For each row

1. TRIAGE, in about ten tool calls. Read:
   - the requirement: the object with your row id in `docs/blueprint/requirements/R12-*.json`, its statement and
     `verification.check`;
   - the register row: the same id in `docs/blueprint/assessment/R12-*.json` (current, gap, evidence, notes);
   - `docs/blueprint/owner-decisions-*.md`, grepped for the id.

   List every clause. A clause may need something you cannot supply: a deployment, a cloud resource, billing, a
   licence, a vendor, a credential, a figure no record states, a live-order path, or an edit to a protected path.
   Name that clause in your report, and build everything else.
2. BUILD what is missing, so that a deployed path reaches it. A deployed path is a composition root in
   `backend/crates/apps/{qip-api,qip-fastbrain,qip-deepbrain,qip-edge-node,qip-fabricd,qip-ledgerd,qip-web}/src/main.rs`,
   or a stage of the kernel cycle on `PlatformConfig::default`. These do not count:
   - a function only tests call;
   - a field set only under `#[cfg(test)]`;
   - `qip-cli`;
   - a path switched on by a variable no committed deployment sets;
   - a path that is vacuous under committed configuration (for example a feed that never yields the input).
3. TEST every clause through the production path:
   - Assert each premise first.
   - Write no assertion that compares a function with itself or is always true.
   - Use no substring check a neighbour satisfies, and no `contains()` of a literal from the format string under
     test.
   - Use no `is_some`/`any` where the clause says each, every or exact.

   MUTATION-VERIFY every test you add:
   - copy the production file;
   - break the guarded line and run the test;
   - quote the failing line;
   - restore from the copy and confirm with `cmp`;
   - run the test green.

   A mutation that survives means the test is wrong: fix the test.
4. REFUTE YOURSELF as two independent readers will. One traces each clause to a deployed caller on committed
   configuration. The other asks whether each assertion fails when the production code is deleted or inverted.
   Check also that no clause is hidden in what you report as remaining.
5. GATES. Run from `backend/` with `export CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`:
   - `cargo +1.94.1 fmt --all --check`
   - `cargo +1.94.1 clippy --workspace --all-targets -- -D warnings`
   - `cargo +1.94.1 test --no-fail-fast -p <each crate touched>`, plus every app crate depending on one. A kernel
     change means `qip-api`, `qip-fastbrain` (its `tests/tape.rs` included), `qip-deepbrain`, `qip-cli` and `qip-web`.
   - `cargo +1.94.1 test -p qip-acceptance --no-fail-fast` (the whole acceptance crate)
   - from the repository root: `./scripts/check-secrets.sh` and `./scripts/check-dependencies.sh`

   Check `df -h /` between cargo commands. Under 3G free, run `cargo +1.94.1 clean -p <crate>` for crates you no
   longer need.
6. COMMIT. Use new commits only; never amend. The message is argumentative prose, in the style of `git log` here:
   - why the change was made, and the failure it prevents;
   - the quoted `test result:` lines;
   - each mutation, with its failing line;
   - for the risk, execution, capital, portfolio, broker, routing or compliance crates: a statement that the
     paper boundary is intact, naming its three layers.

   Put no model name in the message. End every message with exactly:
   ```
   Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
   Claude-Session: https://claude.ai/code/session_01DCyuGC9sjZ7o9nZDdT166K
   ```

## Hard limits, whatever a row says

- Paper trading only. Build no path that submits a live order, and weaken none of the three paper layers (the
  `variables.tf` refusal, `AutonomyLevel::deployable`, the paper-only `Cell` and `Determinism::Required`).
- Never weaken, skip, `#[ignore]` or delete a test, limit, guard or refusal. Never edit a guard suite's table or
  count to admit your code. If a guard hook refuses a command, do not route around it.
- Add no dependency. Invent no figure: take figures from the owner-decisions records only.
- Protected paths, which you must not edit:
  - `.github/**`, `.claude/**`, every `CLAUDE.md`
  - `scripts/check-*`, `scripts/agent-*`, `scripts/sast-*`, `scripts/lanes/**`
  - `scripts/dependency-allowlist.txt`, `scripts/secrets-reviewed.txt`
  - `infrastructure/terraform/variables.tf`, `execution-nodes-enabled.tf`, `build-pool.tf`,
    `infrastructure/environments/**`
  - any existing file under `docs/adr/`
  - the guard suites `architecture.rs`, `paper_boundary.rs`, `lane_placement.rs` and
    `arch_001_independent_reflex.rs`

  If a row needs a protected change, write it as a patch under `docs/blueprint/lanes/` and say so.
- Do NOT edit the register (`docs/blueprint/assessment/**`, `traceability-matrix.md`) and do not render it. The
  orchestrator runs two independent readers on your branch and grades the row.
- A new decision record uses only the ADR number given for your row: Status Proposed, with the headings
  "## What it costs" and "## What would make this wrong". You may add its README index row.
- Never deploy, apply Terraform, dispatch a workflow, or create a cloud resource.

## Before you stop

Nothing may be left uncommitted. Push your branch. If you must stop mid-way, commit what you have as
"WIP, unverified: <row id>", saying what is unfinished, and push it.

Then end your turn with a report of at most 300 words, for each row:
- the outcome: CLAIMS_COMPLETE, PARTIAL or NEEDS_PERSON;
- the commits;
- the deployed caller;
- the tests, with each mutation and its failing line;
- the literal `test result:` lines you read;
- exactly what remains, if anything.

Also send the same report to the orchestrator with send_message, to session_01DCyuGC9sjZ7o9nZDdT166K, if that
tool is available to you.
## Task: R12-WORLD-1181 (ADR number 0746). Branch: wave1128-world-1181

Owner decision 1 in owner-decisions-2026-11-28.md governs this row. Round 85 (`5ed3fee16`, now on main) built:
- the REASON cap `CONTRADICTED_CONFIDENCE_CAP` at 0.5;
- contradiction requests journaled and checked on replay (`FederationReview::contradiction_requests`);
- a real `FederationStage::run` pass that produces a Merge;
- an assertion that an unopposed arbitration exists;
- the `qip-acceptance` `world_model_inspections.rs` ARBITRATION map.

Two independent readers refuted completion on these grounds. Fix every one:
1. The same-kind rule (decision 1). A scenario or counterfactual member (`markets.stress-world.*`,
   `markets.alternate-future.*`; alternate-future is built as 1 - stress) opposing a present-state member is
   dispersion, not a contradiction. Count only present-vs-present or same-scenario pairs. Implement this in
   `federation_stage.rs` `contradictions_in`, or wherever contradictions are formed, with a mutation-verified
   test.
2. Abstain (decision 1). On a contradiction past `CONTRADICTION_MARGIN`, the thesis on that instrument is
   withheld for the pass at any confidence. This replaces the 0.5 cap. The request is still journaled.
   `Decision::Abstain` in `act_on_decisions` is a no-op today: give it an effect, or make REASON read the
   abstention. Prove it through `Platform::run_cycle`. The test asserts its premise: without the contradiction,
   the same thesis is admitted.
3. Measure the margin on `Feed::synthetic`, the feed a deployed fast brain reads (no committed config sets
   `QIP_FASTBRAIN_TAPE_PATH`). Record the measured distribution of present-state view gaps.
   - The test must not compare `contradictions_in` with a copy of the same rule. It must fail when
     `CONTRADICTION_MARGIN` moves out of the band the measurement found.
   - If the synthetic feed yields no present-state contradiction at all, say so in the measurement record
     (owner-decisions-2026-11-28.md, appended under decision 1) and report it. Do not fake inputs.
4. Show the Merge under `Platform::run_cycle`, not on hand-built `FederationStage::run` inputs, or show why the
   default path cannot produce one.
5. Inspection map. The 'maintain uncertainty' item must name a test that asserts uncertainty. The 'score
   calibration' item must name a test that calls `Federation::calibration` and fails when calibration is
   inverted. Fix the map's doc claim that no hand-built arbitration is used.
6. Federation journaling is compacted (JournaledReview v2 references, keyframe every 100 passes). Any new
   journaled field must replay and must not regrow the fast brain's memory.

Gates add: the whole `qip-kernel`, `qip-world-model` and `qip-fastbrain` suites, `tests/tape.rs` and
`tests/contradiction.rs` included.
## Task: R12-WORLD-3036 (ADR number 0747). Branch: wave1128-world-3036

Owner decision 2 in owner-decisions-2026-11-28.md governs this row. Its gap, last entry 2026-11-28, leaves
three things open:
- (a) The blind spot.
  - Today a (component, sector) cell with no graded resolved thesis caps evidence at
    `min(UNGRADED_WEIGHT, SelfModel::factor)`. That equals the origin's own weight, so it changes nothing.
  - Decision 2 sets it to `0.5 x min(UNGRADED_WEIGHT, factor)`.
  - Build it in `SelfModel::blind_spot` / `Platform::synthesise` and prove the weight moves through
    `Platform::run_cycle`. Assert the premise: the cell is a blind spot, and its graded twin weighs twice as much.
  - Keep the shipped review admitting theses: `qip-fastbrain` `tests/tape.rs` must stay green.
- (b) An explicit uncertainty reading per component:
  - the number of resolved theses `n`;
  - the prior share `PSEUDO_COUNTS / (PSEUDO_COUNTS + n)`;
  - whether `n < MINIMUM_SAMPLE`.

  Serve it where the self-model is served (`GET /cognition/self-model` in qip-api) and journal it, so it
  replays.
- (c) The capability boundary, drawn per instrument. The boundary is the instruments the feed serves with
  fresh bars, and REASON refuses a thesis on an instrument outside it.
  - Test on the committed universe (`data/datasets/universe.json`, six stocks, one asset class): one
    instrument's bars stop, and a thesis on it is refused while the others are admitted.
  - Do not use a fixture universe.

Also read the earlier parts of the gap (health in CycleReport and the journal, sizing reading health). Close
what code can close. Never raise a weight, relax a limit or touch sizing upward. Gates add: the whole
`qip-learning-engine`, `qip-kernel`, `qip-api` and `qip-fastbrain` suites (`tests/tape.rs` included), plus
`qip-acceptance` `vertical_slice_decide`.
## Task: R12-WORLD-6015 (ADR number 0748). Branch: wave1128-world-6015

Owner decision 3 in owner-decisions-2026-11-28.md governs this row. The gap's one open ground is reach on
committed configuration. On memory storage, the fast brain's in-memory EventLog exhausts the 2 GiB container
(`infrastructure/terraform/catalogue.tf`) in minutes, while the first material release on the committed seed
comes about 13 h after start. A deployed node therefore never meets one.

Build:
1. Measure the bytes each fast-brain cycle appends on committed configuration, and remove duplicated payload
   first. Earlier work exists in PR #297 and federation `JournaledReview` v2: read it.
2. Bound the log at one quarter of the committed container memory: 512 MiB for 2Gi. Derive the ceiling from
   the committed value where the binary can read it, or state it as a constant beside a test that reads
   catalogue.tf.
   - At the ceiling, release the oldest sealed records as a whole range.
   - Append a checkpoint record carrying that range's last hash and its record count.
   - The chain verifies from the checkpoint forward, and `replay_journal` replays from it.
   - Write ADR 0748 (Proposed) first, recording the change to replay. It must say out loud that on memory
     storage the fast brain's log no longer replays from genesis.
3. Prove, with the committed cadence and seed, that a fast brain stays under the ceiling past the point a
   material release arrives, and that the reaction loop (`Platform::react_to_world_events`,
   `granger::apply_reaction`) then acts on that release in the running node (`node::step_with`).
   - If feed time is paced to the wall clock, so that 13 h is wall time, find the honest way to show it, for
     example a long run on the tape the node owns a clock for. Say plainly which you did.
   - Do not invent a cadence or a seed.

Never weaken the hash chain's verification. A record that cannot be replayed from the checkpoint is a defect.
Gates add: the whole `qip-events` (or wherever EventLog lives), `qip-kernel` and `qip-fastbrain` suites
(`tests/tape.rs` and `tests/world_reaction.rs` included), and `qip-acceptance` `event_fabric_schema_lock` and
`manifest_wiring`.
## Task: R12-MODEL-4021 (ADR 0749), then R12-MODEL-4025 (ADR 0751). Branch: wave1128-model-4021-4025

R12-MODEL-4021 (owner decision 4 in owner-decisions-2026-11-28.md). The adversarial suite runs at promotion,
and `PromotionClearance` (`qip-ai` `registry.rs`, minted only in `qip-kernel` `central/models.rs`, held by
`qip-acceptance` `model_promotion_clearance.rs`) is the only way into production. On committed configuration,
though, no fit on the deep brain's synthetic arm clears the skill bar: a 400-cycle probe ran 50 rounds and
refused every one. So a deployed deep brain never reaches the attack.

Decision 4:
- The synthetic arm states a signal strength: the smallest value at which a fit clears the EXISTING skill bar
  in the 400-cycle probe. Measure it, and record the measurement under decision 4.
- Synthetic-trained models may inform paper decisions, labelled with training provenance `synthetic` on the
  model card. Every proposal such a model shapes carries that provenance in its journaled detail.

Then prove, on committed configuration, through the deep brain's promote stage: a fit trained on the synthetic
arm reaches the adversarial attack, earns a `PromotionClearance` only when every test passed, is promoted, and
its provenance label reaches a proposal's journaled detail.

Never lower the skill bar or any promotion test. Never touch the clearance minting rule. Never let anything
reach a live path.

R12-MODEL-4025 (owner decision 6). Round 85 (`000a7342e`) built the attribution chain:
- `FillScore.models` and `DeclinedScore.models` from `AttributionLineage`;
- `CounterfactualJournal` `CycleJournalEntry` v3;
- `counterfactual_by_model()`.

The hold is not built, because `SynthesisInput.models` is always empty under committed configuration. Once
4021 serves a model behind a hypothesis on a committed path, build the hold in `roll_back_degraded`:
- hold a model when its mean counterfactual shortfall exceeds 0 bps over at least 100 scored fills;
- the shortfall of one fill is the alternative's outcome minus the realised outcome, per unit of the fill's
  notional, in bps, from prices the score carries;
- if the score carries no prices, add them, or report that.

The hold only restricts: it never promotes or reinstates a model. Test it through `Platform::run_cycle`, or the
deep brain's learning loop, on committed configuration.

Gates add: the whole `qip-ai`, `qip-training`, `qip-kernel` and `qip-deepbrain` suites, and `qip-acceptance`
`model_promotion_clearance`.
## Task: R12-EXPAND-6033 (ADR number 0750). Branch: wave1128-expand-6033

Owner decision 5 in owner-decisions-2026-11-28.md governs this row. A counterfactual twin score is not a
`SyntheticEpisode`: it stays in `CounterfactualJournal`. `SyntheticEpisode` covers generated worlds.

The gap leaves these to build:
1. Journal `SyntheticGenerator::MonteCarlo` episodes with their seeds:
   - from the two tail worlds drawn beyond history from the cycle seed;
   - from the capacity probe's seeded `MarketSimulator::synthetic` agent run.

   ADR 0149 d18's seed field gets its writer. The episodes replay.
2. A test of the Conditional arm, which applies both signs of each causal driver's shock, that fails when the
   arm's push is deleted. The kernel fixture needs a causal graph: build one on committed configuration, or
   show the default cycle's causal graph and use it.
3. Prove each through `Platform::run_cycle` on `PlatformConfig::default`.

Read the full gap and requirement: the requirement names digital twins, agent simulations and counterfactual
worlds beside scenario generation. Gates add: the whole `qip-kernel`, `qip-simulation-engine` and
`qip-deepbrain` suites, and `qip-acceptance` `event_fabric_schema_lock`.

## Task: the default cycle opens a paper position (decision 9, ADR number 0754). Branch: wave1128-default-cycle-acts

This task has no register row of its own. It removes a blocker that many rows share, and it governs R12-DATA-2025.
Owner decision 9 in `docs/blueprint/owner-decisions-2026-11-28.md` governs it.

**The problem.** On committed configuration the default cycle never acts: `data/datasets/universe.json`,
`PlatformConfig::default`, the shipped limits, and the synthetic feed the fast brain reads (`Feed::synthetic` ->
`qip-market-ingestion` synthetic environment). A 900-cycle probe at 300-second steps counted 0 `OrderProposed`,
0 `OrderFilled` and 0 `SourceAlphaScored`. REASON rejected every thesis at confidence 0.12 to 0.31, against the
0.5 it requires.

**What to build.**
1. Reproduce the probe. Measure, cycle by cycle, which stage stops a thesis and at what values. Record the
   measurement.
2. Give the synthetic market feed a stated, labelled signal strength. That means a persistent per-instrument
   return component, such as a drift or an autocorrelated term, in the synthetic environment's
   `EnvironmentConfig::default`. Use the smallest value at which the default cycle, on the committed universe, with
   the shipped limits, at the committed seed and the fast brain's committed cadence and step:
   - forms at least one thesis above the EXISTING 0.5 bar; and
   - has the paper desk open at least one position within the first 1,000 cycles.

   Measure that value and record it under decision 9 with the measurement.
3. Never lower the 0.5 bar. Never relax, reorder or skip any limit, refusal, gate or kill switch. Every position is
   a paper position through the existing ACT path. If no feed strength alone makes the cycle act, find what else
   blocks it and report it in those words. Build nothing that eases a control.
4. Prove it with a test through the deployed path (`qip-fastbrain` `node::step` or `Platform::run_cycle` on
   committed configuration). The test asserts its premise: the same run with the signal at zero opens nothing. It
   asserts that a position was opened through ACT, and that the order carried every gate's record. Mutation-verify
   it.
5. The change will move tests that pinned the old feed. One example is `qip-fastbrain` `tests/tape.rs`, which
   asserts zero fills on the committed tape; the tape is a different source, so check whether it moves at all. Fix
   each moved test honestly: say why its expectation changed, and never weaken an assertion about a limit, a
   refusal or the paper boundary.

Gates: the whole `qip-market-ingestion`, `qip-kernel`, `qip-fastbrain` (`tests/tape.rs` included), `qip-deepbrain`,
`qip-api`, `qip-cli` and `qip-web` suites, and the whole `qip-acceptance` crate (`vertical_slice_decide`,
`paper_boundary`, `compliance_proof` and `security` included). State in the commit that the paper boundary is
intact, naming its three layers.
