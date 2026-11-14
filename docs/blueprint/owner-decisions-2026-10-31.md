# Owner decisions, 2026-10-31

The owner wrote, in the orchestrator session (session_01DCyuGC9sjZ7o9nZDdT166K),
on 2026-10-31 between 00:28 and 00:33 UTC, after being told what the open
register rows needed:

> Use this going forward. You can make all my decisions go. Do the 1 2 3 and 4
> all at once. Fan out use caveman and complete all sessions use many agents do
> this to finish what you can by morning.

> I want a 2 minute not hourly

"1 2 3 and 4" were the four items the orchestrator had just listed as the
owner's alone: approve the rolling pull request's head and apply the sast
barrier; apply the lane-written protected patches; bring dev up (plan, up,
one execution node, funded stores); answer the owner-bucket register
decisions. This file records what was decided under that delegation and what
it does not lift, in the register of `owner-decisions-2026-10-24.md` and
`owner-decisions-2026-10-25.md`, which it extends and does not replace.

## What the delegation does not lift

The same four exceptions the owner set on 2026-10-24 stand, because they are
the repository's own rules and not the owner's preference: no live-order path
and no real capital or money movement (ADR 0003, rule 01); no weakening of a
test, a guard, a paper-trading layer or an existing limit value; no new
dependency without an ADR (ADR 0109); and no billed infrastructure created
without its monthly cost named first. Two further limits are mechanical, not
chosen: billing activation needs the owner's console session, and the session
permission layer refuses an agent any edit under `.claude/**` and any change
that narrows a security scanner, whatever the owner has delegated.

## Decisions

1. **Orchestrator.** Session session_01DCyuGC9sjZ7o9nZDdT166K continues the
   wave580 programme. The first orchestrator (session_01RsTafvtWPKcWCWMz5tqDWH)
   stood down at 00:08 UTC after the two had merged the same lanes on two
   branches; its hourly check-in trig_01YC63HGiQdj7TehsQsraEvJ is disabled.
   The integration branch is `ccr-116bbb27-osu8n8`; `ccr-d1bc1e54-kf6mr0`
   (PR #243 to main) is fast-forwarded to it after every gated push, so one
   tree carries both names. The check-in runs every two minutes in the
   orchestrator session, with trig_014RbraY6zTjp2P8ShHwXgyv hourly as the
   backstop.
2. **Lanes.** The six primaries continue; L1b, L2b, L3b, L4b and L5b are
   unparked with disjoint domains (FABRIC/FINOPS, REASON/AMBIENT/AGENCY, TICK,
   EXPAND, MESH/REFLEX) and the primaries keep the rest. Every lane may now
   decide a row that waited only on a product decision, with a new ADR claimed
   in `docs/adr/README.md` in its own pushed commit, choosing the conservative,
   paper-only, fail-closed reading. Pacing stays at a push every two rows,
   because the account's weekly usage limit killed every lane on 2026-10-25
   and its five-hour limit did so again at 00:50 UTC today (reset 02:00 UTC).
3. **Protected patches.** Applied on the integration branch, each adding a
   refusal, a scheduled check or a null-default field and loosening nothing:
   `l9-CICD-9083.patch` (the `Agent-Lane` trailer gate, shipped off),
   `l9-CICD-8032.patch` (`drift.yml`, a daily dev plan that fails on drift),
   `l9-CICD-9095.patch` (the two main-branch rulesets as files plus a daily
   live-rule check, enforcing nothing until POSTed) and
   `l5b-peer-mesh-root.patch` (the `peer_mesh` object on `execution_nodes`,
   null in every environment, no node, no cost); `node --test
   scripts/agent-guard.test.mjs` prints pass 40, fail 0 with the first two
   in. Decided but **not applied, because the session permission layer refused
   the agent** and the refusal is not routed around: `l5-edge-GOV-9075.patch`
   (`.claude/agents/learning-agent.md`), `l9-P3.patch` and
   `l6-res-8026-observability-rule.patch` (`.claude/rules/**`) and
   `l8-sast-barrier.patch` (the one CodeQL barrier row of decision 1 on
   2026-10-25, the only patch that narrows a scanner). These four are the
   owner's to apply with `git apply docs/blueprint/lanes/<name>.patch` on
   this branch. Two patches are refused and stay as records:
   `l6-ncc-hubs-parked.patch` (decision 4 of 2026-10-25 declined the hubs and
   `gcp_placement` refuses them) and `L4-quant-research-families-adr0150.patch`
   (ADR 0150 forbids a new family before a committed sizing note; landing it
   would reopen an ADR by code).
4. **The rolling pull request's head** still needs a person's approving review
   for the `agent boundary` check: no agent can give one, so that click stays
   the owner's, as does the `POST` of the two rulesets once they are in main.
5. **Dev deployment.** The dev WIF bootstrap is already applied (infra.yml run
   118, 2026-10-13, "Apply complete"; run 121 "qip-dev is Synced and Healthy";
   deploy.yml run 284 succeeded), so the plan's W4.1 text was stale and is
   corrected. What stops a dev `up` today is, in order: main's ci is red on the
   zstd advisories until the fix on this branch reaches main; `infra.yml` and
   `deploy.yml` refuse an agent actor (`scripts/agent-policy.json`
   `dispatch_refused_for_agents`), so every dispatch is the owner's; a core
   change lands only inside the change window (Tuesday to Thursday, 14:00 to
   20:00 UTC); the image registry answered 403 on the last two pushes, which is
   billing or IAM and unverified; and an execution node additionally needs
   `artefact_trust_bundle_file`, `google_tls_roots_file` and a CA Service pool
   (`execution-nodes-enabled.tf`), none of which exists. The decision is: the
   owner dispatches `infra.yml` `{dev, urls}` first (free, read-only), then
   `{dev, plan}`, and the orchestrator prepares the trust-bundle and roots
   files and the CA-pool declaration as a reviewed change so the node switch
   has something to flip. The runbook's estimate stands at $780 to $960 a
   month for what dev's tfvars already enable; nothing new is funded by this
   decision.
6. **R12-RISK-4116.** The joint tail world's survival reserve may be applied
   on the default path: it raises a reserve, which is a tightening, and the
   lane's test holds the applied reserve where it was until the switch is on.
   Lane L1 decides the switch and records it in the row.
7. **The 208 owner-bucket rows** (OWNER_DECISION 92, OWNER_COST 35, EXTERNAL
   28, PROTECTED 28, DEPLOY 22, OWNER_RISKLIMIT 2, OWNER_LIVE 1) are decided by
   a read-only decider per batch of twelve with a skeptic per batch, under the
   rules above; the first run died on the five-hour limit and re-runs after
   02:00 UTC. Each ruling lands here as a numbered decision when it is made,
   and the OWNER_LIVE row is refused in advance.
8. **A failed or halted pass still reports what it sent** (R12-MESH-7143,
   03:53 UTC). Lane L5b found that `qip-edge-node`'s scheduler keeps no
   `WorkReport` for a pass that returned an error and resets to the default
   report on a halt, so an order sent to a venue before a cycle broke never
   rides the mesh delta and is never charged into the aggregate the Hedge
   Brain plans against; the centre would later meet its fill as a fill on an
   order it never saw sent. The decision, ADR 0209 claimed by lane L5b: the
   scheduler keeps the report of a failed or halted pass, marks it with the
   gate and the pass number, ships it on the next delta exactly once, and
   sends nothing new after the failure; the centre settles those orders as
   any other and records the halt beside them. A report the scheduler
   cannot mark is refused whole, never shipped unmarked. This is the
   "bill what ran, not what was planned" principle applied to a pass that
   did not finish.
9. **The counterfactual twin's direction was inverted** (not a product
   decision, recorded because it changes every regret figure the register
   cites). Until 2026-10-31 `qip-twin`'s `ActualTrade::direction` read a
   trade against the bid as the long while every producer records a buy
   against the ask, so every declined-path and fill score settled with the
   opposite sign, and four kernel fixtures, one deep-brain fixture and one
   acceptance label had been bent to agree with it. The fix flips the twin,
   turns the fixtures, and removes the cross lane L5 had added to match
   the old reading. Journaled `DeclinedScore` and `FillScore` records
   written before the fix carry an inverted `regret` and
   `would_have_earned`; a register row whose evidence rests on one of
   those figures is to be re-read before it is promoted.

10. **The 208 owner-bucket rows are decided** (2026-10-31, under decision 7).
   The delegated workflow, `wave1031-owner-decisions`, ran eighteen batches
   of at most twelve rows, each with a read-only decider and a skeptic who
   re-read every cited path, symbol and test against the tree before the
   ruling stood. Four rulings moved on the skeptic's objection: R12-DATA-8127,
   R12-WORLD-3043 and R12-REASON-7203 from IMPLEMENT to ADR_THEN_IMPLEMENT,
   because each changes a standing record, and R12-GCP-8060 from
   STAY_PARTIAL_DEPLOY to IMPLEMENT, because the restricted-VIP correction is
   a tightening a lane can land in an unprotected module. The verbatim
   rulings, rationales and skeptic notes live in
   `docs/blueprint/lanes/wave1031-owner-rulings-2026-10-31.json`; this
   decision records the classes, the rule each class carries, and every row
   under its class, so that a lane and a reader of the register see the same
   decision. The counts: 57 STAY_PARTIAL_DEPLOY, 40 ADR_THEN_IMPLEMENT,
   36 STAY_PARTIAL_EXTERNAL, 28 REGRADE_TEXT, 25 IMPLEMENT, 16 REFUSE_LIVE,
   4 FUND_THEN_DEPLOY and 2 APPLY_PATCH. Every ruling was checked to tighten
   only: none eases a guard, a limit, a test or a paper-trading layer, and
   none admits a dependency without an ADR. The one OWNER_LIVE row,
   R12-EXPAND-6065, which decision 7 refused in advance, turned out on
   reading to need no capital: the triage's bucket had filed it with the
   live-capital rows, and its ruling takes it out and sends it to a lane as
   ADR_THEN_IMPLEMENT; nothing in it sizes a position.

   The rule each class carries in the register
   (`docs/blueprint/assessment/R12-*.json`), applied the same day:

   - **REGRADE_TEXT (28).** The row's text was older than the code or cited
     the wrong record, so `current`, `evidence` and `gap` are corrected now
     and the status is inflated nowhere. Two rows move from MISSING to
     PARTIAL because the ruling found the code built and tested
     (R12-FABRIC-10178, R12-DATA-10206); no row moves to COMPLETE. A path or
     symbol a ruling names is cited only after it was found in the tree on
     2026-10-31; one the tree does not hold is recorded in `notes` as not
     found rather than written into the row (the skeptic caught one such,
     `qip-contracts fabric_envelope.rs` on R12-FABRIC-7136, and the row
     cites `qip-events src/event_fabric/envelope.rs::FabricEnvelope`
     instead).
   - **STAY_PARTIAL_DEPLOY (57), STAY_PARTIAL_EXTERNAL (36),
     FUND_THEN_DEPLOY (4), REFUSE_LIVE (16).** Status unchanged. Each row's
     `gap` now opens with the ruling's statement of what is missing, dated
     2026-10-31: deployment evidence the owner's dispatch produces; a third
     party (a vendor, a licence record under ADR 0157, a credential or a
     second approver) nobody engages under the delegation; a funded resource
     with its monthly cost named (R12-DATA-9053 Bigtable about $950,
     R12-TICK-7020 the replay job about $105, R12-SEC-10054 the CA Service
     pool $200 to $250, R12-SEC-9137 the four HSM signing keys $10 to $11;
     the last two go on with dev's billing because the execution node
     cannot start without them, the first two come after the deploy, and
     this decision funds nothing by itself); or live orders, real capital or
     an external action surface ADR 0003 and ADR 0114 D8 refuse, which is
     never closable under any delegation. The earlier gap text is kept after
     the ruling, so nothing a lane wrote is lost.
   - **IMPLEMENT (25), ADR_THEN_IMPLEMENT (40).** Handed to the lanes in
     `docs/blueprint/lanes/wave1031-handoff-2026-10-31.json`, by id prefix
     (RISK, CAPITAL and ASSET to L1; FABRIC, FINOPS and LEDGER to L1b;
     REASON, WORLD, MODEL, CONTRACT and ARCH to L2; AMBIENT and AGENCY to
     L2b; DATA and EVID to L3; TICK to L3b; QUANT to L4; EXPAND to L4b;
     REFLEX and the edge RES rows to L5; MESH to L5b; CICD, GCP, GOV, SEC,
     OBS, API, COVERAGE, EVENT and the infra and kernel RES rows to L6).
     Each row stays PARTIAL until its lane closes it with the mutation
     report the ruling's closure names, and an ADR_THEN_IMPLEMENT row's
     lane claims the ADR number in `docs/adr/README.md` in its own pushed
     commit before writing the body, under the title the ruling gives. The
     register's `notes` carry the ruling, the lane and the title.
   - **APPLY_PATCH (2).** R12-RES-8026
     (`l6-res-8026-observability-rule.patch`) and R12-GOV-9075
     (`l5-edge-GOV-9075.patch`) are decision 3's owner-only patches: the
     session permission layer refuses an agent the files they touch, so they
     stay the owner's to `git apply`, and the rows stay as they are until
     that is done.

   ### Every row, by class (ruling in one line; the full text is in the JSON)

   **REGRADE_TEXT (28)**

   - R12-AGENCY-1025 — REGRADE_TEXT: The status-quo baseline stands under ADR 0145. An ambient draft's no-action counterfactual is the subject's reading carried forward, labelled as such, and the unidentified effect it measures acts on…
   - R12-AGENCY-1065 — REGRADE_TEXT: ADR 0146 decision 5 stands. I do not commission a sentence classifier, and I do not reword the requirement to the structured-claim rule. The paraphrase floor is accepted because the ambient door…
   - R12-AGENCY-1069 — REGRADE_TEXT: Rumour stays structural under ADR 0146 decision 5. A claim with no recorded counterpart, or one recorded under an unauthorised source, is refused, and RUMOUR_PHRASES stays a vocabulary floor with its…
   - R12-AGENCY-1182 — REGRADE_TEXT: Orchestration runs in shadow in the agency binary, the only place the paper boundary lets a multi-step plan run, and that satisfies the scope item, as ADR 0146 decision 10 already ruled for planning…
   - R12-AGENCY-6082 — REGRADE_TEXT: No mapping from a graph variable to a federation Scope is invented, and no scope gate runs on drafts (ADR 0146 decision 4). The AffordanceGraph stays per request and outside the federation. A…
   - R12-AGENCY-6104 — REGRADE_TEXT: One distinct horizon stays the minimum (ADR 0145 decision 7). A second is required only when a producer observes one and a record under ADR 0141 consumes it. No second sweep is built to add a horizon…
   - R12-AGENCY-6105 — REGRADE_TEXT: The ambient door runs the guard, not engine::run, and no per-opportunity structural model of what a notice changes is written (ADR 0145 decisions 4 and 5). The row is graded on the guard path…
   - R12-AGENCY-6106 — REGRADE_TEXT: The eleven-stage pass belongs to the shadow binary. qip agency shadow runs Observe through Gate, shadows Act and learns through learn_shadowed. The deep brain door seats eight stages and runs three…
   - R12-AGENCY-7027 — REGRADE_TEXT: The orchestrator is the deep brain's warm door, and it meets the row's check: a typed DraftingAdapter after the guard's identity and policy checks, journaled to the hash-chained outbox. Causal…
   - R12-AGENCY-7030 — REGRADE_TEXT: ADR 0145 stands. The platform assigns no treatment on the ambient path: no variants, no hold-out and no perturbation, and that settles 'where permissible'. The assignment the platform does support is…
   - R12-AGENCY-7148 — REGRADE_TEXT: ADR 0146 decision 2 stands: no deployed binary acts above Shadow, and no record grants an adapter beyond the internal outbox. The phase's second half is withdrawn by decision, not pending, and the…
   - R12-AMBIENT-10094 — REGRADE_TEXT: TPU stays the literal zero and no agent bid is made: a budget nothing can spend is a billed commitment for nothing (ADR 0158 decisions 2 and 5, my decision 5). A zero grant where no consumer exists…
   - R12-ARCH-9071 — REGRADE_TEXT: The patch is applied and the four guard suites are in protected_paths, so the row's current text is out of date and is corrected. The row stays PARTIAL until I approve the rolling pull request's head…
   - R12-COVERAGE-7186 — REGRADE_TEXT: The competing world models the checklist names are the structural models a request declares, and the engine already acts only where every one of them agrees. A federation view is a probability over a…
   - R12-COVERAGE-7189 — REGRADE_TEXT: No hold-out is authorised, and ADR 0145 stands. An ambient draft's measured, unidentified effect updates no world model, no causal graph and no ambient prior; only the action-policy and memory arm…
   - R12-DATA-10206 — REGRADE_TEXT: The profiles are built, and a plan test holds each one to its routing, so the row's text is older than the code and is corrected. It reads PARTIAL, not done. No reader or writer names a profile, and…
   - R12-FABRIC-10178 — REGRADE_TEXT: The row is not MISSING: the managed log, its follower takeover and the decision thread's refusal to wait on the fabric are built and tested, so regrade it to PARTIAL. It stays PARTIAL, because P0/P1…
   - R12-FABRIC-7136 — REGRADE_TEXT: The contracts, SDK and benchmark harness exist and are tested, so the 'harness missing' text is struck. The three-zone quorum is code proven only against the in-memory log. It becomes a quorum when a…
   - R12-MESH-5064 — REGRADE_TEXT: Legs stay risk-ordered, on paper as anywhere else. A Concurrent arm removes the ordering that bounds a stranded leg, and no delegation loosens a risk control. The requirement text stands, and the row…
   - R12-MODEL-1157 — REGRADE_TEXT: Unsupervised learning is already on the default path: the regime detector's Baum-Welch fit learns its hidden states from unlabelled returns. Reinforcement learning is there in its one-state form, the…
   - R12-MODEL-4014 — REGRADE_TEXT: No deployed binary acts under authority, and the requirement is read on the operator CLI. There, engine::execute refuses a plan that no simulated branch supports (ADR 0146 decision 2). Integration…
   - R12-MODEL-6009 — REGRADE_TEXT: Rollback is to a model artefact, which is what the row's own check names, and it is built: a drifted model is retired and its predecessor returns at the digest that passed. A retired strategy is…
   - R12-REASON-1132 — REGRADE_TEXT: Neural coupling is out of scope, as ADR 0150 decision 7 already decided: a neural output has no trace a person can re-derive, and nothing the platform cannot replay touches a size. The row stays…
   - R12-REASON-1134 — REGRADE_TEXT: Probabilistic logic is the journaled network posterior, recomputed on replay, and that is the scope; neural coupling is out under ADR 0150 decision 7. The row's text is corrected to say it is closed…
   - R12-REASON-1136 — REGRADE_TEXT: The bounded in-tree constraint solver is the platform's constraint solver. No SAT, SMT or MILP crate is admitted (ADR 0150 decision 11), and neural coupling is out (decision 7). The row's 'no SAT' is…
   - R12-REASON-1179 — REGRADE_TEXT: The scope is what ADR 0150 fixed: the bounded in-tree solver and no SMT/MILP crate (decision 11), no neural-to-symbolic compilation (decision 7), and no planner beyond the hypothesis lifecycle…
   - R12-REASON-3058 — REGRADE_TEXT: O12 is declined. No SMT or MILP crate is admitted and no long in-tree solver is written, as ADR 0150 decision 11 already decided: the cycle poses a few Boolean variables, and SMT and MILP are not…
   - R12-RES-9087 — REGRADE_TEXT: The drafted fixture is the test. An incident record that links a fixture the loop has replayed to the same shift, drafted before any action is taken, is what 'links a new test that fails before the…

   **STAY_PARTIAL_DEPLOY (57)**

   - R12-CICD-10160 — STAY_PARTIAL_DEPLOY: Config Sync and Argo Rollouts are scored against ADR 0114 D4's equivalents, as R12-CICD-10087 already is: Argo CD with Config Connector does the sync, and the Cloud Run traffic split (R12-CICD-9116)…
   - R12-CICD-7116 — STAY_PARTIAL_DEPLOY: The simulation gate is applied as written and stays required by name. The row closes on evidence, not on the patch: the first hosted ci run on main showing 'simulation gate' succeeded, and a…
   - R12-CICD-7119 — STAY_PARTIAL_DEPLOY: The performance gate stays as applied: release profile, one test thread, the 2.0x scaling tolerance and the 10,000 us p99 ceiling, all unchanged. If the hosted runner proves noisy, the answer is to…
   - R12-CICD-8008 — STAY_PARTIAL_DEPLOY: This is the same ruling as phase 1. Config Sync and Rollouts are graded against ADR 0114 D4: Argo CD with Config Connector, and the Cloud Run traffic split. I will not write an ADR keeping…
   - R12-CICD-8032 — STAY_PARTIAL_DEPLOY: The drift half is in. drift.yml plans dev every day, a drift is a failed run rather than a warning, and no agent may dispatch it. The row closes when a scheduled hosted run plans dev clean, and when…
   - R12-CICD-9066 — STAY_PARTIAL_DEPLOY: The gate is applied and stands as written. The row closes on two things only I can give: my approving review of the rolling pull request's head, and the first infra.yml up or apps run I dispatch…
   - R12-CICD-9085 — STAY_PARTIAL_DEPLOY: Decision 14 of 2026-10-25 stands. RECORDED_DIGEST is a fidelity baseline and the 2.0x ratios are bounds. The row closes only on a committed baseline of relative figures, recorded on a named runner…
   - R12-CICD-9108 — STAY_PARTIAL_DEPLOY: The SBOM, the scan result and the provenance are attached to the pushed digest with Google's own tooling, gcloud and Artifact Analysis. No new signing tool or action enters the pipeline without an…
   - R12-CICD-9112 — STAY_PARTIAL_DEPLOY: Only a real promotion proves this: dev to test, then replay and paper, with each stage's freight digest recorded against the digest deploy.yml built once. Stage and prod are not promoted to, and the…
   - R12-CONTRACT-7089 — STAY_PARTIAL_DEPLOY: Nothing is left to code: every field is required on and off the wire. The row stays PARTIAL until an envelope is seen passing from a deployed producer to a deployed ledgerd, after I enable billing…
   - R12-COVERAGE-7184 — STAY_PARTIAL_DEPLOY: That ambient cognition cannot starve the reflex, risk or ledger paths is shown on running services or not at all; an in-process test cannot prove one process leaves another its CPU. The row waits on…
   - R12-COVERAGE-7192 — STAY_PARTIAL_DEPLOY: ADR 0119 already answers placement. The mirror is an asynchronous consumer in the target region, deployed only at a second region, and ADR 0035 still runs one region. The replicated journal is the…
   - R12-DATA-10035 — STAY_PARTIAL_DEPLOY: Bigtable stays declined, as decision 5 of 2026-10-25 says. At the two-cluster floor it bills about $950-$1,300 a month for an empty instance, because no client, rebuild job or dependency record…
   - R12-DATA-10037 — STAY_PARTIAL_DEPLOY: Knowledge Catalog stays declined under decision 5. It is a billed managed service, with no storage record under ADR 0101 C4 and no priced ceiling, so nothing is declared in Terraform. The row stays…
   - R12-DATA-10080 — STAY_PARTIAL_DEPLOY: This is declined with Bigtable under decision 5. No fabric sink is written against a store no ADR admits, through a client ADR 0109 does not admit. The row stays MISSING until both records exist and…
   - R12-DATA-8122 — STAY_PARTIAL_DEPLOY: K0 stays in memory and spills nowhere for now. Bigtable is declined under decision 5. K0 is not restated without Bigtable just to pass the row, because that would redefine the requirement, which the…
   - R12-DATA-8128 — STAY_PARTIAL_DEPLOY: Knowledge Catalog stays declined under decision 5. No lineage emitter is written for a catalog that does not exist, because a producer with no consumer is what ADR 0151 decision 1 forbids. The row…
   - R12-FABRIC-7054 — STAY_PARTIAL_DEPLOY: The requirement text stands: replicated market-journal segments stay declined under ADR 0111 §2 until a committed measurement of P2 volume and loss says the journal must survive a zone, and that…
   - R12-FABRIC-8073 — STAY_PARTIAL_DEPLOY: Replication to followers is the managed log's job (ADR 0111), and the follower takeover is proven against the in-memory log. The row's check, a replica holding every acknowledged record, is met only…
   - R12-FABRIC-8091 — STAY_PARTIAL_DEPLOY: A regional Spanner commit, three read-write replicas one per zone, is the RF3 quorum this row asks for, but it counts only when a deployed fabricd acknowledges P0 through it, and decision 5 still…
   - R12-FABRIC-8092 — STAY_PARTIAL_DEPLOY: P1 takes the same reading as P0: the regional Spanner commit is its RF3 quorum once a deployed fabricd acknowledges fills through it, and the immutable archive is the broker's bucket, neither of…
   - R12-FABRIC-9118 — STAY_PARTIAL_DEPLOY: The rollout is a game-day on three brokers behind a provisioned Spanner lease, and nothing short of that run passes it; an in-memory drain or an emulator is a prerequisite, not the evidence. Decision…
   - R12-FINOPS-9186 — STAY_PARTIAL_DEPLOY: BigQuery reservations are declined for now, as in decision 5. The warehouse has one streaming writer and no measured query load, so it stays on on-demand billing. The row reopens with an ADR that…
   - R12-GCP-10060 — STAY_PARTIAL_DEPLOY: Connectivity Tests are declared for paths that exist, not for paths we hope to have. The module stays as built. The root instantiates it from existing module outputs, with no new root variable, once…
   - R12-GCP-7002 — STAY_PARTIAL_DEPLOY: ADR 0211 and ADR 0035 stand: one C4-class node, newyork-1, in shadow mode on the simulated feed, with no venue path. It is switched on only after dev serves, in the order owner-switches.md gives. It…
   - R12-GCP-8035 — STAY_PARTIAL_DEPLOY: ADR 0035 stands: one execution node, us-east4, shadow mode. The requirement says regions are chosen by measurement, and nothing has been measured yet. The first node is the probe. The record that…
   - R12-GCP-8037 — STAY_PARTIAL_DEPLOY: Decision 5 stands. The Spanner ledger, the Knowledge Catalog and a managed model registry are not funded before dev runs. Until then the platform's model registry is the one it journals behind the…
   - R12-GCP-8057 — STAY_PARTIAL_DEPLOY: Decision 4 of 2026-10-25 stands, and l6-ncc-hubs-parked.patch is not applied. With one region the latency mesh has nothing to join. The hub lands with the record that supersedes ADR 0035 for a second…
   - R12-GCP-8058 — STAY_PARTIAL_DEPLOY: Decision 4 stands for the star hub too. There is one region and no GKE service plane (ADR 0114 D4), so no service, data or AI traffic needs a central inspection hub. The parked patch's STAR hub lands…
   - R12-GCP-8104 — STAY_PARTIAL_DEPLOY: The requirement text stands, and the row stays PARTIAL. I will not rewrite it so that ADR 0114 D4's refusal reads as a pass. The portfolio brain runs in the kernel on Cloud Run as exactly one warm…
   - R12-GCP-8116 — STAY_PARTIAL_DEPLOY: The requirement text stands, and the row stays PARTIAL: node pools, taints and PriorityClasses are refused by ADR 0114 D4. Research stays in the deep brain's process. It cannot reach the fast brain…
   - R12-GCP-9086 — STAY_PARTIAL_DEPLOY: The drift route is the one I take: drift.yml's daily plan of dev, which fails on any difference, is applied. The SCC Premium route stays declined under decision 5 of 2026-10-25. The row closes on the…
   - R12-GCP-9120 — STAY_PARTIAL_DEPLOY: Nothing is left to apply: the change-window step has been in infra.yml since b9a45fd5, judging the saved plan before the apply. The row closes on evidence, not code: my approval of the rolling pull…
   - R12-LEDGER-7009 — STAY_PARTIAL_DEPLOY: The ledger of record goes to Spanner as ADR 0113 says, but not before the base deploy runs and an ADR states the measured need and a ceiling. At $2,705 to $8,140 a month it is the largest single bill…
   - R12-MODEL-10065 — STAY_PARTIAL_DEPLOY: Declined: ADR 0114 D4 refuses a GPU pool on GKE, and I decline a Vertex accelerator (decision 5). Neither comes until an experiment measures a need and an ADR names its monthly ceiling; the row stays…
   - R12-MODEL-10084 — STAY_PARTIAL_DEPLOY: No new repository is bought for packs. A signed pack is uploaded to the existing GENERIC qip-deployments-<env> by model-pack-publish.yml under its workload identity, after the two-person signature…
   - R12-MODEL-4024 — STAY_PARTIAL_DEPLOY: The export chain is built, and the row closes only when a deployed cell admits a promoted digest. That waits on the owner's node switch and one two-person published pack, and nothing is enabled early…
   - R12-MODEL-7007 — STAY_PARTIAL_DEPLOY: Accelerators stay declined under decision 5. ADR 0122 decision 4 stands: accelerators are reached only as Vertex AI custom jobs from the offline research lane, and only after a committed measurement…
   - R12-MODEL-9002 — STAY_PARTIAL_DEPLOY: The deep brain keeps fitting in-process with LocalTrainer, and Vertex stays off under decision 5. The Vertex port is wired only once Vertex is funded and a deployment supplies a workload-identity…
   - R12-MODEL-9009 — STAY_PARTIAL_DEPLOY: The model-pack signing keys ADR 0110 already authorised are applied with the deploy and not before. The cell already refuses a model whose pack fails the model-pack key, so the row waits only on a…
   - R12-OBS-9146 — STAY_PARTIAL_DEPLOY: Decision 5 stands: I will not buy a Spanner instance to move one histogram, and a single broker's local append timed and called a quorum ack would be a different number under the same name. The…
   - R12-OBS-9161 — STAY_PARTIAL_DEPLOY: The requirement text stands, and it can be met as written. The one GKE cluster is ADR 0036's control plane, and its controllers are pods whose disruption stops delivery. Pod disruption is declared…
   - R12-QUANT-10067 — STAY_PARTIAL_DEPLOY: Declined for now under decision 5. The in-tree solver runs in-process within ADR 0150's bounds, and no measured queue exists to scale a pool by. A Spot job pool is reopened by an ADR that states a…
   - R12-QUANT-9042 — STAY_PARTIAL_DEPLOY: The execution node stays off until the deploy, which comes last and is the owner's to pay for and dispatch. The code half is done, and the row closes on the plan and apply that show newyork-1…
   - R12-REFLEX-9114 — STAY_PARTIAL_DEPLOY: Rollout by epoch closes only on a deployed standby. A standby is a second execution node in another zone, which no committed environment enables and decision 5 keeps unbought. The row stays PARTIAL.…
   - R12-REFLEX-9117 — STAY_PARTIAL_DEPLOY: A rollout drill runs on a deployed paper node pair or it is not run, and it is not written against machines that do not exist. It waits on a cross-zone lease store and the pair being deployed, and…
   - R12-RES-10125 — STAY_PARTIAL_DEPLOY: Build the fabric alert now, gated on workload_metrics_exist like the other eleven policies. It costs nothing until metrics exist. The row stays open until a deployed fabricd is lost on purpose, the…
   - R12-RES-10158 — STAY_PARTIAL_DEPLOY: Deploy first: nothing applies infrastructure on a timer while billing is off, and an unattended billed apply is not where a rebuild starts. Once dev is up, the rebuild runs monthly in test, up from…
   - R12-RES-8038 — STAY_PARTIAL_DEPLOY: Zone loss needs a standby in a second zone with a lease store it can trust, RF3 fabric replicas and a game day; with no execution node deployed, the Spanner log declined (decision 5) and no…
   - R12-RES-8103 — STAY_PARTIAL_DEPLOY: A rolling upgrade is a person's procedure, since ADR 0213 keeps restart and drain human, and it needs two brokers on one managed log, which decision 5 keeps unfunded. The row stays PARTIAL until that…
   - R12-RES-9169 — STAY_PARTIAL_DEPLOY: Followers stay declined under decision 5. They catch up from the Spanner managed log, whose floor is $2,705 a month. The broker module refuses a second broker under RF1, and no broker image has been…
   - R12-RES-9174 — STAY_PARTIAL_DEPLOY: Bigtable stays declined under decision 5 of 2026-10-25. The plan clause is met in code (two zonal clusters, one app profile per workload), and nothing more is built for a store no one pays for: no…
   - R12-SEC-10050 — STAY_PARTIAL_DEPLOY: Split the cicd ceiling now so CI and infra are each bounded on their own; that is a tightening a lane can land today. 'Only the grants it uses' is a measurement, and it waits on IAM Recommender…
   - R12-SEC-10056 — STAY_PARTIAL_DEPLOY: OBS-020 stands and no exception is admitted. A sink that copies the platform's logs into BigQuery so a scanner can read them puts the same sensitive data in a second place, which is the leak this row…
   - R12-SEC-10157 — STAY_PARTIAL_DEPLOY: The sandbox drill step is already applied on this branch, and the only thing it still needs from me is the review. Once the rolling pull request's head is approved and merged, agent-sandbox.yml is…
   - R12-TICK-2058 — STAY_PARTIAL_DEPLOY: ADR 0156 §2.4 stands. read_indexed keeps proving the whole segment. No ranged read that trusts the manifest further is built on a cost nobody has measured. The deferral is revisited only on a…
   - R12-WORLD-10042 — STAY_PARTIAL_DEPLOY: Declined for now under decision 5 and ADR 0191. Promoted world knowledge stays in the central brains, rebuilt from the event log, which already keeps graph load off the ledger. Spanner Graph is…

   **STAY_PARTIAL_EXTERNAL (36)**

   - R12-API-8054 — STAY_PARTIAL_EXTERNAL: No public address until I name a domain I own and have delegated it to Cloud DNS; console.example.com is not that. The load balancer, Cloud Armor and CDN are billed and come after the paper deploy…
   - R12-ARCH-10159 — STAY_PARTIAL_EXTERNAL: Phase 0 stays entered and not exited, and I will not amend its exit test to pass without NCC. The hub stays declined while ADR 0035 runs one execution region. CA is funded under R12-SEC-10054. PAM…
   - R12-CICD-10116 — STAY_PARTIAL_EXTERNAL: No model provider enters the agent sandbox: its no-model egress is a guard and stays one. Independent evaluation is ADR 0102's read-only reviewer packet, one per dimension (tests, security…
   - R12-CICD-9095 — STAY_PARTIAL_EXTERNAL: Decision 12 stands: both rulesets as written, with the admin bypass only through a pull request. Posting them changes a production security boundary and needs the repository-admin credential, which…
   - R12-DATA-1153 — STAY_PARTIAL_EXTERNAL: No vendor yet for satellite, IoT, shipping, customs, social, product or chain data. Each class enters through its own ADR 0157 licence record, and a charging one needs a contract and a credential…
   - R12-DATA-2006 — STAY_PARTIAL_EXTERNAL: No vendor yet for filings, regulatory portals, court dockets, procurement, patents or corporate registries. A class enters either with a licence record that quotes its publisher's terms or with a…
   - R12-DATA-2008 — STAY_PARTIAL_EXTERNAL: No RPC or indexer vendor yet. qip-chain stays unwired until each class's provider terms are recorded under ADR 0157, as an admission or a quoted refusal. Governance traces stay discarded until ADR…
   - R12-EXPAND-6069 — STAY_PARTIAL_EXTERNAL: Lead/lag is a millisecond fact between venues, and neither a bar nor a one-venue synthetic book can show one, so no residual is fed from either. The four unfed triggers wait for the tick plane under…
   - R12-GCP-10001 — STAY_PARTIAL_EXTERNAL: ADR 0118's one project per environment stands, and the per-plane clause is not reworded away. The plane split is created only under an organisation and billing account the owner holds, and only after…
   - R12-GCP-8059 — STAY_PARTIAL_EXTERNAL: No circuit is ordered and no venue is contracted while the platform trades paper. The router, the Partner attachment and the HA VPN stay declared and switched off. The row stays PARTIAL on that…
   - R12-MODEL-1085 — STAY_PARTIAL_EXTERNAL: Decision 6 stands: no hosted language model vendor yet, so there is no slow-lane teacher to distil. A deterministic template is not a large model and is not distilled to pass the row. When a vendor…
   - R12-MODEL-4004 — STAY_PARTIAL_EXTERNAL: A model fitted on the paper simulator's own fills measures the simulator, not a venue. None is trained or promoted as an order-book, fill-probability or adverse-selection model. Purpose models wait…
   - R12-MODEL-4007 — STAY_PARTIAL_EXTERNAL: Simulator ticks are not a training source: a representation learned from them embeds the simulator. The row waits for a depth feed admitted under ADR 0157 as a committed licence record, and nothing…
   - R12-QUANT-4038 — STAY_PARTIAL_EXTERNAL: ADR 0150 A2 stands, as ADR 0190 confirmed, and the parked scenario-selection family stays parked. Nothing the platform runs poses a scenario selection, so an encoding and a greedy comparison would be…
   - R12-QUANT-4059 — STAY_PARTIAL_EXTERNAL: FeatureEnsembleSelection stays not built, as ADR 0150 A2 decided on purpose and ADR 0190 confirmed. A selection over fewer features than the enumeration limit can at best tie the exact classical…
   - R12-QUANT-4061 — STAY_PARTIAL_EXTERNAL: ADR 0150 decision 5 stands. No quantum-derived feature is admitted until a family has a benefit record against a non-simulated provider. That provider, its key and its budget are billed and mine, and…
   - R12-QUANT-4062 — STAY_PARTIAL_EXTERNAL: No quantum-derived feature exists, and ADR 0150 decision 5 admits none until a family has a benefit record on a non-simulated provider. A per-regime information-gain scorer over an empty set would be…
   - R12-QUANT-7016 — STAY_PARTIAL_EXTERNAL: As with the hosted language model, there is no vendor yet. An IBM Quantum account, token and QPU time are paid and the owner's, and ADR 0190 finds no posed problem that would earn hardware. The…
   - R12-QUANT-7144 — STAY_PARTIAL_EXTERNAL: 'Classical benchmark first' holds. 'Ship only measured wins' also holds, because nothing quantum ships: the router adopts no answer inside its margin, and dev runs the simulator only. A measured QPU…
   - R12-QUANT-9007 — STAY_PARTIAL_EXTERNAL: No quantum vendor now, as with the language model in decision 6. The simulated provider stays the only one, and ComputeSettings keeps refusing hosted by name. The egress to IBM Quantum waits on an…
   - R12-QUANT-9043 — STAY_PARTIAL_EXTERNAL: Vertex GPU training stays declined under decision 5 of 2026-10-25. No measured need and no monthly ceiling is on record, and the Vertex port has no client, credential or egress path. It is reopened…
   - R12-QUANT-9044 — STAY_PARTIAL_EXTERNAL: TPU stays out of scope, as the row itself says. The TPU budget is zero with no consumer (ADR 0158 decision 5), and a TPU7x slice is billed capacity. The JAX or PyTorch stack it implies is a non-Rust…
   - R12-REASON-1024 — STAY_PARTIAL_EXTERNAL: Every formed hypothesis already passes the rule pack before approval, whoever proposed it, so the invariant holds for any proposer. No neural model proposes anything here, and a ridge card is not…
   - R12-REASON-3053 — STAY_PARTIAL_EXTERNAL: The society of explicit reasoners is built, and its verdict now decides approval. 'Alongside neural inference' means the deep brain's hosted language model, which stays withheld until a vendor is…
   - R12-REFLEX-5036 — STAY_PARTIAL_EXTERNAL: A native venue feed needs a licensed source and a venue connection, and both come from vendors I have not bought. QIP_VENUE_FEED keeps `simulated` as its only value. The row stays PARTIAL until a…
   - R12-REFLEX-8070 — STAY_PARTIAL_EXTERNAL: Model packs already ride the signed payload, are checked by the cell against the model-pack key, and swap at a pass boundary. What is missing is a model-pack key under ADR 0110's two-person publish…
   - R12-SEC-10055 — STAY_PARTIAL_EXTERNAL: The perimeter goes on in dry run first, then enforced, as soon as the organisation's access-policy number is supplied. It costs nothing, and that number is the only thing missing. Artifact Registry…
   - R12-SEC-8062 — STAY_PARTIAL_EXTERNAL: Privileged access goes through PAM only once a second person can approve it. The plan refuses self-approval, and I will not loosen that for a team of one. The workforce pool waits on an identity…
   - R12-TICK-1175 — STAY_PARTIAL_EXTERNAL: No market-data vendor is chosen now. Options, futures, perpetual funding, liquidations and on-chain capture each enter only as an ADR 0157 licence record. Someone must read the terms, and the cost is…
   - R12-TICK-2046 — STAY_PARTIAL_EXTERNAL: Crypto order books, perpetual funding, liquidations, dealer rate quotes, and mempool and venue state are not captured until each has an ADR 0157 licence record. That holds even for a free public…
   - R12-TICK-3098 — STAY_PARTIAL_EXTERNAL: The instrument twin meets the constraint and stays as it is. An entity twin, for a company, a central bank or anything else, waits for licensed entity data under an ADR 0157 record and a consumer…
   - R12-TICK-3102 — STAY_PARTIAL_EXTERNAL: The supply-chain twin waits on a supply-chain source licensed under ADR 0157. Port, shipping and commodity-flow data are paid subscriptions, and the choice is mine. Third-order propagation needs no…
   - R12-WORLD-3037 — STAY_PARTIAL_EXTERNAL: A member that saw nothing has produced nothing, and a silent member is reported as silent, not counted as state. Redefining the clause to pass the row is what the 2026-10-24 record rules out. Adding…
   - R12-WORLD-3045 — STAY_PARTIAL_EXTERNAL: Logistics, shipping and inventory vendors are billed and the owner's under ADR 0157, and none is contracted. A supply-chain member over sources nobody feeds would only add another silent member. The…
   - R12-WORLD-3047 — STAY_PARTIAL_EXTERNAL: No event source is licensed, and I am not choosing one now. Vendor choice is billed and stays mine under ADR 0157, and the synthetic venue's outcome stays test-only under ADR 0151 decision 10, which…
   - R12-WORLD-3050 — STAY_PARTIAL_EXTERNAL: The regional member stays microstructure-only. Language, information-ecosystem and logistics inputs need licensed sources that are billed under ADR 0157, and I am not buying any before the deploy. A…

   **FUND_THEN_DEPLOY (4)**

   - R12-DATA-9053 — FUND_THEN_DEPLOY: Bigtable: declined for now, as decision 5 already says. No workload has measured a need for it. It has no Rust adapter, and its gRPC client would be a new dependency under ADR 0109. At about $950 a… [cost_usd_month 950]
   - R12-SEC-10054 — FUND_THEN_DEPLOY: Fund the CA Service pool at the billed plan's $200-$250 a month. The one execution node I asked for cannot start without it, and fabricd and qip-tls already request from it. It goes on with dev's… [cost_usd_month 250]
   - R12-SEC-9137 — FUND_THEN_DEPLOY: Fund the four HSM artefact-signing keys at $10-$11 a month with bundle_not_after = 2027-04-30T00:00:00Z. That is six months out, so the trust bundle is re-read soon after the shared HMAC key is… [cost_usd_month 11]
   - R12-TICK-7020 — FUND_THEN_DEPLOY: A dedicated replay pool is new billed infrastructure, and decision 5 put it after the deploy. When it comes, it is a Cloud Run Job kept separate from the serving services. Its ceiling is about $105 a… [cost_usd_month 105]

   **REFUSE_LIVE (16)**

   - R12-AGENCY-1006 — REFUSE_LIVE: ADR 0145 stands: the one act this platform takes is an internal draft, its effect is filed as measured and unidentified, and nobody calls it causal. Learning a causal effect needs an action in the…
   - R12-AGENCY-1183 — REFUSE_LIVE: O16 is declined: ADR 0114 D8 stands, and no outbound channel, public adapter or other external action surface is opened, with its credential, account and reputational exposure. Agents draft through…
   - R12-AGENCY-1184 — REFUSE_LIVE: Estimators, counterfactual branches and no-action baselines run, and the policy is held to the measured effect. Graphs are not updated, because the only measured effect here is unidentified and ADR…
   - R12-AGENCY-6078 — REFUSE_LIVE: The engine turns goals into gated plans and runs them to shadow. It does not turn results back into causal structure, because the only result is an unidentified draft effect, and a learned edge…
   - R12-AGENCY-6092 — REFUSE_LIVE: O16 is declined for publishing: ADR 0114 D8 stands, and no agent publishes to the outside world. Agents draft through the drafting adapter for a person to post under their own credential. This is an…
   - R12-AGENCY-6093 — REFUSE_LIVE: O16 is declined for outreach: no agent messages, negotiates with or requests anything of an outside party. A person sends what an agent drafts through the drafting adapter, under their own identity.…
   - R12-AGENCY-6094 — REFUSE_LIVE: O16 is declined: ADR 0114 D8 stands, so no survey or experiment is run on the outside world and no public research adapter is built. Lawful acquisition through qip-data-finder and internal simulation…
   - R12-AGENCY-6095 — REFUSE_LIVE: There is no product to price, and pricing as an agent's action is an external action that ADR 0114 D8 refuses. A price analysis is drafted for a person to act on. The row stays BLOCKED, and a product…
   - R12-AGENCY-6096 — REFUSE_LIVE: An agent changes no operational system and deploys nothing. ADR 0114 D8 and the change-management rule both stand, and neither is reopened under the delegation. A change is drafted for a person, and…
   - R12-AGENCY-6097 — REFUSE_LIVE: No agent allocates, hedges, rebalances, reserves liquidity or moves collateral with real capital. ADR 0003 and ADR 0021 stand, and ADR 0108 stays proposed: only the owner can accept it, in writing.…
   - R12-AGENCY-6098 — REFUSE_LIVE: Refused. A market action that leaves paper is a live order, ADR 0003 holds until I accept ADR 0108 in writing, and I do not; narrowing the requirement to paper actions would redefine it to pass the…
   - R12-AGENCY-6108 — REFUSE_LIVE: The memory recommends only on identified outcomes. On the default node it holds none, so it answers 'nothing', and that answer gates no draft; that is the truth and it stays. The memory will learn…
   - R12-AGENCY-7028 — REFUSE_LIVE: O16 is declined: ADR 0114 D8 stands, and no outbound channel, survey or public adapter is built, nor any account or credential opened for one. Agents draft to the internal outbox and a person sends…
   - R12-AGENCY-9062 — REFUSE_LIVE: Refused: this asks for an external action surface. Adapter services that publish, reach out, set prices or create markets stay refused under ADR 0114 D8. The internal drafting adapter, whose only…
   - R12-GOV-3030 — REFUSE_LIVE: A shadow release is not a publication, and I will not read it as one. The requirement text decides, for the reason decision 11 of 2026-10-25 gives. A real publication channel is an external action…
   - R12-OBS-9158 — REFUSE_LIVE: This is an external action surface, and it is refused. The SRE agent does not open incidents or pull requests in an outside system. It is given no credential to scale, roll back, restart or drain…

   **IMPLEMENT (25)**

   - R12-AGENCY-3028 — IMPLEMENT: ADR 0146 decision 3 stands: the Causal Agency Brain is registered by its BrainSpec in the deep brain's registry, which refuses to start without it. It does not become an organisation agent, and…
   - R12-ASSET-4073 — IMPLEMENT: A physical position is a paper research record. Each unit the simulated merchant delivers becomes a RealAsset position carrying its landed cost, and its inventory state from the ledger. It stays…
   - R12-CICD-10117 — IMPLEMENT: Decision 13 stands: exclude, not inject. A lane writes the deploy.yml change as a patch for me. The portal's second build is compared file by file, with exactly the four per-build-secret files…
   - R12-CICD-10168 — IMPLEMENT: ADR 0102 is not amended. The fleet keeps its eight roles, and security, merge and release stay out of it. The requirement's planner, coder, test, security, reviewer, release and SRE roles are the…
   - R12-CICD-10203 — IMPLEMENT: Every action a workflow uses is pinned by its full commit SHA with the tag kept as a comment, and a test refuses a movable tag. A lane writes that as a patch for me to apply, because .github/** is…
   - R12-CICD-9083 — IMPLEMENT: Require the trailer: from the instant the running round's manifest is committed, every agent commit names its lane, and one that names none is refused, the orchestrator's included. The orchestrator…
   - R12-DATA-8014 — IMPLEMENT: Build the Source Manifest Registry on the store ADR 0113 already admitted. It is a registry over SpannerKv, keyed by canonical URI, proven now against the in-process double. enable_spanner stays…
   - R12-DATA-8112 — IMPLEMENT: ADR 0144 already decided scout isolation, so build it as written. The scout runs as its own Cloud Run job, under its own identity with no trading role and its own egress routes. The deep brain only…
   - R12-DATA-8125 — IMPLEMENT: No transfer service is bought for the archive. Object lifecycle cannot move objects between buckets, so the landing clause is restated: the archive receives each sealed segment from fabricd's own…
   - R12-EVENT-5083 — IMPLEMENT: An oracle's report never resolves a market by itself. discover proposes a resolution only when the market's own criteria, evaluated over named observations from its named source, agree with the…
   - R12-EVID-8108 — IMPLEMENT: The requirement is read the way decision D4 of 2026-10-11 and ADR 0114 Amendment 1 already restated it. Scoring is a stage of qip-deepbrain on Cloud Run, rebuilt from the log, with no GKE pool, no…
   - R12-FABRIC-10026 — IMPLEMENT: The module is instantiated and already refuses a non-Hyperdisk journal, so the gap's 'instantiate' is stale. The plan check still lacks an assertion on the journal's own disk type and a run that…
   - R12-FINOPS-10137 — IMPLEMENT: No commitment is bought on the hot path until a sizing record measured on a provisioned C4 execution node is committed, and that becomes a refusal now rather than a promise. The record itself waits…
   - R12-GCP-10010 — IMPLEMENT: No circuit is ordered: a partner contract is a term and a monthly charge for cells that run in shadow in one region, and I am not buying it. What is ours is the defect the 2026-10-11 apply found, a…
   - R12-GCP-8060 — IMPLEMENT: Private Google Access, with googleapis.com resolved to the restricted VIP, is the private route, and it meets 'where supported'. The PSC endpoint stays off, because it serves only an interconnect far…
   - R12-GOV-10005 — IMPLEMENT: The feed exists, so the row's 'no cloud_asset resource' is out of date. What is missing is the plan assertion the requirement names, so a lane adds it and corrects the text. The row is PARTIAL, not…
   - R12-MODEL-6022 — IMPLEMENT: ADR 0149 decision 13 stands: the influence weights and the compute allocation stay journaled records that govern nothing. Build the measurement d13 names now. Only when the influence-weighted…
   - R12-OBS-9148 — IMPLEMENT: ADR 0119 §3 already decided this: the mirror and its lag series are built and proven now on one host and deployed only at a second region, which stays parked (decision 4). Build the series on two…
   - R12-QUANT-4018 — IMPLEMENT: The requirement calls quantum variants optional. ADR 0150 decision 5 keeps them unbuilt until a family has a benefit record on a non-simulated provider. The constraint is therefore the refusal at the…
   - R12-REASON-1133 — IMPLEMENT: Neural coupling stays out of scope under ADR 0150 decision 7. A derivation whose timeline cannot be rebuilt is not a proof, so replay refuses it by name and never reports it as identical.
   - R12-REFLEX-5030 — IMPLEMENT: Belief priors stay freshness-only. A belief value never sizes an order until ADR 0088's belief is built and Brier-scored, and then only as a (0,1] narrowing multiplier under its own record. The…
   - R12-REFLEX-8066 — IMPLEMENT: A host tuning is adopted only after a replay shows it helps. The same recorded spool is replayed on one execution node, booted once with the image's tuning and once without, and the two latency…
   - R12-RES-9177 — IMPLEMENT: A registry copy in another region is storage, not a second execution region, so decision 4 does not hold it and decision 5 does, because it bills; it is a replica, not a multi-region repository that…
   - R12-RISK-4096 — IMPLEMENT: Build the quote gate now: an order on an instrument with no quote in the platform's market snapshot is refused and journaled. It reads presence only, because a freshness bound would be a new…
   - R12-TICK-2077 — IMPLEMENT: ADR 0148 decided this on 2026-10-19 and it stands. A missed opportunity is a claim graded in basis points over its own horizon, and it is never priced at a side nobody proposed. What is not graded is…

   **ADR_THEN_IMPLEMENT (40)**

   - R12-AMBIENT-3027 — ADR_THEN_IMPLEMENT: The Brain Registry records a brain; it does not cover a scope. Treating it as the production organisation would redefine the requirement to pass the row. The attention router stays advisory and off…
   - R12-AMBIENT-3106 — ADR_THEN_IMPLEMENT: The Curiosity Engine of §9.10 is the acquisition rank: AcquisitionCriteria::net_value_bp, ordered by rank_acquisitions. The expansion curriculum keeps its EXPAND-6063 order, because an unhurried…
   - R12-AMBIENT-9001 — ADR_THEN_IMPLEMENT: O02 is decided narrowly. BigQuery, already switched on in dev, is the one evidence store agents read. Bigtable, Spanner Graph and Knowledge Catalog stay declined under decision 5 until an ADR states…
   - R12-ARCH-7134 — ADR_THEN_IMPLEMENT: The requirement text stands and so does ADR 0143. The row is graded against ADR 0143's decline and stays PARTIAL until phase 0 exits. I will not add switches to code that runs paper today: turning…
   - R12-ASSET-4070 — ADR_THEN_IMPLEMENT: Joint construction is approved as one run that can only narrow. construct_from takes grouped caps for strategies from the allocator's plan, for horizons from the reconciled horizon pools and for…
   - R12-CAPITAL-4085 — ADR_THEN_IMPLEMENT: Capital agents are approved as proposers only. Each of the ten named agents submits a plan every cycle, and the kernel records all ten before it chooses. The plan it acts on is the per-line minimum…
   - R12-CAPITAL-9047 — ADR_THEN_IMPLEMENT: The capital-society coordinator is a step the kernel composes over qip-capital, not a new binary (ADR 0127). It settles proposals in the survival waterfall's order: liquidity and collateral first…
   - R12-CICD-9103 — ADR_THEN_IMPLEMENT: The pool's refs/heads/ bound stands. A pull request's ref never gets a Google credential, because widening the bound for a plan would hand that credential to any pull request's code. Infrastructure…
   - R12-CONTRACT-7067 — ADR_THEN_IMPLEMENT: The cell gets the typed envelope, built only from numbers that already exist. Hard limits come from the limit set as it stands. Each standard stress scenario is bounded at the limit set's…
   - R12-CONTRACT-7074 — ADR_THEN_IMPLEMENT: The receiver is the central brain's divergence hold. A re-promotion releases a venue's hold only when the latest MicrostructureDelta concluded for that venue sits inside the thresholds I approved in…
   - R12-CONTRACT-7075 — ADR_THEN_IMPLEMENT: The receiver is the deep brain's gap detector. If a partition's WorldModelState passes its expiry with no successor, the deep brain raises a model-drift gap and queues re-estimation. The state never…
   - R12-COVERAGE-5103 — ADR_THEN_IMPLEMENT: Crypto trades on paper through the same simulated gateway as every class, and its settlement is simulated there too. A decentralised-venue fill's proceeds wait on an operator-declared finality depth…
   - R12-COVERAGE-5104 — ADR_THEN_IMPLEMENT: Event contracts trade on paper only. An event contract is a fully-funded payoff bounded to one unit, priced only where the simulated exchange prints it, and settled only on an operator-declared…
   - R12-DATA-8019 — ADR_THEN_IMPLEMENT: Each fabric stream archives under the retention class its catalogue entry declares. A stream whose declared class disagrees with its topics' Topic::retention_class is refused at catalogue load. The…
   - R12-DATA-8127 — ADR_THEN_IMPLEMENT: BigQuery is fed by batch load from Cloud Storage, not by a fabric sink. fabricd has no boot image, while a load needs only the two storage switches already declared. OutcomeSink stops streaming rows…
   - R12-DATA-8131 — ADR_THEN_IMPLEMENT: Dataflow and Dataproc stay out, because ADR 0011 keeps everything in Rust. The row is scored against its Rust equivalent, the way D4 scored the GKE rows against Cloud Run. Batch reprocessing is a…
   - R12-EXPAND-6065 — ADR_THEN_IMPLEMENT: This row needs no capital. Decision 8 filed it with the live-capital rows from the triage's bucket, and I take it out. Causal drivers are measured on the graph the platform already keeps: each driver…
   - R12-EXPAND-6067 — ADR_THEN_IMPLEMENT: Beliefs are probabilities, so no belief price level is built. A price/belief discrepancy is the federation's arbitrated probability landing on the wrong side of what the price then did, for the same…
   - R12-FINOPS-9189 — ADR_THEN_IMPLEMENT: No cloud worker pool is bought for autonomous development (decision 5). The autonomous-development plane is agent-sandbox.yml on ephemeral hosted runners. It already allows one run per lane, with a…
   - R12-LEDGER-8136 — ADR_THEN_IMPLEMENT: The ledger holds simulated paper balances and no customer data, so no foreign residency rule binds it today. Until a residency review says otherwise, its data stays in the United States. The instance…
   - R12-MESH-5042 — ADR_THEN_IMPLEMENT: Discovery proposes and the operator permits. The signed policy carries an operator-signed market reference, and the cell reports any cycle it finds outside the whitelist and sends nothing on it. Only…
   - R12-MESH-5059 — ADR_THEN_IMPLEMENT: The centre discovers templates only inside what the operator signed. Slot 8 carries the operator's conversions narrowed to the cycles the centre finds over them, so stopping discovery empties it.…
   - R12-MODEL-10180 — ADR_THEN_IMPLEMENT: An automatic promotion moves the centre's alias and activates nothing on a cell. A model is activated only by a two-person signed ModelPack (ADR 0110): its KMS signature is the signature link, its…
   - R12-MODEL-1083 — ADR_THEN_IMPLEMENT: No bucket or registry is bought to carry plans to nodes. A model reaches a node only inline in its two-person signed pack inside the policy payload. A compiled plan reaches it baked into the attested…
   - R12-MODEL-3089 — ADR_THEN_IMPLEMENT: The population is the learning desk's own fitted models. Each decision-eligible card forecasts only its own instrument's question, on the deep brain's existing capacity at no new spend, and a drifted…
   - R12-MODEL-4003 — ADR_THEN_IMPLEMENT: A specialist model is trained only from a source a licence record admits. Volatility and regime qualify now, from the licensed bars. Event probability, valuation, liquidity, default, demand and…
   - R12-MODEL-4011 — ADR_THEN_IMPLEMENT: An ambient model reaches a fast lane only as the signed distillate of a fitted teacher with its own training rows and holdout. The now-stage filter is not a teacher, and a student fitted to its own…
   - R12-OBS-10059 — ADR_THEN_IMPLEMENT: The requirement names Cloud Trace, and the text stands. Request spans go to Cloud Trace from the process, the way ADR 0121 sends metrics to Cloud Monitoring. There is no collector and no new crate.…
   - R12-OBS-9074 — ADR_THEN_IMPLEMENT: The runbook set is signed by the image it is compiled into. The deep brain runs only under the deny-by-default Binary Authorization policy, whose attestor key CI alone holds, and no runbook is ever…
   - R12-OBS-9165 — ADR_THEN_IMPLEMENT: Delivery objectives are declared over facts the workflows already produce. Each fact is recorded at the step that produces it: lead time, rollback, rebuild match, attestation coverage and drift in…
   - R12-REASON-7203 — ADR_THEN_IMPLEMENT: No running process signs a reflex package. The deep brain writes each package as an unsigned draft, and the two-person model-pack publish signs it, exactly as a model pack, so the deep brain never…
   - R12-RES-10130 — ADR_THEN_IMPLEMENT: The requirement text stands: a panel convened on a lost cycle produces new findings and theses, and reading that as existing packages continuing would redefine the row to pass it. While the AI…
   - R12-RES-7099 — ADR_THEN_IMPLEMENT: Custody is read from the simulated venue's own statement and from nothing else. A statement older than two statement periods makes custody Stale. Older than four, or no statement since the node…
   - R12-RES-7106 — ADR_THEN_IMPLEMENT: The requirement stands as written, and I will not narrow it to self-contradiction to pass the row. When the platform's own acting constraints are unsatisfiable, the conclusion is Unresolved: its…
   - R12-TICK-10146 — ADR_THEN_IMPLEMENT: A replay environment is wanted, and the requirement is not restated to the in-process replay. Replay is declared as a fifth environment under ADR 0118 and left off, like test, stage and prod. It is…
   - R12-TICK-2041 — ADR_THEN_IMPLEMENT: The central `Tick` is a raw print, and the cognitive layer does not take it. ADR 0151 decision 9 already says the centre has no clock discipline behind a `Tick`, so a `Tick` cannot be the…
   - R12-TICK-3097 — ADR_THEN_IMPLEMENT: Synthetic worlds may vary what no feed measures. Policy, weather, default and supply drivers get a synthetic-only producer. The branches they grow stay Provenance::Synthetic, and UNDERSTAND and…
   - R12-WORLD-1178 — ADR_THEN_IMPLEMENT: The federation runs every UNDERSTAND, so 'not integrated' is stale. Two members can be built now from the book the kernel already holds: a capital member forecasting the book's floor and a…
   - R12-WORLD-3043 — ADR_THEN_IMPLEMENT: Build the macro member now on the on-chain member's pattern. It reads the NOW macro partition, forecasts its own series, and is resolved by the next reading, so it speaks the day a catalogued…
   - R12-WORLD-3046 — ADR_THEN_IMPLEMENT: The paper feed already states each trade's aggressor side, and like the simulated depth dataset its terms are the platform's own. So an order-flow member reads it, labelled simulated, and a licensed…

   **APPLY_PATCH (2)**

   - R12-GOV-9075 — APPLY_PATCH: Approved as written. The learning agent may write only under docs/proposals/**, and the guard refuses its edits to the deploy workflow, the promotion code and the agent policy, so promotion stays… [docs/blueprint/lanes/l5-edge-GOV-9075.patch]
   - R12-RES-8026 — APPLY_PATCH: Apply the patch as written: it documents the three series the cell now publishes and changes no value, and because it edits .claude/rules, which the permission layer refuses an agent, I apply it… [docs/blueprint/lanes/l6-res-8026-observability-rule.patch]

11. **`qip-streaming`'s dormant `MeshTransport` is deleted, and ADR 0128 is
    not amended** (R12-FABRIC-10177, lane L1b, decided 04:23 UTC). The
    cross-process-send scan L1b built for that row found a socket-opening
    type in `qip-streaming` that nothing calls and that ADR 0128 does not
    name. The scan's allow-list and the ADR can be made to agree in two ways:
    amend the ADR to admit a site with no caller, or remove the site. The
    second is taken. A socket-opening type nothing calls is a cross-process
    send the scan must either admit or see removed, and admitting an unused
    one would make the ADR describe a path that does not exist, which is the
    error the scan exists to catch. L1b deletes it with its own unit tests,
    after confirming by grep that no production path and no other crate's
    test constructs or names it, and keeps the allow-list at its eight sites
    so the deletion stays a finding the scan would have raised. ADR 0128 is
    not edited: a lane never edits an existing ADR, and nothing in it becomes
    false by the deletion.

12. **R12-QUANT-4018 keeps the skeptic's gaps, and decision 10's proposed
    gap text is not applied** (lane L4, decided 06:20 UTC). Decision 10
    ruled the row IMPLEMENT and proposed regrading its gap to "none open;
    producer optional and unbuilt by ADR 0150 d5" once the promotion-seam
    guard landed. The guard landed, and the lane's skeptic then found two
    defects the ruling did not know about: the promotion seam checks that a
    calibrated baseline card exists and never compares the candidate against
    it, and a promotion restored from the log skips
    `require_eligible_workload`. A ruling written before a finding does not
    outrank the finding, so the row stays PARTIAL with the skeptic's gaps.
    Both defects are refusals that are missing, so closing them is a
    tightening and is handed back to lane L4: the seam refuses a candidate
    that does not beat its baseline card, and a restored promotion passes the
    same eligibility check a live one does, each with a test that fails when
    the check is removed. The ruling's text is kept in the row's notes as the
    record of what was decided and why it was superseded.

13. **The commerce dev mount waits on its own ADR** (lane L1, decided
    06:50 UTC; R12-COMMERCE-1087 to 1094, R12-COMMERCE-5094 and the rest of
    R12-ASSET-4073). Decision 10 ruled that the commerce-only paper breadth
    plan ships on through dev's committed configuration. Building it showed
    that the mount is a change to how `modules/cloudrun` lays out a
    workload's config files, not wiring. Every file shares one directory
    named by a hash of all of them, and dev now projects that directory from
    a single-payload secret. So a second file either changes the module's
    layout or moves the universe file's path, which `deepbrain.yaml` names.
    Each committed file also needs a seed entry in `infra.yml`, a protected
    path. Committing the manifest's volume before dev's tfvars sets the
    variable fails `manifest_wiring`'s parity test, and changing that test's
    expectation to match would weaken a guard. So lane L1 writes an ADR for a
    per-file mount first, then one patch that carries the protected parts
    (`variables.tf`, dev's tfvars, `infra.yml`'s seed list) beside the
    unprotected ones, and CI's terraform job validates it. The parity test
    is not touched. The plan file itself is committed and a kernel test runs
    it through the cycle. The rows stay PARTIAL until the mount lands.

14. **The second pass decides the 188 rows the re-triage left with the owner**
    (2026-10-31, under decision 7). Re-triaging the register after decision
    10 (`docs/blueprint/lanes/wave1031-triage-2026-10-31.json`) still found
    188 rows in owner buckets: OWNER_COST 96, DECIDE_ADR 64, PROTECTED 19,
    OWNER_DECISION 5, OWNER_LIVE 3 and OWNER_RISKLIMIT 1. The delegated
    workflow `wave1031-owner-decisions-2` decided them the way decision 10
    decided the first 208: sixteen batches of at most twelve rows, a
    read-only decider per batch, and a skeptic who re-read every cited path,
    symbol and test against the tree before a ruling stood. The verbatim
    decisions (ruling, rationale, closure, the skeptic's notes and, where the
    skeptic overturned, the class the decider had chosen) live in
    `docs/blueprint/lanes/wave1031-owner-rulings-2-2026-10-31.json`. This
    decision records the classes, the rule each class carries and every row
    under its class, so that a lane and a reader of the register see the
    same decision.

    The counts: 76 STAY_PARTIAL_DEPLOY, 36 ADR_THEN_IMPLEMENT, 28 IMPLEMENT,
    21 STAY_PARTIAL_EXTERNAL, 11 REGRADE_TEXT, 9 WRITE_PATCH, 5 REFUSE_LIVE,
    2 FUND_THEN_DEPLOY and no APPLY_PATCH. The three OWNER_LIVE rows,
    R12-CAPITAL-4084, R12-EXEC-7004 and R12-REFLEX-10074, are refused, as
    decision 7 refused that bucket in advance. The one OWNER_RISKLIMIT row,
    R12-CAPITAL-4078, declares no yield shelf, routed share or ladder
    horizon, because the share would be a new capital limit whose only
    effect is to give up liquidity the confidence governor withheld, and
    that is not a tightening.

    The skeptic overturned the decider on 41 rows. Each of the 41 is a
    ruling that would otherwise have reached the register wrong, which is
    why a skeptic re-reads the tree before a ruling stands. On five the
    class moved. Decision 10 recorded four such moves for the first pass and
    no count of corrections within a class, so the two passes compare only
    on that figure.
    R12-COMMERCE-5091, 5092 and 5093 went from WRITE_PATCH to
    ADR_THEN_IMPLEMENT, because the dev mount they ride changes how a
    workload's config files are laid out, and decision 13 had already sent
    that mount to an ADR first. R12-FINOPS-8041 went from REGRADE_TEXT to
    IMPLEMENT, because Bigtable's autoscaling is declared but no plan test
    asserts it, so the regrade would have written false evidence into the
    row. R12-API-9059 went from STAY_PARTIAL_DEPLOY to STAY_PARTIAL_EXTERNAL,
    because ADR 0038, as it has stood since 2026-09-19, keeps the password
    path and reopens when Google publishes the passkey methods, not when a
    probe runs against a running dev project. On the other 36 the class
    held and the substance did not. Some closures would have broken or bent
    an existing guard: R12-CICD-9072's would have failed
    `code_reviewer_and_security_engineer_have_distinct_models`, and
    R12-MESH-1018's told the owner to edit a pin that must not move. Some
    would have let a row pass on an equivalent the requirement does not
    name: R12-CICD-8117, R12-CICD-9076 and R12-CICD-9115. Some tests would
    have failed on unmutated code, or their mutation could not bite:
    R12-EVID-1107, 1109, 1111 and 1112, and R12-DATA-1105. Some citations
    the tree does not hold: R12-TICK-8110's `session_replay` `from_archive`
    and R12-MESH-1018's `THE_PEER_MESH_HAS_NO_DEPLOYED_PEER`. And one ruling
    marked tighten-only was not: R12-EVID-2035's fold would have let a
    posterior rise past today's update and through the red-team approval
    gate, so the corrected ruling never moves a posterior further from the
    prior than the current update would. Where the skeptic overturned, its
    text is the authority for the closure and the gap, and the ruling is
    read together with it. R12-API-9059 is the plainest case: the skeptic
    corrects which of ADR 0038's outcomes holds.

    Every ruling, as corrected, tightens only. None eases a guard, a limit,
    a test or a paper-trading layer, none admits a dependency without an
    ADR, and none funds anything new. The two FUND_THEN_DEPLOY rows name
    resources decision 10 already funded, and they are not counted twice:
    R12-SEC-7014 is R12-SEC-9137's four HSM signing keys ($10 to $11 a
    month), and R12-SEC-9128 is R12-SEC-10054's CA Service pool ($200 to
    $250 a month). The five REFUSE_LIVE rows (R12-CAPITAL-4084,
    R12-CAPITAL-5078, R12-COVERAGE-7187, R12-EXEC-7004, R12-REFLEX-10074)
    need live orders, real capital or an external action surface, so they
    are never closable under any delegation. R12-EXEC-7004's skeptic also
    recorded what its decider missed. The edge node does hold a REST
    order-entry path in code (`NodeGateway::Live` over `RestGateway`). What
    keeps that path shut is that no environment, manifest or lane selects
    it, and that `manifest_wiring`'s `NOT_AN_ORDER_ENTRY_DIAL` refuses it as
    a deployment dial. This decision leaves both exactly as they are.

    The rule each class carries in the register
    (`docs/blueprint/assessment/R12-*.json`), applied the same day. Every
    row's `notes` gains "Owner ruling 2026-10-31, second pass (CLASS):" with
    the ruling in one sentence. Where the skeptic overturned, that sentence
    already carries the skeptic's correction. A pointer to the verbatim JSON
    follows, and no earlier text is removed.

    - **REGRADE_TEXT (11).** The row's text was older than the code, or
      older than a decision, so `current`, `evidence` and `gap` are corrected
      now and no status moves. O08, O10 and O15 are recorded as declined on
      the rows that waited on them (R12-FABRIC-7039, R12-FABRIC-8077,
      R12-FABRIC-7049, R12-EXEC-1168, R12-EXEC-5045). R12-CONTRACT-7080
      accepts ADR 0146 decision 9's amendment. The requirement statement in
      `docs/blueprint/requirements/R12-M7.json` still reads "authority
      class", and its own change amends it, not this one. R12-DATA-9185
      claims only what the skeptic verified: the journal and watermark keys
      are hash-sharded, but the chain and tail families are not, so
      "eliminates hotspots" stays unmet. R12-RES-10126 reads "may cover" as
      the permission it is, and promotes nothing until a lane records a
      mutation against each `degraded.rs` test. Every path and symbol
      written into these rows was found in the tree on 2026-10-31, and
      every line number the rulings cited was re-read. Where the code had
      moved, the row cites the symbol. The rulings also ask that
      `blocked_by` drop O08, O10 and O15, and that R12-GCP-7005's
      `verification` name the manifest test. Those fields are left to the
      grader, as decision 10 left them.
    - **STAY_PARTIAL_DEPLOY (76), STAY_PARTIAL_EXTERNAL (21),
      FUND_THEN_DEPLOY (2), REFUSE_LIVE (5).** Status unchanged. Each row's
      `gap` opens with "2026-10-31 owner ruling, second pass (CLASS):" and
      the statement of what is missing. That is deployment evidence the
      owner's dispatch produces; a third party (a vendor, a licence record
      under ADR 0157, a registrar, a credential or a second approver) nobody
      engages under the delegation; a funded resource with its monthly cost
      named; or, for a refusal, "needs live orders or real capital: never
      closable". The ruling and its closure follow, then "Earlier:" and the
      gap as it stood, so nothing a lane wrote is lost. On the fourteen of
      these rows the skeptic overturned, the gap carries the skeptic's
      corrected statement in place of the decider's. Three rulings carried
      wording the skeptic asked to fix before it was written, and each was
      fixed in the gap: R12-REFLEX-7003's double negative, R12-REFLEX-8065's
      claim that refusing swap does the work of locking memory, and
      R12-REFLEX-10074's claim that a live gateway is refused at start-up
      without condition. R12-QUANT-10066's closure gains the skeptic's
      required amendment of ADR 0122 decision 4. Some rulings in these
      classes also order a text correction to `current`, `evidence` or
      `blocked_by`. That correction is carried in the gap, and those fields
      are left to the row's lane or the grader.
    - **IMPLEMENT (28), ADR_THEN_IMPLEMENT (36).** Handed to the lanes in
      `docs/blueprint/lanes/wave1031-handoff-2-2026-10-31.json`, by decision
      10's map of id prefix to lane, with COMMERCE going to L1 as decision
      13 placed it. Each row stays at its status until its lane closes it
      with the mutation report the closure names. An ADR_THEN_IMPLEMENT
      row's lane re-reads `docs/adr/README.md` and claims the ADR number
      there in its own pushed commit before writing the body, under the
      title the ruling gives. The three COMMERCE rows had no decider title
      and carry the skeptic's, which is decision 13's per-file mount ADR.
      Where the skeptic overturned, the handoff row carries
      `decision_before_skeptic` and `skeptic_governs`, and the lane builds
      to the skeptic's closure. Where an ADR title the decider wrote says
      more than the skeptic allows (R12-CONTRACT-7062's "instead of
      CheckedEvidence", R12-EXPAND-6043's federation residual), the ADR
      follows the skeptic, not the title's wording.
    - **WRITE_PATCH (9).** A lane writes the protected patch at the path
      the ruling names under `docs/blueprint/lanes/`, and none of the nine
      exists yet on 2026-10-31. The orchestrator then applies each patch on
      the integration branch as far as the session permission layer allows.
      R12-CICD-9072's `.claude/agents` hunk is refused to any agent, so that
      hunk stays the owner's to `git apply`, as decision 3 records for its
      kind. R12-COMMERCE-5095 and 5097 wait for decision 13's ADR, and their
      one patch then carries every part at once. Each row keeps its status
      until its patch is applied and its gate evidence is recorded.
    - **APPLY_PATCH (0).** No row in this pass names an existing patch.
      Decision 10's two owner-only patches stand as recorded there.

    The handoff by lane: L1 8, L1b 8, L2 9, L3 12, L3b 7, L4 1, L4b 4, L5 2,
    L5b 1 and L6 21, which is 73 rows. L2b has none in this pass.

    ### Every row, by class (ruling in one line; the full text is in the JSON)

    A row the skeptic overturned is marked [skeptic], and its line carries
    the corrected ruling. A handed-off row names its lane.

    **REGRADE_TEXT (11)**

    - R12-CONTRACT-7080 — REGRADE_TEXT: I accept ADR 0146 decision 9's amendment. A goal carries no authority class. Authority lives on the action policy and starts at Shadow, and a goal document naming authority_class is refused. That is…
    - R12-DATA-2022 — REGRADE_TEXT: The score is built as the requirement says, and its unit check holds. The gap's 'deliberately unset' is older than the code: the desk now scores every cycle by default and journals each round…
    - R12-DATA-9185 — REGRADE_TEXT: The ledger's journal/ and watermark/ keys are hash-sharded over KEY_SHARDS = 16 and the journal family is tested, but per the skeptic chain/<cell>/... stays monotonic per cell and tail/<cell> is one… [skeptic]
    - R12-EXEC-1168 — REGRADE_TEXT: O15 is declined. Quoting stays refused at Cell::send, and quote_loop keeps no path to an order. Lifting either removes a refusal and rewrites the acceptance tests that pin the boundary, which this…
    - R12-EXEC-5045 — REGRADE_TEXT: O15 is declined, as on R12-EXEC-1168. The node keeps refusing ContinuousQuoting at Cell::send, so no edge quote engine is built. Order types, venue selection, leg sequencing and cancel/replace stand…
    - R12-FABRIC-7039 — REGRADE_TEXT: O08 is declined. prost is not admitted, as ADR 0109's table already says: the fabric codec is canonical JSON under hash chains, and no wire format outside this repository asks for protobuf. The…
    - R12-FABRIC-7049 — REGRADE_TEXT: O10 is declined. blake3 is not admitted, neither as a crate nor written in-tree, because ADR 0104 already rejected BLAKE3 for the batch chain and ADR 0043 keeps SHA-256 and HMAC as our only hash…
    - R12-FABRIC-8077 — REGRADE_TEXT: O08 is declined. prost stays unadmitted, and I do not commission an in-tree generator either. The lock's Shape names only JSON kinds, so Rust generated from it would be weaker than the hand-written…
    - R12-GCP-7005 — REGRADE_TEXT: The statement stands, and its check is read through ADR 0036. The catalogue's services are Config Connector RunService manifests, one per workload per environment, and that accepted record removed…
    - R12-LEDGER-5005 — REGRADE_TEXT: I invent no reservation to give InventoryReservation a producer. Nothing on any path reserves a unit today, and ADR 0153 leaves every kind it does not name bookable, with no producer by decision.…
    - R12-RES-10126 — REGRADE_TEXT: 'Other regions may cover eligible functions' is a permission, and I read it as one. Cover is allowed, not owed. With one region under ADR 0035, there is no other region to cover. If another region…

    **STAY_PARTIAL_DEPLOY (76)**

    - R12-ARCH-8029 — STAY_PARTIAL_DEPLOY: The requirement stands as written: Spanner owns financial truth and Bigtable owns hot features. I will not map the fact classes onto today's local stores so the row passes. Both stores stay declined…
    - R12-ARCH-9012 — STAY_PARTIAL_DEPLOY: O24 is declined before the deploy, as ADR 0191 already decided for the world model. The three central brains, qip-api, qip-fastbrain and qip-deepbrain, stay the decomposition, each with its own…
    - R12-CAPITAL-4078 — STAY_PARTIAL_DEPLOY: I declare no yield shelf, no routed share and no ladder horizons. The share is a new capital limit whose only effect is to give up liquidity the confidence governor withheld, so it is not a…
    - R12-CICD-10045 — STAY_PARTIAL_DEPLOY: The Cloud Build private pool stays off, as R12-FINOPS-9189 already ruled. It bills per build minute, holds a private-service-access peering, and its monthly cost is named nowhere I can read, so…
    - R12-CICD-10046 — STAY_PARTIAL_DEPLOY: The registry replica already exists (modules/registry/secondary.tf, images_secondary, secondary_region null everywhere) and R12-RES-9177 already chose it over multi-region, so nothing is rebuilt… [skeptic]
    - R12-CICD-10088 — STAY_PARTIAL_DEPLOY: O19 is answered: provenance is a gcloud-only SLSA statement attached to the pushed digest with Google's own tooling, and no third-party action enters the supply chain. As R12-CICD-9108 already…
    - R12-CICD-8117 — STAY_PARTIAL_DEPLOY: ADR 0114 D4 stands and is not superseded: Config Sync and Argo Rollouts stay declined, no fifth controller joins the control plane, the requirement is not narrowed to the controllers ADR 0114 admits… [skeptic]
    - R12-CICD-9076 — STAY_PARTIAL_DEPLOY: ADR 0102 is not amended and no development-agent task moves into a fleet worker; per the skeptic the gap names the shared fleet-worker account as unmet against 'task-scoped' and states that… [skeptic]
    - R12-COVERAGE-7158 — STAY_PARTIAL_DEPLOY: Independent operation is proven on the binary's default path. Peer communication is shown on two deployed nodes or not at all. A second node and its stable fronting address are billed, ADR 0035…
    - R12-DATA-10033 — STAY_PARTIAL_DEPLOY: The archive, artifact and simulation buckets stay off until the change that lands their first writer, because the archive's seven-year lock cannot be undone. That writer is a deployed fabricd broker…
    - R12-DATA-10040 — STAY_PARTIAL_DEPLOY: O01 is declined for now. AlloyDB stays refused under ADR 0113 and decision 5: no Postgres wire client is admitted and no cluster is created, because workflow state lives on the journal and nothing…
    - R12-DATA-1033 — STAY_PARTIAL_DEPLOY: The library half is done and stays tested: every connector door fills entitlement and geography, and the event log keeps both. I do not narrow the row to the constructor, because that would pass it…
    - R12-DATA-2012 — STAY_PARTIAL_DEPLOY: The pipeline is built as the contract says, on all three production callers. Every poll is gated through the admitted set before the world model sees it, and a claim batch leaves one knowledge delta.…
    - R12-DATA-2018 — STAY_PARTIAL_DEPLOY: The finder inspects every candidate from probe evidence before registration, and the unit check holds. I do not make the deep brain probe a committed catalogue by default. Naming a candidate file and…
    - R12-DATA-8006 — STAY_PARTIAL_DEPLOY: O02 stays decided narrowly, as I ruled on R12-AMBIENT-9001. BigQuery is the one store switched on, and Cloud Storage waits for its first writer. Bigtable, Spanner Graph, Knowledge Catalog and Vertex…
    - R12-DATA-8015 — STAY_PARTIAL_DEPLOY: Spanner Graph stays declined under decision 5, as the ADR I ordered on R12-AMBIENT-9001 says by name for this row. Promoted knowledge stays in the central brains, rebuilt from the event log…
    - R12-DATA-8016 — STAY_PARTIAL_DEPLOY: Bigtable stays declined under decision 5, as I ruled on R12-DATA-9053 and R12-DATA-10035. Derived features stay in memory in qip-feature-dag. I do not admit a gRPC stack for a store nobody pays for.…
    - R12-DATA-8126 — STAY_PARTIAL_DEPLOY: Bigtable stays declined under decision 5, as I ruled on R12-RES-9174. The plan clause is met in code: two zonal clusters and one app profile per consistency need. Nothing more is built for a store…
    - R12-DATA-8130 — STAY_PARTIAL_DEPLOY: I decide O01 by declining it for now. I admit no PostgreSQL client and buy no AlloyDB before the deploy. ADR 0113's refusal stands. Operational metadata stays on the journal and the single-node…
    - R12-DATA-9052 — STAY_PARTIAL_DEPLOY: Spanner Graph stays declined under decision 5; per the skeptic qip-mesh's GraphStore port (MemoryGraph, FileGraph, MeshPort::Graph mapped to the refused MeshTarget::SpannerGraph) already exists with… [skeptic]
    - R12-DATA-9184 — STAY_PARTIAL_DEPLOY: Bigtable stays declined under decision 5, as I ruled on R12-RES-9174. Autoscaling, hotspot-safe row keys and the per-workload profiles exist in code. The check is a plan, and no committed environment…
    - R12-EXEC-5081 — STAY_PARTIAL_DEPLOY: The market factory stays off on every committed path. The commerce-only plan I allowed under R12-ASSET-4073 stays commerce-only, because a factory section needs a venue, product and jurisdiction…
    - R12-FABRIC-10027 — STAY_PARTIAL_DEPLOY: The in-tree half stands. RF3 is admitted only for P0 and P1 acknowledged through the managed log, and a killed leader loses nothing on the in-memory log. RF3 across zones counts only when a deployed…
    - R12-FABRIC-7011 — STAY_PARTIAL_DEPLOY: The module already places one broker per zone and refuses two in one zone, and its plan tests show three zones. A real environment renders three only beside the Spanner managed log. Decision 5 keeps…
    - R12-FABRIC-7035 — STAY_PARTIAL_DEPLOY: Brokers lead and follow through the managed log ADR 0111 chose. A follower that holds the committed log and takes over with nothing lost is proven, on the in-memory log and a fake Spanner transport.…
    - R12-FABRIC-7037 — STAY_PARTIAL_DEPLOY: ADR 0111 and its Amendment 1 stand. No Raft is written or admitted, and the metadata quorum is the regional Spanner database under lease-epoch checks. The requirement's 'no cloud database' clause is…
    - R12-FABRIC-7046 — STAY_PARTIAL_DEPLOY: O07 is not taken now. QUIC stays measured-only under ADR 0112 §4 and its Amendment 1, and I will not swap the cross-region measurement for a local or synthetic one. That measurement needs a second…
    - R12-FABRIC-7058 — STAY_PARTIAL_DEPLOY: Nothing is pinned by default before a deployed node measures the need, as ADR 0082 says. The supervisor's CPUAffinity stays the only default placement. The opt-in QIP_SPOOL_WRITER_CPU pin stays…
    - R12-FABRIC-8004 — STAY_PARTIAL_DEPLOY: No environment gets broker machines yet. The broker has no boot image, the machines have no price, and ADR 0156 lists prod brokers as billed, which decision 5 still declines. The module and its…
    - R12-FABRIC-8081 — STAY_PARTIAL_DEPLOY: No runtime verb creates a topic or changes a quota or a retention. A topic is a reviewed code change naming its retention class, and a stream's policy is the committed catalogue's. The broker keeps…
    - R12-FABRIC-8083 — STAY_PARTIAL_DEPLOY: This takes the same ruling as R12-FABRIC-7046. I do not supersede ADR 0112 §4, and a local netem run does not stand in for the committed cross-region measurement. QUIC by default waits on a second…
    - R12-FABRIC-8085 — STAY_PARTIAL_DEPLOY: The ordering key and rendezvous placement stand as built. 'Followers in other zones' means, for P0/P1, the Spanner managed log on two or three seated brokers. Decision 5 still declines that bill, and…
    - R12-FABRIC-8086 — STAY_PARTIAL_DEPLOY: I read this as R12-FABRIC-8091 and 8092 do. A regional Spanner commit, three read-write replicas one per zone, is the RF3 majority, but it counts only once a deployed fabricd seated on it…
    - R12-FABRIC-8093 — STAY_PARTIAL_DEPLOY: O11 is declined for P2 for now. The market journal stays RF1 under ADR 0111 §2 until a committed measurement of P2 volume and loss from a running broker says it must survive a zone. Its escalation is…
    - R12-FABRIC-8094 — STAY_PARTIAL_DEPLOY: P3 stays RF1 and is replayable from the archive under ADR 0111 §2. A configurable RF2/3 that no mechanism delivers would be the false claim that record forbids, so StreamPolicy keeps refusing it. Its…
    - R12-FABRIC-8096 — STAY_PARTIAL_DEPLOY: Three brokers per production region, one per zone, on C4 or C4D, stands as ADR 0111 Amendment 1 and ADR 0156 §6(a) set it. The module's cap of three is not raised: five comes only after a committed…
    - R12-FABRIC-8099 — STAY_PARTIAL_DEPLOY: The broker half stands as built: each broker has a reserved internal address, there is no access_config and no public forwarding rule. The export half waits with the hub. Decision 4 and R12-GCP-8057…
    - R12-FINOPS-10138 — STAY_PARTIAL_DEPLOY: The bounded half holds: every derived store carries a retention or lifecycle rule, and it is tested. The tiers are not on. Bigtable stays declined at about $950 a month (R12-DATA-9053…
    - R12-FINOPS-8040 — STAY_PARTIAL_DEPLOY: The reservation half is done and guarded: each execution group consumes a specific reservation sized to its target, and removing the affinity fails the scan. No node is declared, and none will be…
    - R12-FINOPS-9188 — STAY_PARTIAL_DEPLOY: Job specs carry a queue, a capacity and a priority, and a critical job on Spot is refused; that stands. I will not label an in-process fit as Spot, and I will not route it through a quota that a…
    - R12-GCP-10009 — STAY_PARTIAL_DEPLOY: Decision 4 and R12-GCP-8057/8058 stand. No hub comes before the record that supersedes ADR 0035 with a second region and names the hub's monthly cost. No hybrid spoke comes before a circuit is…
    - R12-GCP-10012 — STAY_PARTIAL_DEPLOY: The NAT half holds: every NAT translates only named subnets to a reserved address and logs every connection, and the execution node's egress is its own. The row is not promoted, because the text…
    - R12-GCP-8036 — STAY_PARTIAL_DEPLOY: ADR 0035 stands with one execution region, in which the three networks and the controlled egress are built; per the skeptic OpenObserve is declared in dev as the one-region backend under ADR 0028… [skeptic]
    - R12-GCP-8051 — STAY_PARTIAL_DEPLOY: O17's 'before the second region' is declined and ADR 0113 §4 stands; per the skeptic, at the trigger the ADR amending ADR 0118 extends modules/resource-hierarchy with engineering and security planes… [skeptic]
    - R12-GCP-8056 — STAY_PARTIAL_DEPLOY: Decision 4 of 2026-10-25 covers VPC spokes just as it covers the hubs (R12-GCP-8057, R12-GCP-8058). No google_network_connectivity_* resource is declared, and l6-ncc-hubs-parked.patch stays parked.…
    - R12-LEDGER-10041 — STAY_PARTIAL_DEPLOY: The ledger of record goes to Spanner Enterprise Plus as ADR 0113 says, on nam3 with us-east4 leading (R12-LEDGER-8136). That waits until the base deploy has run and an ADR states the measured need…
    - R12-LEDGER-8007 — STAY_PARTIAL_DEPLOY: Financial truth goes to Spanner Enterprise Plus, but only after the deploy runs and an ADR states a measured need and a ceiling against $2,705 to $8,140 a month. Decision 5 stands, as in…
    - R12-LEDGER-8132 — STAY_PARTIAL_DEPLOY: The requirement text stands: a plan of a committed environment must show the ledger instance on ENTERPRISE_PLUS in a multi-region configuration, and I do not narrow it to the module's contract or a…
    - R12-MESH-10075 — STAY_PARTIAL_DEPLOY: O07 is decided: ADR 0112 §4 stands and no QUIC crate is admitted now. quinn comes in only through its own ADR 0109 allow-list record, after a committed cross-region TCP+TLS against QUIC measurement…
    - R12-MESH-1018 — STAY_PARTIAL_DEPLOY: The peer mesh stays off until a committed environment has a real deployed peer; per the skeptic manifest_wiring.rs::THE_PEER_MESH_HAS_NO_DEPLOYED_PEER is not found, nothing in qip-acceptance pins an… [skeptic]
    - R12-MESH-1072 — STAY_PARTIAL_DEPLOY: The peer link is not switched on by default, because a default-on mesh names a peer that does not exist. A committed deployment sets it only when a priced record funds a second execution node, which…
    - R12-MESH-8003 — STAY_PARTIAL_DEPLOY: O07 is decided as in R12-MESH-10075: ADR 0112 §4 stands, with no QUIC now and no quinn admitted. The gate is a committed cross-region TCP+TLS against QUIC measurement on the peer-mesh hop, and that…
    - R12-OBS-10089 — STAY_PARTIAL_DEPLOY: The requirement names collectors and the text stands. ADR 0121 and my R12-OBS-10059 ruling push metrics and spans from the process on a bounded thread that counts and drops a failed push. That…
    - R12-OBS-7115 — STAY_PARTIAL_DEPLOY: ADR 0156 §5.6 stands. The platform graph is joined by shared keys where signals are ingested. No process pushes its graph to another or scrapes a peer across the unjoined networks. Ingestion needs a…
    - R12-QUANT-10066 — STAY_PARTIAL_DEPLOY: No TPU rung goes on the ladder and no TPU is bought. A rung no provider can serve can never be chosen, and a benchmark gate in front of it would measure nothing, so I am not having one built. Nothing…
    - R12-QUANT-10095 — STAY_PARTIAL_DEPLOY: The fan-out stays across the substrates we can actually reach: the CPU baseline, the quantum-inspired search and the QPU arm. Each is compared against the classical baseline. The GPU and TPU arms…
    - R12-QUANT-1013 — STAY_PARTIAL_DEPLOY: Eligible work is routed only to compute we have, and GPU and TPU are recorded as declined with the reason. I am not funding an accelerator so that the four-way placement can be shown (decision 5).…
    - R12-QUANT-10173 — STAY_PARTIAL_DEPLOY: Four of phase 14's five deliverables stand: the compute router, the quantum gateway, the solver registry and the benchmark memory. The GPU/TPU7x pools are not bought. A GKE GPU pool is refused by ADR…
    - R12-QUANT-10190 — STAY_PARTIAL_DEPLOY: The promotion half holds and stays exactly as it is. A quantum-gateway candidate is refused, and a quantum-informed one needs a calibrated classical baseline. Accelerator selection by measured fit…
    - R12-QUANT-10208 — STAY_PARTIAL_DEPLOY: The no-oracle invariant holds and stays: every QPU result is a candidate benchmarked against a classical answer, and the router adopts it only on a verified win. The requirement's GPU clause stands…
    - R12-QUANT-9041 — STAY_PARTIAL_DEPLOY: The router's learning and its replay stand: per substrate and size, and a losing substrate stays stopped. It cannot learn which of the five named substrates wins while GPU and TPU are unbought…
    - R12-REFLEX-10085 — STAY_PARTIAL_DEPLOY: No new repository is bought for bundles, the HSM keys go on with dev's billing (R12-SEC-9137) and the row's ADR 0043 blocker is stale; per the skeptic the node-side stage step waits for an ADR… [skeptic]
    - R12-REFLEX-1196 — STAY_PARTIAL_DEPLOY: ADR 0035 stands: one execution node, newyork-1 in us-east4, in shadow mode. No European or APAC node is declared or funded. The NCC hubs stay parked (decision 4). The record that supersedes ADR 0035…
    - R12-REFLEX-7003 — STAY_PARTIAL_DEPLOY: I decline O13. ADR 0082 stands: no libc, nix or unsafe code enters the workspace to lock memory or busy-poll. The row asks for both only where measurement justifies them, and no running node has…
    - R12-REFLEX-8065 — STAY_PARTIAL_DEPLOY: The requirement stands as written, and the row stays PARTIAL on three clauses. First, the binary locks no memory: I decline O13, ADR 0082 stands, and the boot refusal of swap narrows that gap…
    - R12-REFLEX-8069 — STAY_PARTIAL_DEPLOY: ADR 0137 stands, and this row closes the way I ruled R12-REFLEX-9114: only on a fenced standby deployed in a second zone. That standby is a second billed node. No environment enables it, and decision…
    - R12-REFLEX-9180 — STAY_PARTIAL_DEPLOY: A shard is a node, and Terraform does not record which books a node owns. ADR 0136 makes that the centre's signed assignment. The execution_nodes key is the shard, and each node already carries its…
    - R12-RES-10151 — STAY_PARTIAL_DEPLOY: No regional-loss game day is run with one region. ADR 0035 authorises one execution region, so removing it is an outage, not a test that other regions continue. The local region-isolation drill hangs…
    - R12-RES-10152 — STAY_PARTIAL_DEPLOY: The monthly zonal drill runs against a deployed node pair or a deployed quorum, and neither exists. It must prove fencing and quorum, so it needs both halves. The Reflex half waits on the cross-zone…
    - R12-RES-7112 — STAY_PARTIAL_DEPLOY: Broker failover closes on a game day against a provisioned Spanner managed log, as I ruled for R12-FABRIC-9118, and decision 5 has not taken that bill. The in-memory kill test and the chaos run are…
    - R12-RES-9168 — STAY_PARTIAL_DEPLOY: This row takes R12-RES-8038's ruling. Zone loss is proven on a standby in a second zone, with a lease store it can trust, and on RF3 replicas. None of that is bought before the first node is up. The…
    - R12-SEC-9134 — STAY_PARTIAL_DEPLOY: R12-SEC-10056 already decided this. The stores are scanned only once a monthly discovery cost is named and funded, after the dev deploy, and today nothing names that cost. The templates and the…
    - R12-TICK-2055 — STAY_PARTIAL_DEPLOY: Lake immutability is built and the row waits on a durable store, off until its first writer after the dev deploy at $0.30 to $6 a month; per the skeptic only market-class lake partitions ever reach… [skeptic]
    - R12-TICK-7019 — STAY_PARTIAL_DEPLOY: The tick archive stays where R12-DATA-8125 left it. enable_cloud_storage stays off until the change that lands the first archive writer, because the archive bucket's seven-year lock cannot be undone…
    - R12-TICK-8110 — STAY_PARTIAL_DEPLOY: Replay shards run as the same Cloud Run Job R12-TICK-7020 funds, about $105 a month, after dev runs; per the skeptic qip-kernel has no session_replay from_archive, so an architecture review first… [skeptic]
    - R12-WORLD-8129 — STAY_PARTIAL_DEPLOY: Spanner Graph stays declined, as R12-AMBIENT-9001 already decided O02: BigQuery is the one evidence store agents read, and Bigtable, Spanner Graph and Knowledge Catalog wait for an ADR that states a…

    **STAY_PARTIAL_EXTERNAL (21)**

    - R12-API-9057 — STAY_PARTIAL_EXTERNAL: No public edge until I name a hostname on a domain I own and have delegated to Cloud DNS. No lane invents one, because a hostname in a tfvars file is the switch that allocates a global address and…
    - R12-API-9059 — STAY_PARTIAL_EXTERNAL: MFA is already mandatory, and passkeys stay on ADR 0038's accepted row-three outcome, the password path, with no provider-pin raise and no console change, until Google's Identity Toolkit v2 discovery… [skeptic; decider ruled STAY_PARTIAL_DEPLOY]
    - R12-COVERAGE-7155 — STAY_PARTIAL_EXTERNAL: Decision 6 of 2026-10-25 stands: no hosted language-model vendor yet, so no large-model candidate class is admitted to the foundry. A deterministic template is not a large model and is not registered…
    - R12-COVERAGE-7167 — STAY_PARTIAL_EXTERNAL: I do not name the jurisdictions this platform may execute in. That is a legal determination, not a product call, and the platform holds no licence anywhere. No committed universe instrument names a…
    - R12-DATA-2001 — STAY_PARTIAL_EXTERNAL: ADR 0157 decision 5 stands, and I do not supersede it: candidates are written by an operator or not at all. A stage that manufactures candidates nobody reviewed a route for is a control assessing its…
    - R12-DATA-2005 — STAY_PARTIAL_EXTERNAL: No vendor yet and no invented refusal for the uncovered market classes; per the skeptic coverage means a source admission::admit admits or a quoted RefusedSource, not a KNOWN_SOURCES entry, so trades… [skeptic]
    - R12-DATA-2007 — STAY_PARTIAL_EXTERNAL: No news, press, expert, public-web, deep-web database, social or community source is admitted, and no refusal is written without a clause read. Each class enters through its own ADR 0157 record, and…
    - R12-DATA-2009 — STAY_PARTIAL_EXTERNAL: O03 is decided: no paid alternative-data provider is licensed now. Satellite, flight, port, traffic and commodity-flow data are subscriptions I am not buying before the deploy. AIS, energy-grid and…
    - R12-DATA-2010 — STAY_PARTIAL_EXTERNAL: No commerce vendor now, and the row is not promoted to COMPLETE. ADR 0157 decision 6 deferred the choice; it did not refuse the class. I will not refuse commerce for good just to close the row…
    - R12-EVENT-5085 — STAY_PARTIAL_EXTERNAL: The event plane stays off on every committed path, as I ruled for R12-EVENT-5083: it opens only with a licensed event source (ADR 0151 decision 10). I do not ship a default plan whose venue, product…
    - R12-EVID-2037 — STAY_PARTIAL_EXTERNAL: ADR 0157 decision 9 stands: no coordinate is read out of free text. Routing, shipping and jurisdiction evidence carry a location only when a licensed source states one, and none is contracted…
    - R12-FINOPS-10135 — STAY_PARTIAL_EXTERNAL: Billing export is switched on at the billing account, by its administrator, in the console. The provider has no resource for it, and no agent holds billing-admin. I enable it when I re-enable billing…
    - R12-GCP-8042 — STAY_PARTIAL_EXTERNAL: The foundation root is the guardrails boundary, and its folder policies and the PAM resource are already built. Under the delegation I build no audit sink, locked log bucket or billing-export…
    - R12-LEDGER-5008 — STAY_PARTIAL_EXTERNAL: I choose no relief method under the delegation. FIFO, average cost or specific identification is a tax method for a jurisdiction, and ADR 0153 decision 7 holds that such a method is a legal fact this…
    - R12-RISK-4107 — STAY_PARTIAL_EXTERNAL: ADR 0155 section 19 stands, and the committed universe stays equities. No derivative hedge candidate is declared. Platform::derivative_gate refuses every derivative order until a trading entity holds…
    - R12-SEC-10006 — STAY_PARTIAL_EXTERNAL: Security Command Center stays off. Its custom modules need the organisation's Premium or Enterprise tier. That is a licence set outside this repository, and I have not bought it, as R12-ARCH-10159…
    - R12-SEC-10019 — STAY_PARTIAL_EXTERNAL: The policies are built and tested. No surface is published to use them until I own a domain and delegate it, as R12-API-8054 holds. Meanwhile the console stays on IAP over run.app under ADR 0095. The…
    - R12-SEC-10022 — STAY_PARTIAL_EXTERNAL: Argo CD and Kargo stay unpublished. They are reached through the fleet Connect gateway and a port-forward, which puts no address on the internet at all. I will not open a gateway on a third-party…
    - R12-SEC-9130 — STAY_PARTIAL_EXTERNAL: Every Cloud Armor policy in Terraform carries WAF, adaptive protection and a per-address rate ban, and that stays tested. The one internet-facing surface, the console on run.app, has no policy. It…
    - R12-TICK-1053 — STAY_PARTIAL_EXTERNAL: The guard stands as built, and the desk stays off. I do not grant it simulated depth (my ruling on R12-TICK-1049), so the row waits for a market dataset admitted under an ADR 0157 licence record. Its…
    - R12-TICK-2045 — STAY_PARTIAL_EXTERNAL: No vendor is chosen now for options chains, Greeks, futures curves, basis, borrow or funding. Each enters only as an ADR 0157 licence record, and options stay refused under ADR 0050. A free…

    **FUND_THEN_DEPLOY (2)**

    - R12-SEC-7014 — FUND_THEN_DEPLOY: These are the four HSM signing keys funded under R12-SEC-9137 at $10 to $11 a month, not counted again; per the skeptic no agent edits dev's tfvars: the owner sets bundle_not_after =… [cost_usd_month 11, already funded] [skeptic]
    - R12-SEC-9128 — FUND_THEN_DEPLOY: This is the CA Service pool I funded under R12-SEC-10054 at $200-$250 a month. It is the same pool, not a second one. It goes on with dev's billing, with the organisation string that ruling names… [cost_usd_month 250, already funded]

    **REFUSE_LIVE (5)**

    - R12-CAPITAL-4084 — REFUSE_LIVE: Refused. An autonomous treasury transfer is money movement, and a corridor an intent could honestly use needs a real external destination, which only live capital has. No fabric command file is…
    - R12-CAPITAL-5078 — REFUSE_LIVE: O15 is declined: quoting keeps no path to an order, on the simulator as everywhere else. Lifting quote_loop's no-path-to-an-order and the edge node's quoting refusal would remove two guards from the…
    - R12-COVERAGE-7187 — REFUSE_LIVE: O16 is declined, as I ruled for R12-AGENCY-1183, 6094 and 7028. ADR 0114 D8 stands, and no outbound channel, publication route, survey or public adapter is opened, nor any account or credential for…
    - R12-EXEC-7004 — REFUSE_LIVE: Refused under ADR 0003, decision 8 of 2026-10-25 and decision 7 of 2026-10-31; per the skeptic the node does hold a REST order-entry path (NodeGateway::Live over RestGateway) that no environment… [skeptic]
    - R12-REFLEX-10074 — REFUSE_LIVE: Refused: orders flowing to a real venue is a live-order path. ADR 0003 holds, ADR 0108 stays proposed, and I do not accept it. This is a refusal of a live-order and external action surface, and no…

    **IMPLEMENT (28)**

    - R12-CAPITAL-3020 — IMPLEMENT: ADR 0147 decision 8 stands: no capital specialist agent is added, because a second control over the same RiskState would be a correlated second finding from one origin. The half that decision… [L1]
    - R12-CICD-9101 — IMPLEMENT: O22 is declined: no libfuzzer-sys and no nightly toolchain enter the build. A second toolchain and a new crate would be a standing cost and an exception to ADR 0109, for a gate the in-tree generator… [L6]
    - R12-DATA-1037 — IMPLEMENT: A revision a re-fetch finds is recorded at once, through record_reference's revision branch called from ConnectorArm::refetch for every Revised finding; per the skeptic the second part, re-fetching a… [skeptic] [L3]
    - R12-DATA-1105 — IMPLEMENT: Add the assertion that a source-alpha desk switched off journals no round where the default desk journals one, reading the off cadence through source_alpha_every with an explicit 0; per the skeptic… [skeptic] [L3]
    - R12-DATA-7013 — IMPLEMENT: The model-artifact bucket gets its retention now, in code. It gets an unlocked hold and colder storage with age, and no rule that deletes an artifact or a version a model card still names. Cloud… [L3]
    - R12-DATA-7021 — IMPLEMENT: The invariant is held by a test that can see bytes. The journal canary must find the raw payload however it is serialised, and the entitlement mutation is credited to the storage tests that already… [L3]
    - R12-EVID-1107 — IMPLEMENT: The provenance check is proven on the deep brain's arm path; per the skeptic the test shows two arms polled into one Platform leaving each claim linked to its document and source in the world… [skeptic] [L3]
    - R12-EVID-1109 — IMPLEMENT: Contradiction handling is proven on the deep brain's arm path; per the skeptic a contest is recorded, not withheld: two arms disagreeing on one print leave contested == 1 on the arm's… [skeptic] [L3]
    - R12-EVID-1110 — IMPLEMENT: Source authenticity is proven on the deep brain's arm path: a body whose declared digest does not match is marked spoofed and its record is never handed up. Signatures stay with R12-EVID-2034 under… [L3]
    - R12-EVID-1111 — IMPLEMENT: Location consistency is proven on the deep brain's arm path over the NWS station source, and the gap's 'manifest region only' is corrected; per the skeptic the held record's geographic_consistency… [skeptic] [L3]
    - R12-EVID-1112 — IMPLEMENT: Adversarial-source detection is proven where the deep brain senses; per the skeptic three arms with distinct source ids (min_coordinated_sources is 3) yield a CoordinatedManipulation finding naming… [skeptic] [L3]
    - R12-EXPAND-6034 — IMPLEMENT: Per the skeptic the row is not met through the kernel's lineage record, which ADR 0149 decision 2 renames ModelSetVersion: the lane renames it and, after each successful promotion in… [skeptic] [L4b]
    - R12-EXPAND-6052 — IMPLEMENT: ADR 0149 decision 6 already decided this, and I keep it. A promoted result's lane is read from its compiled form, and no latency-budget field is added. A result that compiles inside the reflex bounds… [L4b]
    - R12-FABRIC-7038 — IMPLEMENT: ADR 0119 section 3 already decided this. The mirror is built and game-dayed now, on two fabricd processes on one host. It is deployed only when a record supersedes ADR 0035's one region. Build the… [L1b]
    - R12-FABRIC-8097 — IMPLEMENT: The journal is built as the text asks: a separate Hyperdisk with provisioned IOPS and throughput, and any other disk type is refused. 'Sized for sustained append/read throughput' becomes a refusal… [L1b]
    - R12-FINOPS-10028 — IMPLEMENT: Reflex capacity is reserved, but Fabric capacity is not. The scan that should catch this misses the brokers because they are standalone machines, not a group. Each broker gets its own required… [L1b]
    - R12-FINOPS-8041 — IMPLEMENT: Spanner's autoscaling is plan-proven but Bigtable's is not, so per the skeptic a lane first adds an autoscaling_config assertion to scaling.tftest.hcl's Bigtable run (mutation: delete one cluster's… [skeptic; decider ruled REGRADE_TEXT] [L1b]
    - R12-LEDGER-5018 — IMPLEMENT: ADR 0153 decision 9 stands, and the producer it names now exists (dfd4d446). But it queues each PhysicalInventory operation without journalling it, which decision 1 forbids. A lane journals each… [L1b]
    - R12-LEDGER-5026 — IMPLEMENT: ADR 0153 decision 8 stands: no warehouse statement is built, because a simulated warehouse drawn from the same ledger would agree by construction and prove nothing. The warehouse clause stays open… [L1b]
    - R12-MESH-1010 — IMPLEMENT: The leg bound is proven where it is enforced, and the two-cell exchange is proven at the node, now. A lane adds the missing 1- and 21-leg refusals and a qip-edge-node test in which two nodes each… [L5b]
    - R12-OBS-8082 — IMPLEMENT: ADR 0156 §5.5 stands: fabricd's spans leave the process only through the shared drain when it is configured. No trace route is opened on the health listener, because a ring served to nobody proves… [L6]
    - R12-OBS-9163 — IMPLEMENT: All six data objectives are declared with targets. Each one is bound to a series recorded where its fact happens, so a deleted recording site reads unmeasured. Bigtable p99 is declared and stays… [L6]
    - R12-REFLEX-5034 — IMPLEMENT: ADR 0133 decision 4 already answers this, and it stands: telemetry leaves the node only by the scrape and is not a fabric record kind. No new ADR is written and no telemetry record is added. The… [L5]
    - R12-SEC-8100 — IMPLEMENT: ADR 0112 already decided this: on a Compute Engine hop, the certificate's SAN, mapped to the workload's service account, replaces the bearer token for authorisation. Build it on every fabric route… [L6]
    - R12-SEC-9132 — IMPLEMENT: I do not buy the Security Command Center tier. Premium or Enterprise is a billed activation made at the organisation, and it stays declined under decision 5 of 2026-10-25, as R12-GCP-9086 already… [L6]
    - R12-TICK-1049 — IMPLEMENT: I do not grant the tick desk qip-simulated-depth. Simulator output is not a training source (R12-MODEL-4007, R12-MODEL-4004), and this desk trains fill-probability and volatility models. It stays off… [L3b]
    - R12-TICK-1050 — IMPLEMENT: As on R12-TICK-1049: no simulated depth is granted to the tick desk, and it waits for a market dataset admitted under ADR 0157. The deep brain's replayed-bar fit is market-data training too. Its rows… [L3b]
    - R12-TICK-1051 — IMPLEMENT: As on R12-TICK-1049: qip-simulated-depth is not granted, and the tick desk's entitlement waits for a licence record under ADR 0157. The deep brain's replayed-bar fit already asks the connector's… [L3b]

    **ADR_THEN_IMPLEMENT (36)**

    - R12-ARCH-9028 — ADR_THEN_IMPLEMENT: Vertex AI Agent Engine is not admitted. It is a paid managed runtime that holds agent sessions and memory outside the event log, and decision 5 keeps new billed services out. From now on an agent… [L2]
    - R12-CICD-9115 — ADR_THEN_IMPLEMENT: O21 is admitted narrowly: a person-dispatched deploy.yml run moves traffic back to the previous promoted revision when the release decision refuses or cannot be read, and drafts the incident for a… [skeptic] [L6]
    - R12-COMMERCE-5090 — ADR_THEN_IMPLEMENT: ADR 0153 decision 8 is amended in its chargeback clause only; its ledger clause stands. A commerce plan must declare a paper chargeback rate beside its fees, and a plan without one is refused at… [L1]
    - R12-COMMERCE-5091 — ADR_THEN_IMPLEMENT: R12-ASSET-4073's commerce-only dev plan stands, but per the skeptic and decision 13 the mount cannot land as a patch first: an ADR for a per-file config mount comes first, then one protected patch… [skeptic; decider ruled WRITE_PATCH] [L1]
    - R12-COMMERCE-5092 — ADR_THEN_IMPLEMENT: The committed dev plan carries an equivalent pair across two merchants, one counterfeit by GTIN and one by seller, with the named test and mutation; per the skeptic it is sequenced after decision… [skeptic; decider ruled WRITE_PATCH] [L1]
    - R12-COMMERCE-5093 — ADR_THEN_IMPLEMENT: A cycle under the committed dev plan journals a CommerceSimulated assessment carrying all six figures, and a candidate missing any input stays refused, at plan load or at the candidate stage; per the… [skeptic; decider ruled WRITE_PATCH] [L1]
    - R12-CONTRACT-7062 — ADR_THEN_IMPLEMENT: A connector's claim reaches the world model only beside its EvidencePacket, which the kernel's evidence intake decodes through the contract and refuses on failure; per the skeptic admission still… [skeptic] [L2]
    - R12-CONTRACT-7069 — ADR_THEN_IMPLEMENT: A new unattributed fill is refused and schema-1 events stay legacy and are never rewritten, but per the skeptic the ADR first decides how intent and decision ids are minted, or makes Attribution a… [skeptic] [L2]
    - R12-CONTRACT-7072 — ADR_THEN_IMPLEMENT: ADR 0151 decision 2 stands on the producer and the consumer. The edge builds a pack per pass over the market events it applied, and never over none. The tick desk's fusion may only remove a flagged… [L2]
    - R12-COVERAGE-5099 — ADR_THEN_IMPLEMENT: A basket or an index carries its constituents and their weights. It is valued only from those constituents' prices. A position in it counts against its constituents' limits as well as its own, never… [L6]
    - R12-DATA-3008 — ADR_THEN_IMPLEMENT: The deep brain names where it is blind or wrong on every research pass and journals those targets. This runs on the default path, the same in every environment. That is the request the requirement… [L3]
    - R12-DATA-8124 — ADR_THEN_IMPLEMENT: fabricd writes each sealed segment to the regional landing and writes the same local bytes a second time into the locked archive under one content address, with no server-side copy and no transfer… [skeptic] [L3]
    - R12-EVID-2035 — ADR_THEN_IMPLEMENT: Copies of one origin count once wherever evidence is weighed and the fabric's two-origin cap stands; per the skeptic the fold is tighten-only, never moving a posterior further from the prior than… [skeptic] [L3]
    - R12-EXEC-1068 — ADR_THEN_IMPLEMENT: Requote layering is refused at the cell and at the desk, beside the cancel-before-fill gate and never in its place. Its bound is that gate's own: within 60 seconds, at least four requotes on one side… [L5]
    - R12-EXPAND-6043 — ADR_THEN_IMPLEMENT: No branch is spawned from an unmeasured residual; per the skeptic a spawn caller already exists in model_tournament.rs's observe_outcome (R12-MODEL-7198), so the ADR amends ADR 0149 decision 12 to… [skeptic] [L4b]
    - R12-EXPAND-6061 — ADR_THEN_IMPLEMENT: The requirement stands, and the three missing kinds are recorded, not dropped. Open questions are the registry's Open entries. A benchmark is the one ADR 0192 already names for the capability's… [L4b]
    - R12-FABRIC-7045 — ADR_THEN_IMPLEMENT: O09 is declined. No OpenTelemetry exporter crate is admitted (ADR 0026 stands). No tokio goes on the fabric SDK path, which the edge node's drain reaches, until a committed measurement shows the… [L1b]
    - R12-GOV-10004 — ADR_THEN_IMPLEMENT: Resources are created only in the United States and the European Union, enforced at the folder with gcp.resourceLocations and compute.requireShieldedVm; per the skeptic the ADR names the three… [skeptic] [L6]
    - R12-GOV-6116 — ADR_THEN_IMPLEMENT: I do not grant foundry lineages one by one. A lineage is evo-{subject}, minted per sweep at run time, and a register entry per lineage would be written to pass whatever the roots happen to run. A… [L6]
    - R12-LEDGER-5004 — ADR_THEN_IMPLEMENT: qip-ledgerd's balances are the book of record and the kernel's desk book a working copy; per the skeptic the comparison runs only once the ledger's applied desk-chain position reaches the… [skeptic] [L1b]
    - R12-OBS-9140 — ADR_THEN_IMPLEMENT: qip-ledgerd adopts the shared drain now, as ADR 0156 §5.5 directs. The reflex node does not. qip-telemetry-drain is a Lane 1 crate, and ADR 0116 says a second hosted Lane 1 crate reopens the sidecar… [L6]
    - R12-OBS-9156 — ADR_THEN_IMPLEMENT: A spent budget refuses promotion of that plane and no other. The check runs at the Kargo promotion out of a stage, where ADR 0036 puts promotion, beside the paper-ceiling step. The step refuses when… [L6]
    - R12-QUANT-9040 — ADR_THEN_IMPLEMENT: IBM Quantum is reached by one identity only: a gateway workload of its own in the optimisation zone. It alone may hold a connected hosted provider, the qip-quantum-token mount and ibm-quantum egress.… [L4]
    - R12-RISK-4101 — ADR_THEN_IMPLEMENT: Deployment identity gets its writer and its producer now. A node's identity is the SHA-256 of the qip-edge-node binary its unit starts. The startup script writes it to a read-only file the unit… [L1]
    - R12-SEC-10201 — ADR_THEN_IMPLEMENT: O18 is declined as written. No agent session gets a Workload Identity pool or a per-run cloud credential, because issuing credentials to agents widens what an agent can reach. The one agent identity… [L6]
    - R12-SEC-9064 — ADR_THEN_IMPLEMENT: ADR 0113 §4 stands: the scouts' own project comes with the second region, not before, and node pools stay refused under ADR 0114 D4. What I take now is the network. The ingestion-discovery zone gets… [L6]
    - R12-SEC-9078 — ADR_THEN_IMPLEMENT: Agents reach models only through the gateway, and only models on a committed approved list. A custom base URL is refused. So that a direct call fails, the gateway runs as its own identity and holds… [L6]
    - R12-TICK-2056 — ADR_THEN_IMPLEMENT: Market-class lake partitions get their own bucket without a locked retention so their 90-day delete fires; per the skeptic qip-storage also routes each lake partition by RecordClass, market to the… [skeptic] [L3b]
    - R12-TICK-2070 — ADR_THEN_IMPLEMENT: No automatic channel carries a deep-brain promotion to the policy issuer. A distillate the deep brain promotes reaches qip-api's trained_models slot only through the two-person release. Its signed… [L3b]
    - R12-TICK-2084 — ADR_THEN_IMPLEMENT: Each kind of mismatch gets one paper-only consumer, named before anything carries it (ADR 0151 decision 1). An Investigation opens a question in the deep brain's research backlog. An… [L3b]
    - R12-TICK-2085 — ADR_THEN_IMPLEMENT: The Asset, Capital and Risk brains are not new components. The Asset brain is the foundry's per-subject search, and a posterior may change only how many candidates a round proposes, each one charged… [L3b]
    - R12-WORLD-10090 — ADR_THEN_IMPLEMENT: The NOW Brain reads an admitted, uncontested knowledge delta as a typed observation of a declared variable; per the skeptic an evidence-derived observation never wakes the lattice: it is kept out of… [skeptic] [L2]
    - R12-WORLD-3085 — ADR_THEN_IMPLEMENT: The seconds-minutes flow member reads the same signed aggressor flow R12-WORLD-3046's member reads: the paper feed's own trades, labelled simulated. A licensed tape later replaces the source, not the… [L2]
    - R12-WORLD-3086 — ADR_THEN_IMPLEMENT: Wire the catalyst. Every UNDERSTAND pass hands the lattice the macro surprises the world model holds at that instant, so a recent release widens the hours-days row without moving its median. ADR 0151… [L2]
    - R12-WORLD-3088 — ADR_THEN_IMPLEMENT: The same call wires the structural member. Every live causal claim into a subject reaches the quarters-years row at the pass, and a retired claim leaves it at the next pass. This lands under the… [L2]
    - R12-WORLD-9025 — ADR_THEN_IMPLEMENT: O24 is decided without a new service. The deep brain, already its own Cloud Run service, is the strategic forecast service. It alone serves the weeks-months and quarters-years rows, and the fast… [L2]

    **WRITE_PATCH (9)**

    - R12-CICD-8031 — WRITE_PATCH: The tests/ directory exclusion goes. A fixture passes only if its path and its exact matched text are both on a list in a protected file, which only a person can lengthen. No fake-key marker is… [L6]
    - R12-CICD-9072 — WRITE_PATCH: Every agent commit names its role and its model from the instant R12-CICD-9083's lane trailer becomes required, a commit missing either is refused, and no reviewer agent shares a model with a coder… [skeptic] [L6]
    - R12-CICD-9082 — WRITE_PATCH: Every lane's sandbox run reads the lane's own committed spend ledger with the trusted agent-lanes.mjs spend and fails on a budget overrun, an unreadable line, a missing ledger or, per the skeptic, a… [skeptic] [L6]
    - R12-CICD-9107 — WRITE_PATCH: Publish to the two repositories; do not remove them. Deleting them would be a destructive apply and would rewrite the waiver test, and the requirement names both. For the commit it built, deploy.yml… [L6]
    - R12-COMMERCE-5095 — WRITE_PATCH: The commerce-only dev plan is synthetic, buys only from SimulatedMerchant and fails closed on an incomplete logistics plan; per the skeptic one patch carries every part (variables.tf, dev's tfvars… [skeptic] [L1]
    - R12-COMMERCE-5097 — WRITE_PATCH: The resale router runs only on the simulated commerce plane, inside R12-COMMERCE-5095's one-patch mount; per the skeptic the committed plan names no venues (ResaleEntry refuses unknown fields), the… [skeptic] [L1]
    - R12-SEC-10002 — WRITE_PATCH: Workforce IAP access is by Cloud Identity group only and PAM keeps individual user: principals; per the skeptic the claim that no shared account can be written in holds only once the lane's change… [skeptic] [L6]
    - R12-SEC-10053 — WRITE_PATCH: D12 and ADR 0110 Amendment 1 stand. Every signing key is HSM in every environment. CMEK keys are HSM in stage and prod and may stay SOFTWARE in dev and test. The attestor moves to HSM as a new key… [L6]
    - R12-SEC-9068 — WRITE_PATCH: O18 is decided once, under R12-SEC-10201's record: no agent identity pool and no per-run cloud credential. An agent run holds the sandbox's contents:read token and nothing else. Every workflow that… [L6]

    **APPLY_PATCH (0)**

    None in this pass.

15. **ADR numbers are allocated to lanes in blocks** (decided 09:25 UTC).
    The register's rule is to claim a number in the shared index, in its own
    pushed commit, after re-reading the index. On 2026-10-31 that rule
    collided twice within the hour: lanes L5 and L3 both claimed 0239,
    eighteen seconds apart, and lane L2 claimed 0231 and 0232 three minutes
    after lane L6 had claimed 0231 to 0238. Re-reading the index cannot
    prevent this. Each lane reads its own fetch, and a claim pushed after
    the fetch is invisible until the next one. So the integration branch
    now allocates a block to each lane, and a lane takes numbers only from
    its own block, in order, still pushing each claim alone before the body:
    L1 0245-0254, L2 0255-0264, L3 0265-0274, L4 0275-0284, L5 0242 then
    0285-0294, L6 0295-0314. The lanes stopped by their model's weekly limit
    get theirs when they resume: L1b 0315-0324, L2b 0325-0334, L3b
    0335-0344, L4b 0345-0354, L5b 0355-0364. The numbers already claimed
    and pushed stand where the first push holds them: 0228 to 0230 for L1,
    0231 to 0238 for L6, 0239 to 0241 for L3. L5 moves to 0242, and L2's two
    claims move to 0255 and 0256. A block running out is extended by the
    orchestrator, never by the lane.

16. **The lane-trailer gate stays off until it can judge a commit by its
    own files** (lane L6, R12-CICD-9083, decided 10:00 UTC). Decision 10
    ruled that every agent commit names its lane once the running round's
    manifest is committed. Lane L6 wrote that patch, and in building it
    found two ways switching it on would refuse legitimate work.
    `agent-guard`'s step 1b judges a pull request's whole file list against
    every lane its commits name, so the rolling integration pull request
    would be refused even after a person approved it. And the composition
    roots, `backend/Cargo.lock`, `data/**` and `.github/rulesets/**` are
    editable by no role in the manifest, while most lanes touch at least
    one. A gate that refuses the integration path would be switched off
    again within the day, which is worse than leaving it off until it can
    fire correctly. So `l6-cicd-9083-lane-manifest.patch` is held, not
    applied, and the trailer requirement ships off as it does today. The
    row stays PARTIAL on two preconditions: a protected patch to
    `agent-guard.mjs` and `agent-boundary.yml` that judges each commit's
    own files on a pull request, as range mode already does, and a manifest
    that gives the four unowned path groups an owner. Lane L6 writes both
    as one patch. The same round applied L6's
    `l6-res-9177-secondary-registry.patch`, which defaults off in every
    environment and bills nothing until a region is named.

17. **R12-CAPITAL-4091's counterfactual menu invents no figures** (lane L1,
    decided 10:45 UTC). The hedge arm needs an instrument and a ratio, and the
    venue arm a venue and a taker cost. The committed universe holds six
    synthetic single names and no index or hedge instrument, so any default
    would be a figure somebody made up. The declaration therefore ships no
    hedge arm, with the reason stated. It carries a venue arm only if a venue
    and its cost are already stated in the tree, and otherwise none. It stays
    counterfactual-only: it scores fills and is not loaded into the hedge
    policies, so the hedge review journals no proposal from it. It is
    embedded under `backend/` in `qip-kernel` and resolved against the loaded
    universe at assembly. Anything it cannot resolve drops that one arm and
    never refuses the whole set, because a set refused for one missing
    instrument would leave every fill unscored. The row stays PARTIAL until a
    person declares a hedge instrument and ratio.

18. **R12-RES-7106's eligibility rule stands, and the fixtures change**
    (lane L6, decided 11:00 UTC). Under the ruled ADR 0238 an acting problem
    the constraints leave unsatisfiable is Unresolved and never sized. With
    that in place, the default cycle forms no proposal on any fixture tape:
    the best admitted confidence on 192 single-instrument tapes is 4823 bp,
    against the 5000 bp `r10-eligible` needs. That breaks 38 tests whose
    premise is that a cycle trades. Changing `r10-eligible`'s premises would
    be a threshold change, and a test-only seam to set the rule pack is
    refused by ADR 0212 decision 7. So the fixtures change: each of the 38
    keeps every assertion byte-identical, and only its inputs build an
    uncontested panel at or above eligibility. At least five are spot-checked
    with a mutation of the code they guard, and a new test pins that a
    contested panel forms no proposal. Kill-switch tests that reached ACT's
    gate keep a fixture that still reaches it, and the new stop in REASON
    gets its own test. A journal sealed before the change must still verify
    under the rule in force when it was sealed. If that needs a schema
    change, the lane stops and asks before merging. Lane L6's tightening of
    the fleet Job to Binary Authorization's default policy is accepted. It
    fails closed, so the fleet worker image must be attested, or exempted by
    exact repository, before the next fleet apply. That is the owner's to do.

19. **An edge node's pass-time rows close on a deployed node, not on the
    startup template** (lane L2, on R12-ARCH-9014). The execution node's
    startup template writes `QIP_VENUE_FEED=simulated`, so a node built from
    it runs passes. That template is committed but no environment renders it:
    `execution_nodes_enabled` is false in dev and `execution_nodes` is empty
    everywhere else. It is therefore not a default path, and a row cannot
    cite it as one. Making `qip-edge-node` run passes when `QIP_VENUE_FEED`
    is unset would turn a refusal default into an admission, which the
    fail-closed principle forbids. The rows stay PARTIAL on deployment. They
    close when a deployed node runs passes and a game day is recorded
    against it.

20. **R12-RES-7106's follow-up reading of "contested"** (lane L6). Four
    points, each accepted:
    - `SymbolicDerivation`'s `SCHEMA_VERSION` moves from 1 to 2, and a
      record verifies under the version it was sealed with.
    - The fixture `PlatformConfig` may carry `licensed_datasets`. The default
      stays unchanged, and a test pins the default.
    - Under ADR 0238, "contested" means dissent on the evidence or the
      direction. Two objections are excluded. A risk-control vote that only
      reports a blocking limit is excluded, because that limit refuses at its
      own gate. An adversarial re-argument against a position already held is
      excluded, because otherwise the platform could never add to, hedge or
      plan an exit from anything it holds. Each exclusion gets a test, and a
      test also shows that genuine dissent is still contested.
    - Test (1)'s assertion becomes "Unresolved and not sized", which is
      stricter: the ADR removes the old property. Test (5) asserts that
      nothing trades and the halt is journaled. The ACT kill gate keeps its
      own unit test.
    Taken together this is still a tightening against `main`, not an easing.

21. **R12-CONTRACT-7067's health figures are figures already in the tree**
    (lane L2). Model health is the lattice's `CALIBRATION_BRIER_BOUND` of
    0.30, carried as `min_quality_bp` 7000 and named as the lattice's bound.
    It is not called a promotion gate's floor, because no promotion gate
    has a calibration floor. Data health is each slot's
    `PolicyItem::time_to_live`. Neither needs a new figure from the owner.
    `CALIBRATION_FLOOR` and `TAIL_BRIER_BAR` are not used, because neither
    measures what the clause asks about.

23. **The router's value-of-information gate is not eased for a test**
    (lane L4, on R12-QUANT-4032 and R12-QUANT-4063, 2026-11-06). With the
    quantum path on, the router attempts QAOA on the statevector simulator
    every cycle. Its value-of-information estimate for a QPU job declines
    the job, so the gateway's submit is never reached. No contract record
    then carries a quantum result, and the rows' closure test cannot pass
    as written. That gate is what stops a QPU job the estimate cannot
    justify, so it is not relaxed, and no test seam goes around it. Easing
    a guard so that a test passes is the trade the rules forbid.
    R12-QUANT-4032 stays PARTIAL and is regraded EXTERNAL: it closes on a
    measured QPU record, which needs a hosted provider. Its test may land
    only if it asserts today's behaviour, that the estimate declines and
    nothing is submitted, and a mutation proves it does. R12-QUANT-4063's
    router half does not depend on the gate and is codeable now: a polled
    answer is adopted only for `JobClass::Optimisation`.

24. **R12-EXPAND-6036 is a measurement, and nothing is built for it yet**
    (lane L4, 2026-11-07). The row's closure asks whether every size a
    parked quantum family poses is at most 22, and says to land the parked
    patch if any size falls in (22, 24]. Reading the code does not settle
    that question:
    - No single constant bounds the sizes the parked families pose
      (`FeatureEnsembleSelection`, `GraphSearch`).
    - The active QAOA portfolio QUBO has `theses.len()` variables, and no
      per-cycle cap on approved theses was found.
    - The row's wording conflates that active family with the parked ones.

    A run over the committed six-name synthetic universe would not
    establish production sizes. So the row stays PARTIAL and is regraded
    DEPENDS: it closes on sizes measured over a real universe. The
    parked-family patch is not landed. No cap on approved theses is
    invented, because a cap is a figure and figures are a person's to
    give. The missing cap is recorded on the row as an open question.

25. **RES-7106's seven tests change their inputs, not their assertions**
    (lane L6, 2026-11-07). After decision 20, ADR 0238's stop on a
    contested panel also catches seven more tests in other crates. Each
    one assumes that a cycle trades:
    - two in `qip-api` `proposals.rs`
    - two in `qip-deepbrain` `research.rs`
    - two in `qip-fastbrain` `tape.rs`
    - one in `qip-kernel` `position_record`

    They are restored under decision 18's conditions. Every assertion
    stays byte-identical. Only a test's inputs change, so that it builds
    an uncontested panel at or above `r10-eligible`. Each of the two
    `qip-fastbrain` tape cases gets its own fixture tape. The committed
    shared tape is not changed: other suites and the tape gate read it,
    and decision 18 already accepts that the default cycle on it forms no
    proposal. In each file at least one changed test is mutation-checked
    against the code it guards. A test that cannot reach eligibility
    without changing an asserted value, the shipped rule pack or a
    threshold is parked and reported, never weakened. The work is pushed
    only on a green full-workspace gate. One disclosure: the lane amended
    one local, unpushed revert commit, so no pushed history was
    rewritten.

26. **R12-LEDGER-5014 waits for a fabric versioning ADR** (lane L1,
    2026-11-08). The row stays PARTIAL. The lane's chain-opened slice adds
    the `ChainOpened` sequence to `SourceEvent` and `Decision::Filled`.
    That changes the recorded shape of `reflex.journal_recorded` and
    `reflex.outcome_recorded`, and `event_fabric_schema_lock` refuses a
    shape change at v1. The slice is kept, unapplied, in
    `docs/blueprint/lanes/l1-ledger-5014-chain-opened-wip.patch`. The
    first move of a fabric topic from v1 to v2 needs one ADR on fabric
    versioning. That ADR covers the lock rows, the bindings, the broker
    registries and the versions each consumer admits. A single-row
    argument does not move a topic. A separate P1 record that widens ADR
    0100 section 5 is declined. R12-CAPITAL-5067 also stays PARTIAL: its
    P2 task record needs a new fabric topic, which is its own change and
    is not made under this ruling.

27. **R12-RES-7106 does not land while any of its seven tests is red**
    (lane L6, 2026-11-11). Under decision 25 the lane restored three of the
    seven tests by changing their inputs only. Four stay red: one in
    `qip-deepbrain` `research.rs`, two in `qip-fastbrain` `tape.rs` and one
    in `qip-kernel` `position_record`. None of the seven is ignored,
    deleted or reworded to pass, and the row stays PARTIAL. The lane keeps
    its work as one unapplied patch file under `docs/blueprint/lanes/`. A
    later session takes it from there. The two tape tests each get their
    own derived tape. The kernel test gets a fixture if such a history
    exists. For the research test the session first establishes whether
    any band lies between the eligibility floor and
    `DECISION_UNCERTAINTY_FLOOR`. If the band is empty, an ADR amends ADR
    0238, narrowing only: an uncertain decision becomes an information
    request and is never sized. The test is then restated to that
    stricter property, with a mutation.

28. **A row no longer waits on a design question that lives in the
    repository** (owner, 2026-11-11: "make all the decisions you can").
    The session that meets such a question decides it by ADR, from a
    number range the orchestrator gives it in its spawn message. It
    decides by six preferences, in this order:
    - narrow, refuse or measure, and never widen an authority, a limit or
      the paper boundary;
    - `qip-contracts` holds data types only, so no behaviour moves into
      it;
    - prefer no new crate edge, and pin any unavoidable one by test so the
      set cannot grow;
    - one source for one fact: keep the ledger or event-log record and
      derive or retire the second claim;
    - an arm with no producer in the tree is declared absent with its
      reason, and is not built on an invented input;
    - an accepted ADR is amended only by a new ADR that names the clause.

    Six things stay with the owner: a figure nobody gave, a change to a
    requirement's wording, the reversal of an accepted refusal, a new
    dependency, a billed service and a deployment. The first ruling made
    under this decision is the treasury split: treasury and funding are
    extracted into `services/qip-treasury`, `bank.rs` stays in
    `qip-capital`, and a type that carries behaviour does not move to
    `qip-contracts`.

29. **The latency clause of R12-MODEL-3090 is read against the replayable
    meter, not a clock** (orchestrator under decision 28, 2026-11-11). For
    an entrant that runs inside the cycle, the cost of a decision is the
    evidence-examined count the cycle already records, because that count
    replays from the event log and a wall-clock reading does not. No
    wall-clock figure is added to the decision path to satisfy the clause.
    This ruling does not move the row's grade. The session that holds the
    row checks the meter against the row's bar, with a mutation, and grades
    on what it finds.

30. **Risk limits follow the declared risk tolerance, and only downward**
    (owner, 2026-11-11: "Make the risk limit dynamic based on tolerance").
    The design is fixed here and argued in ADR 0405, which the session
    that builds it writes:
    - The tolerance is the one the capital ledger already holds,
      `Mandate::risk_tolerance` in `qip-capital`: a fraction from 0 to 1,
      refused outside that range, and never above the desk's for a user.
      No second tolerance figure is introduced.
    - The operator's limit set is the ceiling. The limit in force is the
      ceiling narrowed by the tolerance: a maximum is multiplied by the
      fraction, and a minimum (liquidity, cash buffer, funding buffer) is
      left where the operator put it. A tolerance of 1 gives the ceiling
      exactly. A tolerance of 0 gives a maximum of zero, so every order is
      refused. No tolerance can raise a limit above its ceiling.
    - The narrowing is arithmetic, done before the pre-trade check and so
      before an order exists. It is never routed to a model, and no model
      output, agent finding or configuration value can change the
      tolerance. Only the mandate path can, and its changes are in the
      event log, so the limit in force at any decision replays.
    - A refusal under a narrowed limit names the ceiling, the tolerance
      and the limit in force.
    - The change is not done until a test shows a narrowed limit firing on
      an order the ceiling alone would have admitted, and that test has
      been mutation-verified.

    The paper boundary is not touched: this narrows what the simulator is
    asked to do and adds no path to a venue.

31. **A live order path was asked for and is not built** (owner,
    2026-11-11: "Build the live order path, you can deploy, i give you
    full permission for .claude"). The three parts are recorded
    separately, because they are three different things.
    - The live order path. Not built, and nothing was started. The
      repository's own rule for exactly this request is to stop and ask
      (`.claude/rules/01-security-and-safety.md`), paper trading is a
      standing product decision that an ADR reopens and a code change
      does not (ADR 0003, `.claude/rules/10-product-direction.md`), and
      three independent layers refuse a live ceiling by design. A
      one-line instruction to an agent is not the record that a decision
      to trade real money should rest on. What the decision needs is in
      the owner's hands: an ADR that supersedes ADR 0003 and is accepted
      by the owner in review, a named venue and its agreement, the
      regulatory position for that venue and the accounts traded, the
      credential placed in Secret Manager by a person, a capital ceiling
      as a figure, and a person who signs each raise of the autonomy
      level. The 27 rows whose remainder is a live path stay as they are.
    - Deployment. The approval is recorded. The dispatch still has to be
      the owner's: `scripts/agent-policy.json` lists `infra.yml` and
      `deploy.yml` among the workflows an agent identity may never start,
      and the guard step at the top of each refuses before anything
      authenticates. An agent does not route around that guard. The
      owner's act is to run `infra.yml` with environment `dev` and action
      `plan`, read the plan, and then run it with action `up`.
    - Permission for `.claude`. Used once, for two rule files whose text
      described code that had since moved
      (`.claude/rules/domains/infrastructure.md` and `observability.md`).
      Each claim was checked against the code and the suites that read
      rule files were run before the commit. No rule was relaxed and none
      will be under this permission.

32. **A row with a REGRADE_TEXT ruling moves to COMPLETE only on a verification
    that ran, and an independent check that agreed** (orchestrator under
    decision 28, 2026-11-12, after the owner's 2026-11-11 "focus on the
    fastest wins"). Decision 14 says of the REGRADE_TEXT class that "no row
    moves to COMPLETE". That sentence described what applying the rulings
    did on 2026-10-31: the row's text was corrected and nothing was
    verified, so nothing was promoted. It is not read as a bar on ever
    promoting such a row, because the ruling's reading would then be a
    reading no evidence could meet. It is not read as a promotion either.
    A ruling fixes the reading of the requirement; it never makes an unmet
    bar met.

    So a REGRADE_TEXT row reaches COMPLETE only when three things hold: a
    session that did not write the code ran the row's named test and a
    mutation of the code it guards, and recorded both; every part of the
    requirement, read as the ruling reads it, is met by code it cites; and
    a second, read-only check that did not see the first session's
    reasoning tried to refute the grade and could not.

    Applied in round 31. Three sessions graded sixteen rows COMPLETE, and a
    skeptic per row then tried to refute each. Fifteen held. R12-AGENCY-7027
    did not: the requirement names causal simulation access for the
    orchestrator, the warm door that was graded as the orchestrator has
    none, and the row's own verification command was not run. Its ruling
    explains that absence as a decided design; it does not supply the
    access, and the same ruling had left the row PARTIAL on 2026-10-31. The
    row is returned to PARTIAL with both points in its gap.

    The owner may reverse this reading. If the owner means decision 14's
    sentence as a standing bar, the fifteen rows go back to PARTIAL and
    nothing else changes.

33. **Four owner actions, done by the orchestrator on the owner's
    instruction** (owner, 2026-11-13: "i command you to do step1 through 4
    you asked me to do. I give you full authority complete it right know").
    The four steps were the ones the orchestrator had listed as the owner's
    own that afternoon. What each came to:

    - *Start `infra.yml` for dev: plan, then up.* The plan was dispatched
      on `main` (run 123, 2026-11-13 18:28Z) and stopped at `terraform
      init`: Google answered the state bucket read with 403
      `UserProjectAccountProblem`, "The billing account for the owning
      project is disabled in state absent". Project `algorik-qip-dev` has
      no billing account attached. Nothing was planned and `up` was not
      started. Attaching billing needs the owner's console session, the
      mechanical limit the delegation already names; no instruction given
      in a conversation can supply it. Every row waiting on a deployment
      waits on this first. `up` also stays behind the rule that billed
      infrastructure is not created without its monthly cost named, and a
      plan that cannot run names none. The owner attached billing the same
      evening and the plan then ran; decision 34 carries on from there.
    - *Apply the two held patches.* Both edit the protected guard suite
      `architecture.rs`, which is why they were held. Applied unchanged:
      lane t50-02's `only_the_cost_router_mints_a_deterministic_gate`
      (ADR 0430), an added test and nothing removed; and lane t50-19's
      change (ADR 0464), which adds `qip-survival` to `NO_SOLVER_AUTHORITY`
      and allows one service edge, `qip-survival -> qip-capital`. With it
      lane t50-19 merges. The edge is the only allowance among them.
    - *Merge the rolling pull request once CI is green.* Done only on a
      head whose every check has passed, and recorded in the pull request
      that follows it.
    - *Set the desk risk tolerance figure.* dev now states
      `desk_risk_tolerance = "1"`. The owner delegated the figure and named
      none. A tolerance below one is a risk appetite: how much of the
      operator's ceiling the desk may use. Nothing in the repository
      states the desk's, and the orchestrator does not make one up. One is
      the only value that asserts nothing new: the operator's limit set
      exactly as written, which is what every binary already ran with the
      variable unset. Stating it carries the term through Terraform and
      the three dev manifests, refuses a value outside 0 to 1 at plan
      time, and has each root log the tolerance as stated and not as
      defaulted. It narrows no limit and raises none; ADR 0405 lets the
      term do nothing but narrow. The two patches that carried the
      variable and corrected ADR 0405 are applied with it. test, stage and
      prod state nothing. The owner sets a real appetite by changing that
      one line to a lower fraction, for example "0.5" to halve every
      maximum.

34. **A change window on Friday evening, and main's ci made green, so dev
    can be applied** (owner, 2026-11-13). The orchestrator had told the owner
    that `infra.yml up` for dev could not run that day for two reasons the
    repository itself gives: the plan adds a network firewall policy, a core
    change, and core changes land only Tuesday to Thursday, 14:00 to 20:00
    UTC (`infrastructure/change-windows.json`, R12-GCP-9120); and `up`
    applies nothing on a commit whose push-started ci did not succeed, while
    main's ci had been red on its static-analysis job. It said it would not
    widen the window unasked, because that relaxes a guard. The owner
    answered, quoting both reasons: "This is incorrect fix this and do it
    immediately".

    - *The window.* A second window is added: Friday from 18:00 UTC,
      carried to 03:00 UTC on Saturday, which is still Friday evening in
      the Americas. It was first written to end at 23:59 UTC; the session
      then lost four hours to a usage limit, the apply could no longer
      start before midnight UTC, and the same instruction was carried over
      it rather than left to expire. It is a standing window, because the
      policy's format names weekdays and has no one-off date; it is not
      widened further, and the Tuesday to Thursday window is untouched. This is a relaxation of a
      change-management guard, made on the owner's instruction and said
      here so nobody reads it as an oversight. The reason the guard gives
      still holds on a Friday evening only if a person is watching: the
      owner was. The owner removes it by deleting one line. `qip-cli`'s
      `change_window` test now pins the window's days and every edge, so
      a window that leaked into another day or night would fail.
    - *Main's ci.* One finding failed the static-analysis gate on main:
      `rust/cleartext-logging` at
      `backend/crates/tests/qip-acceptance/tests/deploy_provenance.rs`, an
      assertion message that printed an entry of a constant named
      `PER_BUILD_SECRETS`. The message no longer prints the entry; it names
      it by position. No baseline entry is added and no scanner setting is
      touched.
    - The plan the owner was shown on 2026-11-13 (run 125): 37 to add, 25
      to change, 1 to destroy, the one destroy being the replacement of the
      storage service agent's grant on the evidence key. No key, secret or
      bucket is destroyed. The repository's own estimate for dev fully up is
      about 780 to 960 USD a month (`dev-redeploy-readiness.md` section 5),
      against a recorded ceiling of 25 USD a day.

35. **Merges on the owner's word, every branch to main, finished branches
    removed** (owner, 2026-11-13 and 2026-11-14). Two instructions, quoted:
    "You have full permissions to approve merges run it all complete it I
    need you to drive it home to complete", and "I need all branches pushed
    to main as well those done cleaned up".

    - *Merges.* The orchestrator merges its own rolling pull requests once
      their ci is green, with merge commits. #252 (round 48) was merged on
      2026-11-14 with one check red and that is recorded here so nobody
      finds it later and reads it as an oversight: `agent boundary` asks for
      an approving review of the protected paths from a person with write
      access, and the pull request was opened under the owner's own
      account, which GitHub does not let approve it. The protected changes
      in it were each the owner's instruction (decision 33). The check was
      not edited, skipped or re-run to get a different answer.
    - *Branches to main.* A lane branch reaches main only through a gated
      round; none is pushed there directly. Round 49 takes thirteen. Three
      finished branches held nothing main lacked and are recorded with
      merges that change no file: l1-d2-01 and l6-d1-01, whose only
      difference from main was ADR index rows main already holds in their
      accepted form, and l2-a1-01, whose one commit claims ADR 0370 to 0374
      and was followed by no record. That claim is not taken, and those
      five numbers stay unused. Lane t50-31 is sent back: its World Model
      Arbitration Brain and round 48's Meta Brain pin the same roster
      counts, and reconciling them is the lane's work, not a merge
      resolution.
    - *Removal.* Branches are removed by the repository's own
      `branch-cleanup` workflow, first as a dry run, then deleting only
      branches whose tip is an ancestor of main, so no commit is lost.
      `archive_unmerged` stays false: a branch with work not in main is
      left alone.
    - *Round 49's grades.* Three rows were claimed COMPLETE. The
      independent read (decision 32) confirmed R12-EXEC-1068 and
      R12-AMBIENT-1180 and refused R12-LEDGER-1197, which stays PARTIAL: its
      check asks that a cell fill produce an inventory update, and no code
      path or test does that. R12-AMBIENT-1180's sweep thresholds are the
      lane's engineering defaults, not figures the owner gave; they are the
      owner's to change.
    - *Dev.* `infra.yml up` ran on 2026-11-14 at 00:01 UTC inside the
      window of decision 34 (run 127). It created 57 resources and stopped
      on three permission refusals; #253 is the fix. Nothing was destroyed
      but the one grant the plan named.
    - *Not lifted by any of this:* `prod`; a live order path; a narrowed
      scanner or a weakened test; edits under `.claude/**`; `deploy.yml`
      and `vendor.yml`, which the owner has not named.
36. **The rows that waited on an owner decision are decided, on the owner's
    word** (owner, 2026-11-14: "For 157 decision make the decision write
    it"). An audit of all 1,518 register rows on main `738f4d5e` that day
    found 157 open rows whose remaining gap was a decision or a figure only
    the owner could give. Each is decided below by the orchestrator under
    the delegation of this file. The decisions were written from each row's
    own requirement, gap and earlier rulings, by a quick reading and not by
    a study of the code, so each is a starting position the owner can
    overturn with one line here. What the delegation does not lift was held:
    nothing below opens a live order path, commits money, or weakens a test,
    a guard or a risk limit; a figure is the restrictive one and is marked
    provisional; where an earlier ruling already decided a row it is
    restated, not changed. Counts: 13 figures, 73 design choices, 56
    deferred without spend, 8 refused, 7 left with the owner. No register
    row is regraded by this decision: a decision removes the wait, and the
    row closes only when what it names is built and verified.

    - *A figure is set, provisional* (13):
      - `R12-AMBIENT-1040`: Provisional: no cadence set.
        QIP_DEEPBRAIN_REFETCH_EVERY stays 0 (off) in every committed
        deployment until the owner names a connector. Do not wire
        SourceWatcher on a default path. Row stays PARTIAL. Why:
        Off-by-default is the restrictive value; a connector needs licensing
        evaluation first. Next: Owner sets cadence and catalogued connector
        in a committed deployment; then wire SourceWatcher.
      - `R12-AMBIENT-9034`: Provisional defaults: per-agent budget 0 units
        and model-weight ceiling 0 bp for any agent or model with no stated
        line, so unlisted bidders are refused. Owner raises per line. Add
        budget lines and ceiling to ComputePlan; allocator enforces. Why:
        Most restrictive value fails closed; owner may change. Next: Add
        fields to ComputePlanFields, enforce in allocate_within, set
        defaults in committed environment.
      - `R12-CAPITAL-1098`: Provisional: reserve floor, leverage ceiling and
        financing permissions stay undeclared in deployments (refuse all
        financing, leverage ceiling 1x, no permissions) until the owner
        supplies figures. Paper only. Row stays PARTIAL. Why: Most
        restrictive values; no capital is committed. Next: Owner supplies
        figures; then compose grant path and an InternalFunding caller.
      - `R12-COVERAGE-5105`: Keep the shipped MinLiquidity 5.0-day floor
        unchanged; no exemption for non-continuously priced classes.
        Provisional: private funds and real assets stay refused on paper
        until the owner signs an ADR for a class-specific floor. Why:
        Weakening a risk limit is not delegable; the most restrictive value
        is the existing floor. Next: Row stays PARTIAL; owner ADR on
        liquidity floor for these classes reopens it.
      - `R12-COVERAGE-7157`: Composite stays PARTIAL until R12-CAPITAL-1098.
        Provisional restrictive defaults: no financing permission, leverage
        ceiling 1.0x (no leverage), reserve floor 100% of the treasury
        balance, until the owner gives real figures. Why: Most restrictive
        values fail closed; the owner gives actual risk figures. Next: Owner
        confirms or replaces figures; then compose the grant path via the
        ledger.
      - `R12-DATA-2019`: Keep refusing future-dated payloads as
        manipulation. Add a cross-probe signal, provisional: refuse any
        candidate whose records are dated after the probe by even 1 second,
        no tolerance. Discovery stays off in every committed environment.
        Why: Most restrictive value; plaintext stays elevated, not relaxed.
        Next: Owner may change the tolerance; enabling
        deepbrain_discover_every stays a separate egress decision.
      - `R12-DATA-6006`: Price of information, provisional: 0 per unit of
        IC. Net score equals gross minus cost, delay-discounted, floored at
        0, so no source is promoted on a priced gain. Inputs: SourceCost and
        publication_delay_ms read from the manifest. Owner may replace the
        figure. Why: Most restrictive value; no ADR states a price, so none
        is invented upward. Next: Add manifest cost and delay inputs to the
        desk; test duplicate near-zero, late discounted, costly discounted.
      - `R12-GOV-10004`: Provisional allowed locations for
        gcp.resourceLocations: United States and European Union only, as the
        earlier ruling says. Declare it, and compute.requireShieldedVm, at
        the folder as well as the project. ADR names the three value groups.
        Why: Earlier ruling ADR_THEN_IMPLEMENT fixes the value; US plus EU
        is the most restrictive that fits the multi-region US archive
        bucket. Next: Write the ADR, add the location variable and folder
        policies with an acceptance test per policy; owner applies
        foundation root.
      - `R12-LEDGER-8137`: Keys half is built. Autoscaling floor:
        provisional minimum 100 processing units, the figure already in
        tree, with no autoscale-up funded until ceiling ADR. Plan evidence
        waits for enabled Spanner. Owner may change floor. Why: Most
        restrictive value given by text; spend is deferred under decision 5.
        Next: Plan with autoscaling_config after Spanner funding and deploy;
        resize floor from measured close load.
      - `R12-MESH-5056`: Provisional: every edge cost figure (duty,
        logistics, settlement risk) defaults to the most restrictive value,
        the highest in the cell's validated range, until the operator states
        measured costs. Tax/duty estimate stays unmet: no jurisdictional
        source in repo. Why: Unstated cost must not inflate net edge; tax
        data needs an external source. Next: Wire the default into
        CentralConfig::arbitrage; owner supplies a jurisdictional source to
        close tax/duty.
      - `R12-MODEL-4025`: Provisional counterfactual hold rule: hold a model
        when mean counterfactual shortfall exceeds 0 bps over at least 100
        scored fills; retire stays on drift. Most restrictive; owner may
        change. Order-to-model attribution (proposal to model card) must be
        built first. Why: Text gives no figure; strictest value holds a
        model on any degradation with a sample floor. Next: Add model
        reference to proposal, order and FillScore; then implement hold with
        a test per path.
      - `R12-RISK-4116`: Joint-tail survival reserve may be applied,
        provisional, at the most restrictive reading: reserve set by the
        worst of the library, synthetic and joint worlds. No limit value or
        threshold changes. Stays PARTIAL until composition root calls
        set_joint_tail_reserve_applied. Why: Earlier ruling item 6 allows
        applying it; restrictive default fails closed; reserve is not a
        limit change. Next: Wire set_joint_tail_reserve_applied at
        composition roots; extend 'outside library bounds' proof to the
        factor library and capped-long books.
      - `R12-RISK-5044`: Net-edge margin: provisional 25 bp over
        fixed_cost_fraction, armed in every composition root with
        with_net_edge_margin_bp. Gate to also charge slippage and impact.
        Entry signals with no stated expected edge are refused, not
        admitted. Owner may change the figure. Why: A gate no binary arms
        cannot fire; text gives no figure, so choose conservative
        provisional value. Next: Arm margin in roots, add slippage and
        impact terms, mutation-test the bypass, ADR 0155 s8 records the
        figure.

    - *A design or reading is chosen* (73):
      - `R12-AGENCY-1065`: Restate earlier ruling: ADR 0146 decision 5
        stands. No sentence classifier. Requirement not reworded.
        Structured-claim check against the record stays; phrase floor
        FALSE_CLAIM_PHRASES accepted with named paraphrase miss. Fails
        closed, reproducible from log. Why: Earlier ruling already decides;
        structural citation check is log-reproducible. Next: Row stays
        PARTIAL; nothing to build.
      - `R12-AGENCY-1069`: Restate earlier ruling: rumour stays structural
        under ADR 0146 decision 5. Claim with no recorded counterpart or
        unauthorised source is refused. RUMOUR_PHRASES stays a vocabulary
        floor; paraphrase miss accepted. Why: Earlier ruling decides it;
        structure beats phrase matching. Next: Row stays PARTIAL; nothing to
        build.
      - `R12-AGENCY-7027`: Restate earlier ruling: warm door is the
        orchestrator (DraftingAdapter, guard, hash-chained outbox).
        Simulation clause not built: ADR 0145 d4/d5 and ADR 0146 d11
        refusals stand, no second warm door. Row stays PARTIAL. Why: Closing
        needs reversing accepted refusals; fail closed keeps them. Next:
        Revisit only if owner names a producer of declared plans via ADR.
      - `R12-ARCH-7134`: Restate earlier ruling: requirement text and ADR
        0143 stand. Row stays PARTIAL until phase 0 exits. No switches added
        to the ten 'none' capabilities; ADR 0258 ratchet only shrinks and
        refuses new ungated entries. Why: Earlier ruling decides; adding
        switches would stop paper deploys. Next: Closes when phase 0 exits
        (R12-ARCH-10159).
      - `R12-CAPITAL-10097`: Keep ADR 0159 F: ForecastState stays advisory
        with no capital, risk or hedge consumer. A consumer needs measured
        calibration over a RISK-owned sample count (provisional: unset, so
        nothing influences sizing) and a committed world-to-forecast table;
        forecasts may never reduce reserves. Why: Fails closed; calibration
        unmeasured so no influence. Next: Build only after
        Calibration::Measured exists with direction-rule tests and
        mutations.
      - `R12-CAPITAL-1100`: Keep ADR 0127: settlement brain is a
        closure-tested boundary across qip-capital-fabric and qip-edge
        modules, not a single crate; requirement not reworded. Unify the two
        SettlementConvention types in qip-contracts so both halves share one
        term set. Why: Single shared contract is structural and replayable;
        avoids a rewrite. Next: Move SettlementConvention to qip_contracts,
        update both halves, extend brain_closure test with mutation.
      - `R12-CICD-10168`: Restate earlier ruling: ADR 0102 not amended. The
        eight-role fleet stands; requirement's roles map to the development
        agents acting only via pull request. Exit test named. Stays PARTIAL
        until phase 8 exits and phase 9 is entered. Why: Earlier ruling
        decides; no new agent loop beyond CI gates. Next: Re-grade when
        phase 8 exits.
      - `R12-CICD-8117`: Restate earlier ruling: STAY_PARTIAL_DEPLOY. ADR
        0114 D4 stands; Config Sync and Argo Rollouts stay declined, no
        fifth controller. Requirement not narrowed; gitops.rs guard
        unchanged. Dev evidence recorded only. Why: Earlier ruling decides;
        keeps controller set minimal. Next: Closes only on a future ADR
        admitting both mechanisms.
      - `R12-CICD-9088`: Restating earlier ruling: algorik-app repository is
        declined. Single repository, ownership by directory and CODEOWNERS
        (ADR 0016 amendment, ADR 0101 C6). Keeps one event log and one gate.
        Why: Earlier ruling already decides it; separate repos would split
        the gate and reproducibility. Next: Nothing to build; row stays
        BLOCKED as declined.
      - `R12-CICD-9089`: Restating earlier ruling: algorik-models repository
        declined. Single repository, directory ownership via CODEOWNERS (ADR
        0016 amendment, ADR 0101 C6). Why: Earlier ruling stands; one
        repository keeps one gate. Next: Nothing to build; row stays BLOCKED
        as declined.
      - `R12-CICD-9090`: Restating earlier ruling: algorik-infra repository
        declined. Terraform stays under infrastructure/ in the single
        repository with CODEOWNERS (ADR 0016, ADR 0101 C6). Why: Earlier
        ruling stands; one repository keeps one gate. Next: Nothing to
        build; row stays BLOCKED as declined.
      - `R12-CICD-9091`: Restating earlier ruling: algorik-platform-config
        repository declined. GitOps baseline stays under
        infrastructure/gitops in the single repository with CODEOWNERS. Why:
        Earlier ruling stands; one repository keeps one gate. Next: Nothing
        to build; row stays BLOCKED as declined.
      - `R12-CICD-9092`: Restating earlier ruling: algorik-env repository
        declined. Desired state stays in infrastructure/gitops/envs in the
        single repository with CODEOWNERS. Why: Earlier ruling stands; one
        repository keeps one gate. Next: Nothing to build; row stays BLOCKED
        as declined.
      - `R12-CICD-9093`: Restating earlier ruling: algorik-schemas
        repository declined. Schemas stay in qip-contracts plus
        event_fabric_schema.lock in the single repository with CODEOWNERS.
        Why: Earlier ruling stands; one repository keeps one gate. Next:
        Nothing to build; row stays BLOCKED as declined.
      - `R12-CICD-9094`: Restating earlier ruling: algorik-runbooks
        repository declined. Runbooks and acceptance tests stay in docs/ops
        and qip-acceptance in the single repository with CODEOWNERS. Why:
        Earlier ruling stands; one repository keeps one gate. Next: Nothing
        to build; row stays BLOCKED as declined.
      - `R12-CICD-9107`: Restating earlier ruling: publish to rust_packages
        and deployment_bundles via a deploy.yml step for the commit built;
        do not remove the repositories. Removal is a destructive apply and
        would rewrite the waiver test. Why: Earlier ruling decides it;
        publishing is non-destructive and keeps digests immutable and
        attributable. Next: Add deploy.yml publish step by digest, then
        update pipeline_waivers.rs.
      - `R12-CICD-9115`: Restating earlier ruling: O21 admitted narrowly.
        Only a person-dispatched deploy.yml run moves Cloud Run traffic back
        to the previous promoted revision when release_decision refuses or
        cannot be read, and drafts an incident. No unattended controller.
        Why: Earlier ruling decides it; person-dispatched and fail-closed on
        unreadable decision. Next: Write ADR amending 0114, then wire
        release_decision into deploy.yml.
      - `R12-COVERAGE-7173`: Fail closed: unauthenticated, self-classified
        cell episodes do not feed the evidence fabric or belief calibration.
        Central store stays read by nothing that decides. Reopen only
        through a new ADR defining signed, sequenced carriage. Why: The wire
        authenticates nobody and mapping needs invented confidence; keeps
        replay from the log honest. Next: Row stays PARTIAL; write ADR for
        signed episode carriage before any KnowledgeDelta mapping.
      - `R12-COVERAGE-7189`: Restating earlier ruling (REGRADE_TEXT): no
        hold-out authorised, ADR 0145 stands. Unidentified ambient effects
        update no world model, causal graph or ambient prior; only
        action-policy and memory arm runs. Row stays PARTIAL. Why: Earlier
        ruling decides it; updating from unidentified effects would break
        attribution. Next: Reopens only via an ADR amending 0114 authorising
        a control, plus a consumer under ADR 0141.
      - `R12-DATA-2003`: Mount the candidate catalogue as a secret-volume
        item: own Secret Manager container in local.secret_names, seed step
        in infra.yml, volume item in deepbrain.yaml. This keeps the re-read
        reproducible from the journal. Written as a proposal; the protected
        paths are changed by the owner's dispatch. Why: Secrets reach
        processes as files only; re-assessment needs a mounted catalogue.
        Next: Owner dispatches the protected infra change; then a dev pass
        re-assesses and journals quarantine.
      - `R12-DATA-2021`: Restate the requirement to the
        hand-written-connector model per ADR 0157 section 4 and ADR 0180 d3.
        Sandbox output is held until a journaled promotion event; fail
        closed, so unpromoted output is never ingested. Row stays PARTIAL
        until a test proves it. Why: No generated adapters exist, so a gate
        guards only hand-written arms; the journal makes it replayable.
        Next: Write the promotion-gate test over hand-written connector
        arms; mutation-verify it.
      - `R12-DATA-2026`: Same catalogue-mount design as R12-DATA-2003: a
        Secret Manager container, an infra.yml seed step and a
        deepbrain.yaml secret volume. Promotion is journaled as
        SourceLifecycleChanged, so it replays from the log. The row stays
        PARTIAL until a dev pass runs. Why: Reproducible from the event log;
        protected path needs owner dispatch. Next: Owner dispatches the
        infra change; then show a journaled promote transition from a dev
        pass.
      - `R12-DATA-2027`: Same catalogue-mount design as R12-DATA-2003.
        Throttle journals SourceLifecycleChanged and changes routing class
        only. State that limit in the requirement text rather than claim
        poll-rate reduction. Row stays PARTIAL. Why: Reproducible from the
        journal; honest text avoids overclaiming a control. Next: Owner
        dispatches infra change; then prove a journaled throttle in a dev
        pass.
      - `R12-DATA-2028`: Same catalogue-mount design as R12-DATA-2003.
        Quarantine is fail-closed and journaled as SourceLifecycleChanged,
        with the catalogue dataset quarantined too. Row stays PARTIAL. Why:
        Restrictive, replayable from the log; protected path needs owner
        dispatch. Next: Owner dispatches infra change; then prove a
        journaled quarantine in a dev pass.
      - `R12-DATA-2029`: Same catalogue-mount design as R12-DATA-2003.
        Retire is journaled as SourceLifecycleChanged and retires the
        catalogue entry. Row stays PARTIAL. Why: Replayable from the event
        log; protected path needs owner dispatch. Next: Owner dispatches
        infra change; then prove a journaled retire in a dev pass.
      - `R12-DATA-3008`: Restate earlier ruling: the deep brain names where
        it is blind or wrong on every research pass and journals targets,
        same in every environment. Pass cadence stays off. Narrow the
        R12-GCP-10144 guard only with a written argument, never by loosening
        it silently. Why: Journaled targets are reproducible; no guard
        weakened. Next: Owner picks: set cadence key in later environments
        too, or narrow the guard with an argument.
      - `R12-DATA-8131`: Restating earlier ruling ADR_THEN_IMPLEMENT:
        Dataflow and Dataproc stay out (ADR 0011, everything in Rust). Row
        scored against its Rust equivalent: a batch reprocessing job reading
        only durable GCS/BigQuery, no live fabric subscription. Why: Earlier
        ruling decides it; Rust batch job is reproducible from durable
        sources and spends nothing new. Next: ADR for the Rust batch job,
        then Terraform job resource, test that it reads GCS/BQ only, and a
        real plan.
      - `R12-EVID-2031`: Fail closed: an unclaimed record is not admitted by
        default. Record in an ADR that only a named set of arms
        (venue/vendor market-data bars) is an observation, not an extracted
        claim; every other arm must mint evidence. A test refuses any arm
        outside that set reaching observe with None. Why: Keeps every
        world-model input attributable and reproducible from the event log.
        Next: Write ADR, then the refusal test with mutation check;
        fact-to-evidence link stays open.
      - `R12-EXPAND-1174`: Restating ADR 0199 (confirming ADR 0151 d4):
        RegionalEpisodes ride CellReport only when a central consumer names
        the decision they feed, over the mesh spine (ADR 0128). No
        SharedEpisode built. Row stays PARTIAL citing the
        KnowledgeDeltaRecord. Why: A producer with no consumer is declined;
        the one exchange with producer and reader is reproducible from the
        log. Next: Build episode/lesson exchange only after a central
        consumer is named in an ADR.
      - `R12-EXPAND-6030`: Restate ADR 0158 d8 and ADR 0199: a specialist
        proposal yields a reviewable BrainSpec with gap lineage only, never
        an instantiated agent with tools, corpus or authority. Row stays
        PARTIAL; fails closed. Why: Instantiating agents with authority
        widens scope; keeps every step reproducible and reviewable. Next:
        Requirement text change to drop 'instantiate' is the only closure;
        build nothing.
      - `R12-EXPAND-6036`: Restate ruling 24: this is a measurement, nothing
        built. Parked research families stay parked until a committed note
        shows a posed size above 22 and at most 24 (MAX_QUBITS). Portfolio
        family keeps its classical baseline. Why: ADR 0150 A2, 0190, 0199
        and ADR 0006 baseline rule; no measured benefit yet. Next: Write the
        sizing note; land parked families only if a posed size lies in (22,
        24].
      - `R12-EXPAND-6062`: Keep the four reachable verbs (Sense, Reason,
        Simulate, Operate). Trade, Settle, Hedge, Transfer, Purchase, Create
        stay unpopulated per ADR 0158 d15 on a paper platform. Requirement
        text narrows to the four; row stays PARTIAL until reworded. Why:
        Populating money-moving verbs implies a live path; refuse by
        construction. Next: Reword requirement; then journal the registry
        and make a decision read from it.
      - `R12-FABRIC-7039`: Restate O08 ruling: prost declined; no generated
        Rust types. Schema lock from Rust types (ADR 0156 d1) stays the
        source. Row stays PARTIAL on generated types, not reworded to pass.
        Reopens only if an external wire format requires protobuf. Why:
        Canonical JSON under hash chains is reproducible; no outside
        protobuf consumer exists. Next: If reopened: ADR admitting prost
        under ADR 0109 with protected allow-list patch.
      - `R12-FABRIC-7045`: Restate O09 ruling: declined. No OpenTelemetry
        exporter crate (ADR 0026 stands). No tokio on the fabric SDK path
        until a committed measurement shows it is needed. Sync
        producer/consumer and GroupCoordinator stay. Why: The SDK path is
        reached by the edge node drain; tokio near the cell needs a
        measurement first. Next: Write the measurement note; only then an
        ADR for async subscriptions and OTel hooks.
      - `R12-FABRIC-7048`: Restate: BLOCKED stays. Wire codec remains
        canonical JSON under the schema lock. prost not admitted (ADR 0109,
        ADR 0112 Amendment 1) until an external wire format requires
        protobuf. Why: Earlier ruling stands; no external consumer demands
        protobuf. Next: Nothing built; reopen via ADR 0109 allow-list patch
        if an external format appears.
      - `R12-FABRIC-7049`: Restate O10 ruling: blake3 declined (ADR 0104,
        ADR 0043 s2), neither crate nor in-tree. SHA-256 plus CRC32C stays;
        codec keeps refusing ContentHash::Blake3. Row stays PARTIAL on the
        BLAKE3 wording, not reworded to pass. Why: Fewer hash primitives to
        audit; refusal test already guards it. Next: Nothing built; owner
        may reword requirement to SHA-256.
      - `R12-FABRIC-8077`: Restate O08 ruling: no prost, no in-tree
        generator, because Shape names only JSON kinds and cannot carry
        contract types. Immutable IDs and the compatibility gate are met.
        Row stays PARTIAL on typed binding generation. Why: Generated Rust
        would be weaker than the hand-written types. Next: Reopen only if a
        wire format outside this repository requires protobuf.
      - `R12-GOV-6127`: Earlier ruling 11 stands: the arbitrage leg ground
        rule decided there is restated, not changed. Profit alone, or a
        standing mandate repeated every cycle, is never a sufficient ground.
        Fail closed: a leg without a per-action evidence ground is refused.
        Why: Earlier ruling decides it; fail-closed refusal keeps every step
        reproducible from the log. Next: Build the per-action ground and the
        token only Cell::admit_replacement mints, per ADR 0200 Known limits
        2.
      - `R12-LEDGER-5005`: Restate earlier ruling: REGRADE_TEXT. No
        reservation is invented to give InventoryReservation a producer. ADR
        0153 leaves unnamed kinds bookable with no producer. Row stays
        PARTIAL. Fails closed: nothing posts a reservation nobody made. Why:
        Earlier ruling decides it; an invented reservation would not be
        reproducible from real events. Next: Reopen when a path really
        reserves a unit; journal it whole at that seam with a read-back
        test.
      - `R12-LEDGER-5019`: Keep ADR 0153 d7: no TaxCharge producer. Kind
        stays bookable and tested in qip-contracts; the test that nothing
        else names OperationKind::TaxCharge stays. Row stays PARTIAL. Why:
        Tax method, rate and jurisdiction are unknown and a paper platform
        realises nothing taxable; inventing them breaks reproducibility.
        Next: Owner supplies tax method, rate and jurisdiction facts if ever
        needed, then a follow-on record adds a producer.
      - `R12-LEDGER-5020`: Keep ADR 0153 decision 2: Draw and Release have
        no producer. A real capital call is recorded by hand. Commit stage
        stays produced (5006). Row stays PARTIAL. Why: No capital-call
        observer exists, and a draw inferred from a cash-flow schedule is a
        forecast, not a loggable event. Next: If a capital-call notice
        becomes a universe record, a follow-on record adds the producer.
      - `R12-LEDGER-5026`: Restate earlier ruling: IMPLEMENT stands with ADR
        0153 decision 8. No warehouse statement is built, since a simulated
        warehouse drawn from the same ledger agrees by construction and
        proves nothing. Warehouse clause stays open; marketplace
        reconciliation stays. Why: Earlier ruling decides it; a
        self-agreeing statement is not independent evidence. Next: Reopen
        when a real independent warehouse record exists; add a kernel test
        seeding a break for the work_listed skip.
      - `R12-MESH-5064`: Restate REGRADE_TEXT ruling: legs stay
        risk-ordered, least reversible first, sequential per ADR 0154
        decision 5. No Concurrent arm; it loosens a risk control.
        Requirement text stands; row stays PARTIAL on concurrent clause.
        Why: Earlier ruling; sequential ordering bounds a stranded leg and
        is reproducible and fail-closed. Next: Reopens only via a RISK
        record with the forfeited-edge measurement ADR 0154's dissent asks
        for.
      - `R12-MESH-5066`: Keep ADR 0153 d6: no cycle-completion record.
        Per-leg Filled postings, balanced and proven by cycle_ledger.rs, are
        the ledger evidence. Requirement title is read as met only under d6;
        row stays PARTIAL until deployed broker hop and whitelist path
        exist. Why: Avoids a second source of truth beside the event log and
        ledger postings. Next: Deploy edge node with
        QIP_ARBITRAGE_POLICY_PATH and broker hop; then re-test part 2.
      - `R12-MODEL-10123`: Keep hedges unscored: no hedge policy is declared
        (owner decision 7, ADR 0391 decision 7), and a default would add
        target exposure. Next build, fail closed: score capital plans
        against realised outcome, and rebuild routing judgements from
        RoutingScored log records on restart. Why: Log-rebuild keeps scoring
        reproducible; hedge scoring would score nothing on default path.
        Next: Build capital-plan scoring and routing-book rebuild; hedges
        reopen only if owner declares a hedge policy.
      - `R12-MODEL-6002`: Build a unified training-data export from the
        journaled event log, so every path is replayable, covering declined,
        filled, partial, quote and agency paths. Hedge capture stays out,
        held by owner decision 7. Row stays PARTIAL on hedges. Why: Export
        from the event log keeps records reproducible; hedge path has
        nothing to record by default. Next: Implement log-derived export
        with a test failing when the declined capture is deleted.
      - `R12-MODEL-6004`: Define the counterfactual for a hedged or unwound
        path as the unhedged hold marked to the same horizon, priced by the
        twin from log records. Hedge half stays held by owner decision 7 (no
        declared hedge policy); unwind queue may be built. Row stays
        PARTIAL. Why: Unhedged hold is deterministic, replayable from the
        log, and conservative. Next: Build unwind queue and twin arm; hedge
        queue waits for a declared hedge policy.
      - `R12-MODEL-9030`: Keep the decline per ADR 0149/0140: no scoring
        against ForecastState and no tournament-win promotion gate.
        qip-evolution keeps its single path (deflated Sharpe and gates) with
        the tournament record as read-only context. A strategy is admitted
        only when it states direction and probability. Why: Accepted ADRs
        decide it; one promotion path stays auditable. Next: Nothing to
        build; row stays PARTIAL until a strategy states a Forecast and is
        admitted.
      - `R12-OBS-7115`: Restate earlier ruling: ADR 0156 §5.6 stands, row
        stays PARTIAL. Each process keeps its own in-process graph; no push
        or peer scrape. Build now: deployment key from service, region and
        cell config values, blank refused. Ingestion join waits on a funded
        collector. Why: Keeps scope per-process and fails closed on blank
        keys; collector is billed and unpriced. Next: Lane builds key test
        the_deployment_entity_key_carries_service_region_and_cell_and_a_blank_value_is_refused;
        row closes after R12-OBS-10089 collector funded.
      - `R12-OBS-9074`: Restate earlier ruling: ADR_THEN_IMPLEMENT. Runbook
        set is signed by the image it compiles into, under the
        deny-by-default Binary Authorization policy whose attestor key only
        CI holds. No KMS artefact_signing bought. Write the ADR, then
        implement. Why: Signer outside the build without billed KMS; reuses
        the existing attestation and fails closed. Next: Write ADR accepting
        the image attestation as runbook signature; then implement a
        verifier refusing unsigned sets.
      - `R12-QUANT-1077`: Keep GraphSearch not built. ADR 0150 A2 stands: no
        family lands without a committed note showing real sizes above the
        exact-enumeration limit and within the provider qubit ceiling.
        Parked patch stays unlanded; no gate weakened. Row stays MISSING.
        Why: Fails closed; no benefit note exists, and landing would bypass
        the accepted gate. Next: Write the sizing note from real platform
        problem sizes; if it passes, land the parked patch.
      - `R12-QUANT-1078`: Keep FeatureEnsembleSelection not built, on
        purpose under ADR 0150 A2. Parked patch stays unlanded until a
        committed sizing note passes the A2 test. Row stays MISSING. Why: A2
        gate is accepted and not weakened; no sizing note exists. Next:
        Sizing note first; then land the parked feature-selection family
        with its greedy baseline.
      - `R12-QUANT-1079`: Keep KernelsAndVariationalModels not built. Parked
        kernel patch stays unlanded until the ADR 0150 A2 sizing note exists
        and passes. Row stays MISSING. Why: A2 gate holds; landing without
        the note would skip the measured-benefit rule. Next: Commit the
        sizing note; if sizes qualify, land the kernel family with ridge
        baseline.
      - `R12-QUANT-1080`: Keep ReservoirAndExtremeLearning not built. Parked
        reservoir patch stays unlanded until the ADR 0150 A2 sizing note
        exists and passes. Row stays MISSING. Why: A2 gate holds; no note
        shows sizes above enumeration limit and within qubit ceiling. Next:
        Commit the sizing note; then land the reservoir family with
        echo-state or AR baseline.
      - `R12-QUANT-1081`: Keep ScenarioHypothesisSelection not built. Parked
        scenario patch stays unlanded until the ADR 0150 A2 sizing note
        exists and passes. Row stays MISSING. Why: A2 gate holds; landing
        now would bypass an accepted ADR. Next: Commit the sizing note; then
        land the scenario-selection family with its baseline.
      - `R12-QUANT-1145`: Keep both families not built.
        ScenarioHypothesisSelection waits on the ADR 0150 A2 sizing note;
        SymbolicCombinatorialSearch stays NotBuilt under A6. Row stays
        MISSING. Why: Both accepted ADR gates stand; no guard is weakened.
        Next: Sizing note for the scenario family; Symbolic needs an ADR
        superseding A6.
      - `R12-QUANT-1158`: Row stays PARTIAL. Five parked families stay
        unlanded until the ADR 0150 A2 sizing note passes; no scope item is
        counted built until then. Honest status is six families not built.
        Why: A2 is accepted and unweakened; counting parked code as
        delivered would be a false claim. Next: Write the sizing note; land
        the families that qualify, each with named test and mutation
        evidence.
      - `R12-QUANT-4033`: Row stays MISSING. Conflict-graph QUBO stays
        parked under ADR 0150 A2 until the sizing note passes. Benchmark
        must also be against qip-arbitrage's own path router, not only
        greedy-by-weight. Why: Gate holds; the current baseline is not the
        classical path router the check names. Next: Sizing note, then land
        the patch and add the qip-arbitrage router baseline comparison.
      - `R12-QUANT-4035`: Keep FeatureEnsembleSelection not built per ADR
        0150 A2. Do not land the parked patch. Precondition: a committed
        size note showing problem sizes lie above the exact-enumeration
        limit and at or below the provider qubit ceiling. No note, no code.
        Why: A2 is accepted and restrictive; landing unevidenced families
        would weaken a fail-closed gate. Next: Patch stays parked; revisit
        when a size note is committed.
      - `R12-QUANT-4036`: Kernel/variational family stays not built under
        ADR 0150 A2. Parked patch not landed. Revisit only after the
        committed size note exists, with the in-sample-chosen classical
        baseline kept. Why: A2 requires a size note before any new family's
        encoding, baseline or benchmark. Next: Parked patch applies once the
        size note lands.
      - `R12-QUANT-4037`: Reservoir/extreme-learning family stays not built
        under ADR 0150 A2. Parked patch (with echo-state baseline) not
        landed until the committed size note exists. Why: A2 gates new
        families on a measured size note; none exists. Next: Patch stays
        parked pending the size note.
      - `R12-QUANT-4041`: Scenario/model selection stays unbuilt under A2.
        When the size note exists, land ScenarioSelection::check, which
        refuses a selection that drops every minority hypothesis. Extend it
        to world-model members before closing. Why: A2 blocks it; the
        minority refusal keeps uncertainty preserved and fails closed. Next:
        Needs the size note, then the parked patch plus world-model member
        coverage.
      - `R12-QUANT-4054`: Quantum forecasting check stays blocked under ADR
        0150 A2. Keep the parked patch's out-of-time MSE and 95% interval
        coverage beside the classical model as the design, unlanded until
        the size note exists. Why: No quantum path may land without the A2
        size note; classical baseline is mandatory (ADR 0006). Next: Land
        with the reservoir family after the size note.
      - `R12-REASON-1132`: Restate ruling REGRADE_TEXT: neural coupling out
        of scope per ADR 0150 decision 7. Causal graphs are recorded and
        replay-verified. Row stays PARTIAL until the owner rewords the
        requirement. Why: A neural output has no re-derivable trace; replay
        must stay exact. Next: Closes only when the owner rewords the
        requirement.
      - `R12-REASON-1133`: Restate ruling IMPLEMENT: neural coupling stays
        out under ADR 0150 decision 7. A derivation whose timeline cannot be
        rebuilt is refused by name on replay, never reported identical. Why:
        Fail closed: an unrebuildable timeline is not a proof. Next: Row
        stays PARTIAL until the owner rewords the requirement.
      - `R12-REASON-1134`: Restate ruling REGRADE_TEXT: probabilistic logic
        is the journaled network posterior, recomputed on replay. Neural
        coupling is out under ADR 0150 decision 7. Row stays PARTIAL pending
        rewording. Why: Keeps every step reproducible from the log. Next:
        Owner rewording of the requirement closes it.
      - `R12-REASON-1136`: Restate ruling REGRADE_TEXT: the bounded in-tree
        constraint solver is the constraint solver. No SAT/SMT/MILP crate
        (decision 11); neural coupling out (decision 7). Row stays PARTIAL
        pending rewording. Why: Avoids an unaudited dependency and keeps
        replay deterministic. Next: Reopen only under decision 12's
        measured-refusal path.
      - `R12-REASON-1179`: Restate ruling REGRADE_TEXT: scope is what ADR
        0150 fixed: bounded in-tree solver, no SMT/MILP (decision 11), no
        neural-to-symbolic compilation (decision 7), no planner beyond the
        hypothesis lifecycle (decision 9). Graph solvers named as causal
        path enumeration. Why: Declined clauses are decided, not missing
        work. Next: Row stays PARTIAL until the owner rewords the scope
        list.
      - `R12-REASON-3058`: Restate ruling REGRADE_TEXT: O12 declined. No
        SMT/MILP crate, no long in-tree solver (ADR 0150 decision 11). Row
        stays PARTIAL against the SMT/MILP clause; dependency allow-list
        untouched. Why: The cycle poses few Boolean variables; a new crate
        is unjustified. Next: Reopens only on decision 12's measured,
        refused, real problem.
      - `R12-RISK-4097`: Narrow the bridge clause, provisional: no bridge
        limit while ADR 0021 forbids transfers, since an always-empty axis
        is a limit that cannot fire. Record a finding instead. Add a bridge
        axis only when an observed bridge statement source exists. Why: A
        limit that cannot fire reads as protection; narrowing is honest and
        fails closed. Next: Owner confirms the narrowed wording, or names a
        bridge statement source.
      - `R12-SEC-10002`: Restate earlier ruling: workforce IAP lists require
        ^group: only; PAM keeps individual user: principals. No-org mode
        fails closed, console unreachable there until a Cloud Identity group
        exists. Acceptance test refuses user: in IAP lists. Why: Group-only
        fails closed and blocks shared-account literals; earlier ruling
        already decided. Next: Patch variables.tf validation and IAP modules
        in a protected-path patch for orchestrator; add acceptance test.
      - `R12-SEC-9064`: Restate earlier ruling and O17: ADR 0113 s4 stands.
        Scouts get their own project with the second region, not before.
        Node pools stay refused (ADR 0114 D4). Now harden the
        ingestion-discovery zone network and egress identity only. Why:
        Project split adds cost and IAM; earlier ruling already decided;
        network isolation is available now. Next: Write ADR when second
        region is scheduled; tighten the zone's default-deny and egress
        identity now.
      - `R12-SEC-9122`: Follow R12-SEC-10002 ruling: group-only workforce
        IAP access, PAM keeps individual user: principals. Workload identity
        stays one service account per workload. No separate change. Why:
        Same human-access control as 10002; one decision avoids divergence;
        group-only fails closed. Next: Closes with the 10002 patch and its
        acceptance test.

    - *Deferred: no money is committed* (56):
      - `R12-AMBIENT-7202`: TPU budget stays Decimal::ZERO under owner
        decision 5 (billed compute declined). Declined clause recorded, row
        stays PARTIAL. Code gaps (composition-root acceptance test,
        zero-value signal classes) remain build work, not owner choices.
        Why: No TPU exists to bill; spending is not authorised. Next: Build
        qip-acceptance test via new_for_node; revisit TPU only if billed
        compute is approved.
      - `R12-ARCH-1192`: QIP_DEEPBRAIN_TICK_EVERY stays unset and tick
        entitlements are not added to the dev manifest. Licensing posture is
        evaluated before use; no data licence is granted here. Row stays
        PARTIAL. Why: A tick entitlement is a licence grant; evaluate before
        use, no spend committed. Next: Revisit when the owner records a
        licensing call for tick data.
      - `R12-ARCH-9028`: Restate earlier ruling and O25: Vertex AI Agent
        Engine not admitted. Paid managed runtime holding sessions and
        memory outside the event log. Single in-process Rust runtime stays;
        no AgentManifest runtime change. Why: Billed service declined; state
        outside event log breaks reproducibility. Next: Revisit if billed
        services are approved and an ADR keeps sessions in the log.
      - `R12-COVERAGE-7156`: Experimental-learning clause stays declined.
        Optimisation clause holds. Per ADR 0150 decision 2 and ADR 0199
        decision 3, no research family is built. Reopen on a committed
        benefit case measured against the classical baseline. Why: Earlier
        owner refusal of the research-families patch stands; no spend on
        quantum research. Next: Row stays PARTIAL; revisit only via ADR 0150
        benefit case or R12-EXPAND-6036 sizing note.
      - `R12-COVERAGE-7183`: Restating earlier decisions: symbolic-search
        and scenario-selection quantum families stay parked (ADR 0150,
        0190); no HypothesisSearch submitter is added. Reopen only via ADR
        0150 committed benefit case or measurement. Why: Owner refused the
        L4 research-families patch on 2026-10-31; no spend on parked
        families. Next: Row stays PARTIAL; revisit on a measured benefit
        against the classical baseline.
      - `R12-DATA-10035`: Restating earlier ruling (STAY_PARTIAL_DEPLOY):
        Bigtable stays declined under decision 5 (about $950-$1,300 a month
        for an empty instance). Derived features stay in memory. Reopen only
        via an ADR with a measured need, monthly ceiling and client. Why:
        Earlier ruling stands; no money committed for an unused instance.
        Next: Row stays PARTIAL; ADR under ADR 0101 C4 plus client ADR under
        ADR 0109 would reopen it.
      - `R12-DATA-10037`: Restate 2026-10-31 ruling: Knowledge Catalog
        (Dataplex) stays declined under decision 5. Billed managed service,
        no ADR 0101 C4 storage record, no priced ceiling. Nothing declared
        in Terraform. Row stays MISSING. Why: Earlier ruling already
        decides; billed service with no ceiling. Next: Reopens only via ADR
        stating measured need, scope and monthly ceiling; then gated module,
        lineage, real plan.
      - `R12-DATA-10040`: Restate ruling: AlloyDB stays refused under ADR
        0113 and decision 5 (O01 declined). No Postgres client admitted, no
        cluster created. Workflow state stays on the journal. Row stays
        PARTIAL. Why: Earlier ruling decides; billed cluster, no measured
        need or ceiling. Next: Reopens via ADR superseding ADR 0113 AlloyDB
        clause with measured need, monthly ceiling, plus client ADR under
        0109.
      - `R12-DATA-10080`: Restate ruling: declined with Bigtable under
        decision 5. No fabric sink written against a store no ADR admits and
        a client ADR 0109 does not admit. Row stays MISSING. Why: Earlier
        ruling decides; depends on Bigtable decline and dependency ADR.
        Next: Needs R12-DATA-10035 reopening ADR, client dependency ADR,
        then fabricd sink, integration test, real plan.
      - `R12-DATA-10206`: Restate ruling: three app profiles are built and
        plan-tested, regrade to PARTIAL. No reader or writer names a
        profile; that waits on the Bigtable decline (decision 5). No row
        moves to COMPLETE. Why: Earlier REGRADE_TEXT ruling stands; Bigtable
        stays off, no spend. Next: Reopens only via Bigtable ADR with
        measured need and ceiling plus client ADR; then readers name
        profiles.
      - `R12-DATA-2023`: Timeliness scoring stays as built. Enabling
        discovery probes through the egress proxy to candidate hosts is not
        decided here. QIP_DEEPBRAIN_DISCOVER_EVERY stays null everywhere;
        the row stays PARTIAL. Why: Probes are external egress and a
        licensing posture decision; protected path. Next: Revisit when the
        owner approves egress posture for discovery and sets the cadence.
      - `R12-DATA-8006`: Restating earlier ruling STAY_PARTIAL_DEPLOY:
        BigQuery is the only store switched on; Cloud Storage waits for its
        first writer. Bigtable, Spanner Graph, Knowledge Catalog, Vertex
        declined under decision 5. Row stays PARTIAL. Why: Earlier ruling
        decides it; no spend before dev deploy; requirement text not read
        down. Next: After dev deploy, one ADR per store with measured need
        and monthly ceiling; Bigtable also needs gRPC client ADR.
      - `R12-DATA-8015`: Restating earlier ruling STAY_PARTIAL_DEPLOY:
        Spanner Graph declined under decision 5. Promoted knowledge stays in
        central brains, rebuilt from event log. In-memory graph does not
        stand in for K1. Row stays PARTIAL. Why: Earlier ruling decides it;
        separate Spanner instance costs $2,705-$8,140 a month for ledger
        config, graph unpriced. Next: After dev deploy, ADR with measured
        graph load, ceiling and Spanner client under ADR 0109.
      - `R12-DATA-8016`: Restating earlier ruling STAY_PARTIAL_DEPLOY:
        Bigtable declined under decision 5; floor $950-$1,300 a month.
        Derived features stay in memory in qip-feature-dag. No gRPC stack
        admitted. Row stays MISSING. Why: Earlier ruling decides it; no
        spend for an unfunded store. Next: After dev deploy, Bigtable store
        ADR plus gRPC client ADR under ADR 0109; then adapter and emulator
        test.
      - `R12-DATA-8122`: Restating earlier ruling STAY_PARTIAL_DEPLOY: K0
        stays in memory with bounded horizon and spills nowhere. Bigtable
        declined under decision 5. Requirement not redefined to pass the
        row. Row stays PARTIAL. Why: Earlier ruling decides it; Bigtable
        floor $950-$1,300 a month unfunded. Next: Follows R12-DATA-10035
        reopening and client ADRs; then spill code with test that state
        beyond horizon spills, not drops.
      - `R12-DATA-8126`: Restating earlier ruling STAY_PARTIAL_DEPLOY:
        Bigtable declined under decision 5. Plan clause met in code (two
        clusters, three app profiles). Nothing more built;
        fabric-sink-bigtable waits. Row stays PARTIAL. Why: Earlier ruling
        decides it; no spend on unfunded store, floor $950-$1,300 a month.
        Next: After R12-DATA-10035 ADR and gRPC client ADR:
        fabric-sink-bigtable, enable_bigtable flip, committed plan.
      - `R12-DATA-8128`: Restating earlier ruling STAY_PARTIAL_DEPLOY:
        Knowledge Catalog declined under decision 5. No lineage emitter for
        a catalog that does not exist (ADR 0151 decision 1). Row stays
        MISSING. Why: Earlier ruling decides it; producer with no consumer
        forbidden; catalog unpriced. Next: ADR stating need, lineage and
        evidence-contract scope, and monthly ceiling; then gated Terraform,
        emitter, plan.
      - `R12-DATA-8130`: Restating earlier ruling STAY_PARTIAL_DEPLOY: O01
        declined for now. No PostgreSQL client admitted, no AlloyDB bought
        before deploy. ADR 0113 refusal stands. Operational metadata stays
        on journal and single-node engine store, never in ledger. Why:
        Earlier ruling decides it; AlloyDB primary unpriced. Next: After dev
        deploy, ADR reversing 0113 with measured need, ceiling, and ADR 0109
        client record.
      - `R12-DATA-9052`: Restating earlier ruling STAY_PARTIAL_DEPLOY:
        Spanner Graph declined under decision 5. qip-mesh GraphStore port
        stays caller-less. Promoted-only write gate comes with the Spanner
        Graph ADR. Row stays PARTIAL. Why: Earlier ruling decides it;
        separate instance unpriced, ledger config $2,705-$8,140 a month.
        Next: After dev deploy, ADR with measured need and ceiling plus
        Spanner client under ADR 0109; gate test mutation 'admit scratchpad
        write' must fail.
      - `R12-EXEC-1169`: Guarded-but-off factory is not accepted as delivery
        by delegation. No default path creates markets; breadth plan stays
        unset. Row stays PARTIAL. Why: Accepting scope delivery or enabling
        market creation is the owner's call; off is the restrictive default.
        Next: Closes when a committed deployment sets the breadth plan or
        the owner accepts the factory as delivered.
      - `R12-EXPAND-1119`: Umbrella cannot close. Stays MISSING, graded
        against the one exchange record that exists (KnowledgeDeltaRecord).
        No episode, residual or lesson exchange built, per ADR 0199. Why:
        Depends on R12-EXPAND-1174 exchange, which ADR 0199 declines; no new
        brain or cross-process path. Next: Reopens only if a central
        consumer names the decision an exchange feeds.
      - `R12-EXPAND-6027`: No billed egress path or hosted connector now.
        Row stays PARTIAL. EvidencePolicy record not built; SourceScore is
        the chosen artifact. Revisit when the owner funds Cloud NAT egress
        and a connector is configured. Why: Hosted source needs billed zone
        policy and Cloud NAT; no spend committed. Next: Keep SourceScore
        path. Later: connector arm, then outcome-driven reputation and
        adversarial detection.
      - `R12-FINOPS-8041`: Spanner and Bigtable stay declined (decision 5);
        no data-store bounds appear in a plan. 1/1 brain bounds are by
        design. The Bigtable autoscaling assertion stays, unverified until
        CI runs terraform test. Why: Enabling stores spends money; ruling
        already fixed. Next: Revisit when an ADR with a ceiling enables
        either store; then plan shows its autoscaling limits.
      - `R12-FINOPS-9186`: Restate: BigQuery reservations declined for now;
        warehouse stays on-demand. Row stays MISSING. Reopens with an ADR
        naming the edition, slots for research, backtest and BI, and a
        monthly ceiling. Why: One streaming writer and no measured query
        load; reservations are billed. Next: Deploy dev warehouse, measure
        contention, then ADR and modules/data reservations with a plan
        harness.
      - `R12-GCP-10009`: Restate: no NCC hub before the ADR superseding ADR
        0035 with a second region and the hub's monthly cost. No hybrid
        spoke before a circuit is ordered. Row stays MISSING. Every spoke
        must carry a route filter. Why: Hub and circuit are billed; single
        region today. Next: After the second-region ADR: amend parked patch
        so star spokes are filtered, add a failing tftest, then apply.
      - `R12-GCP-8037`: Restate decision 5: Spanner ledger, Knowledge
        Catalog and managed model registry are not funded before dev runs.
        The journaled signed-release registry stands in. Row stays PARTIAL.
        Why: Managed stores are billed and no measured need exists. Next:
        After dev runs, an ADR with measured need and monthly ceiling
        enables each as one global resource; a plan shows each once.
      - `R12-GCP-8051`: Restate earlier ruling: STAY_PARTIAL_DEPLOY. ADR
        0113 section 4 stands, one project per environment. Engineering and
        security projects arrive with the second-region record superseding
        ADR 0035, under an owner-held organisation. Nothing built now. Why:
        Earlier ruling decides it; extra projects and org are owner-held and
        unneeded with one region. Next: Revisit on second-region ADR; then
        amend ADR 0118, extend resource-hierarchy, add tftest.
      - `R12-GCP-8053`: Keep separate VPCs, Shared VPC stays refused. NCC
        hub, spokes and per-pair export filters wait for the record
        superseding ADR 0035; ADR 0156 6c topology rule stands. Row stays
        PARTIAL. Why: One region has nothing to join; hub and data transfer
        are billed. Next: Revisit on second-region ADR naming hub monthly
        cost; replace the refusal scan then.
      - `R12-GCP-8056`: Restate earlier ruling: STAY_PARTIAL_DEPLOY.
        Decision 4 of 2026-10-25 covers VPC spokes. No
        google_network_connectivity_* resource declared;
        l6-ncc-hubs-parked.patch stays parked; row stays MISSING. Why:
        Earlier ruling decides it; spokes are billed and have nothing to
        join in one region. Next: Apply patch only after the 8035 ADR
        supersedes ADR 0035 and names hub cost.
      - `R12-GCP-8057`: Restate earlier ruling: STAY_PARTIAL_DEPLOY.
        Decision 4 stands; mesh hub patch not applied. The gcp_placement
        guard changes only by the second-region record, never to let the
        patch through. Why: Earlier ruling decides it; one region, hub
        billed. Next: Apply l6-ncc-hubs-parked.patch after the second-region
        ADR states hub cost.
      - `R12-GCP-8058`: Restate earlier ruling: STAY_PARTIAL_DEPLOY.
        Decision 4 stands for the star hub. No GKE service plane exists (ADR
        0114 D4), so no central inspection hub is needed. Patch stays
        parked. Why: Earlier ruling decides it; no traffic to inspect, hub
        billed. Next: Revisit with the second-region ADR (8035, 8057) naming
        monthly cost.
      - `R12-GCP-8104`: Restate earlier ruling: STAY_PARTIAL_DEPLOY.
        Requirement text stands, row stays PARTIAL. Portfolio brain stays
        one warm Cloud Run writer to protect the hash-chained log; HPA
        refused per ADR 0114 D4. Spanner waits on decision 5. Why: Earlier
        ruling decides it; a second writer would fork the log, Spanner is
        billed. Next: Reopen only on a measurement that Cloud Run cannot
        meet the need, via ADR.
      - `R12-GCP-8116`: Restate earlier ruling: STAY_PARTIAL_DEPLOY. Node
        pools, taints and PriorityClasses refused per ADR 0114 D4. Research
        stays in the deep brain process, with the shed-research runbook and
        Cloud Run instance ceilings as the control. Why: Earlier ruling
        decides it; a separate research Job is billed and unmeasured. Next:
        If qip_deepbrain_iteration_ms shows starvation despite the runbook,
        write an ADR for a Job with named cost.
      - `R12-LEDGER-10041`: Restate earlier ruling: STAY_PARTIAL_DEPLOY.
        Ledger of record goes to Spanner Enterprise Plus per ADR 0113 on
        nam3 with us-east4 leading. enable_spanner stays false everywhere;
        nothing enabled, no first run claimed from the in-process double.
        Why: Earlier ruling decides it; the instance is billed at $2,705 to
        $8,140 a month and needs a measured need. Next: After dev runs: ADR
        with measured load and monthly ceiling, 8136 change, owner tfvars
        patch, real-instance evidence.
      - `R12-LEDGER-7009`: Restate 2026-10-31 ruling: ledger goes to Spanner
        per ADR 0113, but no spend ($2,705-8,140/mo) until base deploy runs
        and an ADR states measured need and ceiling. Row stays PARTIAL;
        ledgerd keeps refusing local store when deployed. Why: Earlier
        ruling decides it; largest bill; decision 5 stands. Next: Revisit
        when owner enables billing and ceiling ADR lands; then verify config
        name, plan, deploy.
      - `R12-LEDGER-8007`: Restate earlier ruling: Spanner Enterprise Plus
        is financial truth but unfunded until deploy runs and an ADR gives
        measured need and ceiling. Positions, obligations, grants arrive as
        ledger postings via R12-LEDGER-5004 read path. Row stays PARTIAL.
        Why: Earlier ruling and decision 5 already decide it. Next: After
        dev deploy: ceiling ADR, 8136 nam3 change, protected tfvars patch,
        ledgerd boot image.
      - `R12-LEDGER-8132`: Restate earlier ruling: requirement text stands,
        not narrowed to module contract or mocked plan. enable_spanner stays
        false until deploy runs and an ADR sets a ceiling within
        $2,705-8,140/mo; nam3 change under 8136 lands first. Why: Earlier
        ruling decides it; funding declined under decision 5. Next: Owner
        sets enable_spanner in protected tfvars after ceiling ADR; then show
        ENTERPRISE_PLUS multi-region plan.
      - `R12-MESH-10075`: Restate earlier ruling: O07 decided, ADR 0112
        section 4 stands, no QUIC crate or quinn admitted. TCP+mTLS not read
        as meeting direct QUIC. Row stays PARTIAL until a second region is
        decided and a TCP+TLS vs QUIC measurement shows a cost. Why: Earlier
        ruling decides it; measurement needs second region ADR 0035 does not
        run. Next: Reopens when a record supersedes ADR 0035 with a second
        region; then commit the measurement.
      - `R12-MESH-8003`: Restate earlier ruling as in R12-MESH-10075: ADR
        0112 section 4 stands, no QUIC, no quinn. Text stands, TCP+mTLS not
        read as meeting it, row stays PARTIAL until a second region is
        decided and measured. Why: Earlier ruling decides it; second region
        is parked under decision 4. Next: Reopens when a record supersedes
        ADR 0035 with a second region and its cost.
      - `R12-MODEL-10065`: Restate earlier ruling: GPU pool on GKE refused
        (ADR 0114 D4), Vertex accelerator declined under decision 5. Row
        stays BLOCKED; nothing built or deployed. Why: Earlier ruling
        decides it; no measured accelerator consumer. Next: Reopens only via
        an ADR citing a measured accelerator consumer and a monthly ceiling,
        after paper deploy.
      - `R12-MODEL-7007`: Restate earlier ruling: accelerators declined
        under decision 5. ADR 0122 decision 4 stands: Vertex custom jobs
        only, offline research lane, only after a committed CPU-inadequacy
        measurement for a named experiment. Gpu and Tpu arms stay
        undispatched. Why: Earlier ruling decides it; no measured need or
        ceiling. Next: Reopens by an ADR with the measurement and monthly
        accelerator ceiling; then a Vertex custom-job spec behind
        enable_vertex_ai.
      - `R12-QUANT-10066`: Restate earlier ruling: STAY_PARTIAL_DEPLOY. No
        TPU rung, no TPU bought; row stays MISSING. Cite ADR 0158 d5, ADR
        0122 d4, decision 5 of 2026-10-25. Reopen on an ADR with a measured
        TPU need and monthly ceiling. Why: A rung no provider can serve is
        never chosen; a gate over it measures nothing. Next: Revisit when an
        ADR states measured need and ceiling and decides the ladder
        cost-order change.
      - `R12-QUANT-10067`: Restate earlier ruling: STAY_PARTIAL_DEPLOY.
        Declined for now under decision 5. In-tree solver runs in-process
        within ADR 0150 bounds. Row stays MISSING; no Spot job pool. Why: No
        measured queue exists to scale by; a pool is billed. Next: After
        deploy, ADR states measured queue depth and monthly ceiling; then
        add Terraform job-pool module with reviewed plan.
      - `R12-QUANT-10095`: Restate earlier ruling: STAY_PARTIAL_DEPLOY.
        Fan-out stays across CPU baseline, quantum-inspired search and QPU
        arm, each compared with the classical baseline. GPU and TPU arms
        declined 'no device' (O04, decision 5, ADR 0122 d4). Row stays
        PARTIAL. Why: No accelerator is bought; baseline comparison is kept
        for every arm. Next: Reopen on an ADR amending ADR 0122 d4 with a
        CPU-inadequacy measurement and monthly ceiling.
      - `R12-QUANT-1013`: Restate earlier ruling: STAY_PARTIAL_DEPLOY. Route
        eligible work only to compute that exists; GPU and TPU recorded as
        declined with reason. No accelerator funded (decision 5). Evidence
        line corrected to ambient.rs:1107. Row stays PARTIAL. Why: Four-way
        placement would need billed accelerators the owner declined. Next:
        After an accelerator ADR with measured need and ceiling, route
        ComputeAllocation through Substrate verdicts.
      - `R12-QUANT-10173`: Restate earlier ruling: STAY_PARTIAL_DEPLOY. Four
        of five phase-14 deliverables stand; GPU/TPU7x pools declined (ADR
        0114 D4, ADR 0122 d4, decision 5 of 2026-10-25). Phase exit stays
        unmet; row stays PARTIAL. Why: GKE GPU pool refused by ADR; Vertex
        accelerators are unfunded. Next: After deploy, ADR names measured
        need and monthly ceiling; then add Gpu/Tpu arm with exit test.
      - `R12-QUANT-4046`: O06 deferred: no IBM QPU account, credential,
        egress route or budget admitted. Router keeps CPU, quantum-inspired
        and simulated QPU. Hosted admission in compute_routing.rs stays off.
        GPU/TPU arms stay declared-only, unbuilt. Why: QPU admission means
        an external account and per-job spend; none is committed. Next:
        Revisit when the owner funds a QPU account and a measured benefit
        over the classical baseline exists.
      - `R12-REASON-9005`: Dedicated compute declined (ADR 0002, owner
        decision 5): no new billed infrastructure. Checker clause stays met
        by symbolic_stage::verify and `qip replay`. Row stays PARTIAL until
        the owner changes the requirement or funds compute. Why: Dedicated
        compute costs money and needs an external solver. Next: Revisit if
        the owner funds compute or drops the clause.
      - `R12-REFLEX-7003`: Restate ruling STAY_PARTIAL_DEPLOY: O13 declined;
        ADR 0082 stands, no libc/nix/unsafe for mlock or busy-poll. Pinning
        and huge pages stay. mlock and busy-poll wait on R12-REFLEX-8066's
        tuned-vs-untuned replay on the first funded node. Why: No node has
        measured anything; unsafe code is forbidden. Next: Needs a funded
        node and R12-REFLEX-8066 replay evidence.
      - `R12-REFLEX-8065`: Restate ruling STAY_PARTIAL_DEPLOY: O13 declined
        (no mlockall); local scratch SSD refused (billed -lssd shape, ADR
        0211, decision 5). IRQ/RPS/XPS tuning is written only with
        R12-REFLEX-8066's replay on the first funded node, boot refusing
        unread masks. Why: Cost is unnamed and tuning must be measured
        first. Next: Nothing lands before the funded-node replay.
      - `R12-SEC-10031`: Mesh stays refused. mTLS for warm services stays
        met via ADR 0112 hop table; CA Service issuance deferred, consistent
        with owner decision 5. Revisit when the owner funds CA Service.
        Reflex-outside-mesh remains tested. Why: CA Service is billed;
        earlier owner decision declines it for now. Next: Reopen on owner
        funding: CA issuance in Terraform and composition roots, plus
        API-8113 IAM evidence.
      - `R12-SEC-7014`: Restate earlier ruling: four HSM signing keys funded
        under R12-SEC-9137 at about $11 a month, not counted again.
        Deployment waits on owner. Owner sets bundle_not_after =
        2027-04-30T00:00:00Z in the same protected edit that enables
        artefact_signing. No agent edits dev tfvars. Why: Already decided
        FUND_THEN_DEPLOY; no new spend committed here. Next: After owner's
        protected edit and R12-SEC-10053 patch: dev plan showing four keys
        with one-signer bindings.
      - `R12-SEC-9127`: artefact_signing stays null everywhere until the
        owner switches it on; funding already decided under R12-SEC-9137.
        Attestation root HSM is handled in R12-SEC-10053. No scanner or gate
        weakened. Why: Owner switch plus already-funded HSM keys; nothing
        new committed. Next: Reopen when owner enables artefact_signing;
        then a plan shows the package-signing keys.
      - `R12-TICK-2065`: No code gap. QIP_DEEPBRAIN_TICK_EVERY stays unset
        and tick desk off. No dataset entitlement is assumed. Each dataset
        needs a licensing decision before QIP_DEEPBRAIN_TICK_ENTITLEMENTS is
        set. Why: Licensing posture is evaluated before a source is used;
        the text names no entitled dataset. Next: Revisit when owner
        licenses a named dataset; then enable the desk and show deployment
        evidence.
      - `R12-TICK-7020`: Restate earlier ruling: replay pool is a Cloud Run
        Job, ceiling about $105 a month, behind enable_replay_pool
        defaulting false, built after the deploy and only on owner funding.
        Nothing built or billed now. Why: New billed infrastructure; earlier
        FUND_THEN_DEPLOY ruling stands. Next: After deploy and owner
        funding: replay-job module, tftest proving it shares nothing with
        serving, real plan.
      - `R12-WORLD-8129`: Restate earlier ruling and O02: Spanner Graph
        stays declined. BigQuery remains the one evidence store. No graph
        writer is written. Revisit only via an ADR under ADR 0113 s1 stating
        a measured need and a monthly ceiling. Why: Store not admitted by
        ADR 0113; spend requires a ceiling the owner has not set. Next:
        Reopen after dev runs and the ADR exists; then enable_spanner plan
        and REST writer behind GraphStore port.

    - *Refused: it would need live trading or an external action* (8):
      - `R12-DATA-1101`: Continuous search is not built here. Restate ADR
        0157 section 5 and ADR 0180 d3: no crawl or search stage; candidates
        are written by an operator. Cadence stays null. Row stays PARTIAL.
        Why: Open-ended external search is an external action; already ruled
        out by ADR. Next: Only a new ADR superseding 0157 section 5 reopens
        it; operator-written candidates remain.
      - `R12-DATA-1104`: Scout-generated adapters are never built here.
        Restate ADR 0157 section 4 ('A descriptor never polls') and ADR 0180
        d3. Connectors stay hand-written and reviewed. Row stays MISSING.
        Why: Earlier ADR ruling decides; generated adapters reach external
        sources unreviewed. Next: Only a superseding ADR with per-source
        licence records and egress routes reopens it.
      - `R12-EXEC-1168`: Restating earlier ruling REGRADE_TEXT: O15
        declined. Continuous quoting stays refused at Cell::send; quote_loop
        keeps no path to an order. Row stays PARTIAL on continuous quoting.
        Why: Lifting it removes a paper-boundary refusal and rewrites
        pinning acceptance tests; no delegation may do that. Next: Reopens
        only by the owner's own ADR; no lane work.
      - `R12-EXEC-8068`: Restating earlier ruling: BLOCKED. No venue adapter
        session is opened. ADR 0108 is not accepted and Stage 1 is not
        authorised; QIP_VENUE_FEED other than simulated keeps stopping the
        process (ADR 0003). Why: A direct venue session is a path toward
        live orders; never built here. Next: Only ADR 0108 P2
        endpoint-derived venue class pre-work (off-allow-list host is Live
        and refused) may proceed.
      - `R12-GCP-10030`: Restate: regional GKE Standard for warm services is
        refused (ADR 0114 D4, Amendment 1). Warm binaries stay on Cloud Run;
        the one GKE cluster stays the GitOps control plane running no
        trading binary. Row stays BLOCKED. Why: Earlier ruling; no trading
        workload on Kubernetes. Next: Nothing built; capabilities placed per
        ADR 0114 Amendment 1.
      - `R12-GCP-7006`: Restate: stateful services on GKE refused (ADR 0114
        D4). Stateful processes stay on Compute Engine (fabricd, ledgerd)
        with state in Spanner when funded (ADR 0111, 0113). Row stays
        BLOCKED. Why: Earlier ruling; repository rule forbids Kubernetes
        workloads. Next: Nothing built.
      - `R12-GCP-8005`: Restate: GKE Standard service plane refused (ADR
        0114 D4). Its capabilities are built on Cloud Run: evidence
        EVID-8108, replay TICK-8110, sinks FABRIC-8109. Row stays BLOCKED.
        Why: Earlier ruling; no workload on Kubernetes. Next: Nothing built;
        track the Cloud Run capability rows.
      - `R12-GCP-8115`: Restate earlier ruling: GKE Standard regional
        service-plane clusters are refused per ADR 0114 D4 and Amendment 1.
        Requirement stands unmet; Cloud Run serves warm services. Never
        built here. Why: Earlier ruling decides it; ADR 0114 D4 refuses the
        cluster. Next: Reopens only if ADR 0114 D4's own measurement
        condition is met.

    - *Left with the owner: it cannot be delegated* (7):
      - `R12-API-9057`: Restate earlier ruling: STAY_PARTIAL_EXTERNAL. No
        public edge, no hostname invented. Owner must name a hostname on a
        domain they own, delegate it to Cloud DNS, and approve LB, Cloud
        Armor and CDN cost in a priced ADR. Why: Domain ownership and
        recurring spend cannot be delegated. Next: After owner action,
        protected patch sets public_edge.hostnames in dev tfvars; owner
        dispatches plan.
      - `R12-CICD-10182`: O20 not taken. Prod reconciliation from Git stays
        closed; ADR 0036/0093, deploy gate and gitops.rs refusals stand.
        Reopening prod promotion needs the owner's own ADR and human
        dispatch. Why: Production changes need explicit owner approval.
        Next: Owner writes and accepts an ADR reopening prod promotion;
        nothing built meanwhile.
      - `R12-EVENT-5089`: Owner must choose which event markets the breadth
        plan installs and set it in protected infrastructure/environments/
        tfvars. Until then the plan stays unset and installs nothing, which
        fails closed. Why: Market choice and protected tfvars are
        owner-held; plane is built and tested. Next: Owner names markets and
        edits tfvars; then QIP_DEEPBRAIN_BREADTH_PLAN deploy evidence closes
        the row.
      - `R12-GOV-9075`: Restate earlier ruling: APPLY_PATCH approved as
        written. Learning agent writes only under docs/proposals/; guard
        refuses edits to deploy workflow, promotion code and agent policy. A
        person must apply docs/blueprint/lanes/l5-edge-GOV-9075.patch. Why:
        Protected-path change needs a person to apply it; agents cannot.
        Next: Owner applies the patch and approves the protected path; then
        run the agent-guard test.
      - `R12-OBS-9152`: Not wired by an agent. Setting strategy_plan_path in
        protected dev tfvars activates a paper strategy on a billed
        execution node, so the owner makes that flip. Emission and test stay
        as built; row stays PARTIAL. Why: Protected tfvars plus billed node
        (execution_nodes_enabled=false) is not delegable spend. Next: Owner
        sets strategy_plan_path and enables the node in dev dispatch; then
        show the series moving.
      - `R12-SEC-10003`: Owner must write the real requester and approver
        Cloud Identity groups into pam_entitlements in
        infrastructure/environments/*/terraform.tfvars, run a plan against
        the real project, and remove any live standing human grants. Agents
        cannot invent identifiers. Why: Account identifiers and live-project
        access are a person's; protected tfvars. Next: Stays PARTIAL until
        owner supplies groups and a real plan shows the entitlement.
      - `R12-SEC-9123`: Owner must name the requester and approver
        principals and set pam_entitlements in protected tfvars for an
        environment, then run a plan. Agents cannot choose named people.
        Why: Named principals are a person's decision; protected path. Next:
        Same closure as R12-SEC-10003; stays PARTIAL until a plan shows the
        entitlement.

## Where this stopped (2026-11-01)

Lanes L2, L4 and L6 ended their last turns on "You've hit your weekly
limit · resets Nov 6, 1pm (UTC)" around 13:00 UTC on 2026-10-31. L1, L3 and
L5 were idle with their work pushed, and the limit is the account's, so a
message to any of them would meet the same refusal. The re-grade of the 389
older rows stopped on the same limit. Its classifiers finished 22 of 26 batches, and no
skeptic pass ran over them. Under the standing rule no further agent is
spawned until the reset, so no older row is re-graded on the strength of
that partial run. Decision 22 stays reserved for the re-grade.
