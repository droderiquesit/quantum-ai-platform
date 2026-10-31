# ADR 0214: While the AI platform is lost, no new cognition is produced, its work queues in a bounded backlog, and installed packages keep trading

- **Status**: **Accepted 2026-10-30 under the owner's delegation** ("Make my
  decision on all lets move", `docs/blueprint/owner-decisions-2026-10-25.md`,
  extending `docs/blueprint/owner-decisions-2026-10-24.md`). The limits of
  that delegation hold here. This record sets no risk number, adds no
  outage threshold, changes no limit, threshold or limit set, and touches
  no paper-trading layer.
- **Date**: 2026-10-30
- **Confirms**: ADR 0008. Cells decide alone, and a cell that cannot reach
  the centre keeps working within its envelope.
- **Extends**: the deep brain's run loop
  (`backend/crates/apps/qip-deepbrain/src/node.rs`), the strategic-service
  gate WORLD-9025 put there (`gate_strategic_service`), and the forecast
  lattice's backlog (`qip_kernel::lattice_stage::LATTICE_MAX_BACKLOG`).
- **Rows**: R12-RES-10130.

## Context

R12-RES-10130 reads: "AI platform loss: no new cognition, existing packages
continue. If the AI/agent platform is lost, no new cognitive/model output is
produced, existing packages continue, and research backlog queues. Check:
integration: Node trades identically with the centre silent."

Lane L6 shipped two tests for the row on 2026-10-24. A skeptic refuted the
promotion, and the findings were correct:

1. The deep brain's outage gate, `gate_strategic_service`, held only the
   lattice's strategic rows. REASON's neighbours went on producing while
   the node was unready: the research sweep, the tick desk, the strategy
   search and the learning desk, which can promote a package.
2. "AI platform is lost" was a proxy nobody had named as the signal.
3. The edge test deployed its strategy with `Cell::deploy_with_pricing`.
   No package was installed through `StrategyInstaller`, so nothing showed
   that an installed package survives.
4. The edge centre exists only behind `QIP_MESH_PEER`. No committed
   environment sets it, so the default path has no centre at all.
5. Nobody had decided what "research backlog" means.

## Decision

1. **The signal is the deep brain's own readiness, and no new threshold is
   added.** The AI platform is lost when the node's status reports itself
   `Halted` (the kill switch) or `PersistentlyFailing` (more consecutive
   cycles failed to traverse the loop than the configured failure
   tolerance). Both are already measured, and `/ready` takes the node out
   of rotation on each. `NodeStatus::self_observed_outage` reads the two
   independently of `unready`'s first-match order, so a node both stalled
   and failing is not read as healthy. `cognition::ai_platform_lost` is the
   one statement of the predicate, and `gate_strategic_service` now calls
   it, so the lattice and the cognitive loops cannot disagree.

   The deep brain is the platform the row means. It is the only process
   that hosts the agent roster, the only one that may construct a hosted
   language model (ADR 0037), and the only one that searches, fits,
   researches and replays.

   Three readiness states are not a loss.
   - `Warming` is excluded for the reason WORLD-9025 gave: every node
     warms, and holding its first rounds would make a backlog that no
     outage caused.
   - `Stalled` is excluded because the run loop asks the question at the
     top of a cycle, so it has just come back. A stall seen from inside the
     loop is a clock that jumped between cycles, not a platform that
     stopped thinking: a tape whose periods are a day apart, a suspended
     process, a test clock moved a day. A wedged node cannot ask.
   - `Stopping` is excluded because it is set only after the loop has
     ended. A loop that reads it is a second run over a finished status,
     and a stop is a request to end, not a loss.

   The probe, on its own thread, still reports all three. Two existing
   deep-brain tests found the last two when the first draft of this record
   included them. A tape-driven learning test read a day-long tape period
   as a stall. An ambient-journal test that runs the loop twice over one
   status read the first run's `stopping` as an outage. The strategic gate
   used to count both, and no longer does. Before this record, a tape run
   held the lattice's strategic rows on every cycle whose period was longer
   than the stall ceiling.

2. **While the signal holds, no loop that produces new cognition runs.**
   The run loop reads readiness once per cycle and holds four loops:
   - the evolution engine's search round and champion contest
     (`CognitiveLoop::Search`);
   - the learning round, which fits models, scores drift and promotes
     packages (`Learning`);
   - the research sweep, with its scout hand-off, ambient journal records
     and agency drafts (`Research`);
   - the tick desk's round of replay, labels, models and fused priors
     (`Tick`).

   Each round that was due on the cycle, by the loop's own cadence
   predicate, is written to the research backlog instead of being run.
   Those predicates are `EvolutionEngine::{search_due, learning_due}`,
   `ResearchLoop::due` and `TickDesk::due`. Each `maybe_*` now calls them
   too, so there is one copy of each cadence.

3. **"Research backlog" is `cognition::ResearchBacklog`.** It is a bounded,
   hash-chained journal of `HeldWork`: the cycle, the instant, the cause and
   the held loops. Its capacity is `LATTICE_MAX_BACKLOG`, the bound the
   kernel already puts on the strategic questions the same outage holds. One
   outage gets one bound, not a second number; at the default five-minute
   cadence that is more than three days of held cycles. It is journaled
   beside the platform's event log (`<log>.backlog.jsonl`) or in memory when
   that log is. It reopens on restart with what was held and not drained.
   A journal whose drains are out of order, whose chain is broken, or that
   holds more than its capacity stops the process. `main.rs` always attaches
   it.

4. **When the signal clears, the backlog drains in the order it was held,
   before the cycle's own rounds.** Each held cycle is handed back to the
   loops it was held from, with its own cycle number. A drained round
   therefore runs exactly where its cadence placed it, over the evidence
   available at recovery. Within one recovery cycle, every drained search
   and learning round runs before every drained sweep, which runs before
   every drained tick round. This keeps the order the loop already runs in.
   Each drain is journaled before the work is handed back.

5. **Refuse, never clamp, and fail closed.** A full backlog refuses the
   held work, journals a `Refused` record and reports it on the cycle line.
   It never evicts older work. A refused, unqueued or unjournalable round
   is still not run, because a full queue is not a reason to think while
   the platform is lost. With no backlog attached, a case `main.rs` never
   produces, the loops are still held and the cycle line says nothing was
   queued.

6. **Existing packages continue.** The decision cycle is not held. SENSE
   through LEARN run every cycle on the strategies and models already
   installed. The tick desk's last fused priors keep narrowing REASON as
   before, the warehouse keeps shipping resolved outcomes, and source
   discovery, re-fetch and source scoring keep running. Those are checks
   over data and produce no model output. Nothing in this record promotes a
   package or withdraws one. On the edge, a package installed through
   `StrategyInstaller` keeps trading whether or not a centre exists.

7. **The edge half is held on the default path.** That path has no
   centre, since no committed environment sets `QIP_MESH_PEER`. Two nodes
   are driven through `DecisionLoop::turn`, each with the same package
   installed through `StrategyInstaller` and the installer riding on the
   loop as `main.rs` wires it. One has a centre that answers every poll and
   has nothing new to say. The other has no centre and no mesh worker. Their
   turns, the orders the venue took, the refusals journaled and the shipped
   journal chain are identical, and the package is deployed after every
   turn in both.

## What changes and what does not

- **Changes.** On a deep brain that is unready for any reason but warming,
  the search, learning, research and tick rounds stop and their due work is
  journaled. When the node is ready again, the work runs in order. The cycle
  line gains a cognition line. One file is added beside the event log. A
  poisoned status lock is now read through rather than skipped, so a panic
  in a health handler can no longer leave the strategic service up by
  default. The strategic gate stops treating an in-loop stall as an
  outage, as decision 1 explains.
- **Does not change.** The decision cycle and its stages. Every risk limit,
  threshold, limit set, capital grant and multiplier. Readiness itself, and
  its stall ceiling and failure tolerance. The edge cell and its pass. The
  three paper-trading layers: Terraform's refusal of the live rungs,
  `AutonomyLevel::deployable` in the composition roots, and the edge
  `Cell`'s paper-only constructors.

## What it costs

- **Research stops during exactly the outages an operator most wants
  explained.** A halted or persistently failing deep brain produces no
  sweep, no anomaly finding and no drift score until it recovers, and the
  operations loop's anomaly count for those cycles is absent. The backlog
  records what was not done. It does not do it.
- **A halt is an AI-platform loss.** An operator who trips the deep brain's
  kill switch also stops its research, search and learning for as long as
  the halt lasts. This is deliberate: one predicate for both gates. It
  means a halt held only to stop trading costs research too.
- **Drained rounds answer old questions on new evidence.** A sweep held
  from cycle 3 and run at cycle 5 reads the platform as it stands at cycle
  5. The backlog preserves *that* the question was due and in what order,
  not the evidence it would have had. A long outage drains as a burst of
  rounds in one gap. The burst is bounded by the capacity, but it can make
  that one iteration long, and the operations loop may read it as a
  level shift.
- **Past the bound, held work is lost to the queue.** It is recorded as
  refused, but it is not queued. More than `LATTICE_MAX_BACKLOG` held cycles
  is the outage the bound exists to stop being a store.
- **REASON's narration is not held, because there is nothing to hold.** No
  production path calls `AgentRuntime::complete`, and no hosted model is
  installed (owner decision 6, 2026-10-25). The kernel's deterministic panel
  goes on producing theses over installed packages, and this record reads
  that as existing packages continuing, not as new cognition. If a model
  call is ever wired into REASON, it must be gated on
  `ai_platform_lost`, or this record is wrong.

## What is still unmet, stated plainly

- The deep-brain signal is the node's own observation of itself. A deep
  brain that is gone entirely, with the process dead, observes nothing. In
  that case "no new cognition" holds trivially and the queue does not fill,
  because nothing runs to fill it.
- The edge's centre is absent on every committed path. "Identical with the
  centre silent" is therefore proven against a present-but-quiet centre in
  a test, and the deployed path is the silent one. Nothing is deployed
  (`execution_nodes` is empty or disabled in every environment).

## What would make this wrong

- A cognitive loop running on a cycle that began with the node unready for
  any reason but warming.
- A second statement of the loss predicate, or of any loop's cadence, that
  can disagree with the first.
- A backlog that evicts, clamps or silently drops held work, or a loop that
  runs because the backlog is full.
- A package promoted, or one withdrawn, because the AI platform was lost.
- An edge turn whose pass, orders or refusals depend on whether a centre
  exists.

## Evidence

- `backend/crates/apps/qip-deepbrain/src/cognition.rs`:
  `ai_platform_lost`, `CognitiveLoop`, `HeldWork`, `BacklogRecord`,
  `ResearchBacklog::{open, hold, drain_next}`, `RESEARCH_BACKLOG_CAPACITY`.
- `backend/crates/apps/qip-deepbrain/src/node.rs`: `run_with`'s one
  readiness read, the hold, the enqueue and the drain; `research_pass` and
  `tick_pass`.
- `…/src/node.rs::tests::while_the_ai_platform_is_lost_no_loop_produces_new_cognition_and_the_backlog_queues_in_order`.
  Six cycles run through `run_with` with the engine (search and learning
  every cycle), the research loop, the tick desk and the backlog attached.
  Cycles 1 and 2 run all four loops. After cycles 2 and 3 the status folds
  in a finished cycle marked halted, as the loop folds in a cycle the kill
  switch stopped, so cycles 3 and 4 each begin `Halted`. No loop produces
  anything, and all four are held and queued (pending 1, then 2). Cycle 5
  begins ready, drains cycles 3 and 4 in order, then runs all four loops.
  Every cycle traverses every stage. The journal reads held 3, held 4,
  drained 3, drained 4, and its chain verifies. Three mutations were run,
  each failing the test: research run for the cycle in hand while lost
  ("the research loop swept while the AI platform was lost"), the enqueue
  dropped (pending 0 against 1), and a learning round run while lost ("a
  learning round ran — and could promote a package").
- `backend/crates/apps/qip-edge-node/tests/reasoning_outage_loop.rs::the_node_trades_identically_with_the_centre_silent`.
  The test is described in decision 7. One mutation was run: making
  `DecisionLoop::turn` run its pass only when a mesh worker is attached
  fails it at "turn 0 differs with the centre silent".
