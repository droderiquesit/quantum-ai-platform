# Scaling and availability

**Only the API scales horizontally. Adding instances to anything else here is a
correctness change, not a capacity change.** That one sentence is the whole
page; the rest is what the configuration actually does about it, which is less
than the sentence, and what to do when something makes you want to ignore it.

The runtime is the one `infrastructure/terraform/catalogue.tf` and
`modules/execution-node` provision under
[ADR 0024](../adr/0024-the-blueprint-runtime-is-provisioned-in-code-and-the-gitops-runtime-is-retired.md).
It has never been applied.

## What scales, and what the Terraform says about it

| Workload | Concurrency | Instances | What a second instance does |
| --- | --- | --- | --- |
| `qip-api` | 80 (`catalogue.tf:49`) | 0 to 4 (`catalogue.tf:56-57`) | Halves what each instance carries. This is the one that works. |
| `qip-fastbrain` | 1 (`catalogue.tf:138`) | exactly 1 (`catalogue.tf:152-154`) | Would poll the feed again, build a second world model, decide again, and append to the same hash-chained log — and this is the workload permitted to reach a venue. The ceiling of one makes it impossible. |
| `qip-deepbrain` | 1 (`catalogue.tf:203`) | exactly 1 (`catalogue.tf:211-213`) | Would write a second copy of the same evidence, indistinguishable from a replay. Same ceiling, same reason. |
| execution node | — | 1, or 2 only during a replacement (`modules/execution-node/variables.tf:126-144`) | Trades a second book against the same signed capital envelope. |

Read the third column before the first. `concurrency` bounds *requests per
instance*, not instances; the instance bounds are the catalogue's own, passed
through to the module per entry (`catalogue.tf:320-322`, applied at
`modules/cloudrun/main.tf:464-465`) rather than left to its defaults of a zero
floor and a ceiling of four (`modules/cloudrun/variables.tf:242-283`). The
API takes `0/4` because it is the one workload that answers requests. Both
brains are pinned to `min_instances = 1`, `max_instances = 1` with an
`always_on_justification`, and the two halves of that pin close two failures
this runtime would otherwise have:

* **A brain cannot scale to zero.** Nothing calls a brain — no scheduler, no
  invoker, and `POST /cycle` is the API's own route — so with a zero floor
  the instance Cloud Run retired for want of a request would never be started
  again, and the cycle would simply stop on the first quiet hour. The floor
  of one keeps it, and keeps its CPU allocated between requests, which a loop
  that never receives one needs (`cpu_idle = var.min_instances == 0`,
  `modules/cloudrun/main.tf:497`). A floor above zero is refused at plan time
  without a written justification (`modules/cloudrun/main.tf:217-231`); each
  brain's entry carries one.
* **A brain cannot scale to two.** Each brain opens the event log and runs
  the cycle on its own clock, so a second instance would run the same cycle
  and append to the same hash-chained log. Two writers of one chain produce
  the fork the chain exists to detect, and the platform would report its own
  redundancy as corruption. The ceiling of one makes that structural rather
  than a comment beside `concurrency`; a probe that arrives while the one
  instance is busy queues, and does not start a second.

The execution node is the one workload with a hard bound: `node_count` is one,
and the module refuses more than two even during a replacement.

## The API has hit its ceiling

1. Confirm it is load and not a stuck revision. `deploy.yml` proves the serving
   revision is Ready and runs the attested digest before it records anything
   (`.github/workflows/deploy.yml:487-505`); a revision that is not Ready is a
   failed deployment, not a scaling problem.
2. **Do not raise `max_instances` first.** Four is the module's default and
   deliberately bounded: an unbounded ceiling turns a retry storm into a bill
   (`modules/cloudrun/variables.tf:276-284`).
3. Know what you are buying. Two things in the API are per-process and get
   worse with every instance:
   * **Rate-limit counters.** Four instances means a caller's real allowance is
     four times what one process believes it is enforcing.
   * **The cell registry.** A cell's report reaches whichever instance Cloud
     Run routed it to. The more instances, the smaller the share each one sees,
     and the likelier the console served by another calls a cell stale that is
     not. It is display rather than risk arithmetic, but it is what an operator
     is reading during an incident — and on this runtime no cell reports at
     all, because the mesh path is unwired (`catalogue.tf:21-27`).
4. If the load is real and sustained, the fix is to move that state out of the
   process — not to raise the ceiling.

## A zone was lost

Cloud Run is regional; the services need nothing from you.

The execution node is zonal on purpose — its group and its placement policy
are claims about one zone (`modules/execution-node/variables.tf:32-43`) — so a
zone loss is that node gone, with its journal on the deleted boot disk. What
survives is in its snapshots, if the schedule was attached; see
[disaster recovery](disaster-recovery.md). Standing the node up in another zone
is a change to its `zone` in `execution_nodes` and a plan, and the new machine
starts with an empty journal.

## A node is being replaced

There is no drain. Replacement is the group's update policy: surge one, wait
for the health check, retire the old machine (`modules/execution-node/main.tf:433-447`),
and `deploy.yml` invokes exactly that for every group in the environment,
`--max-surge 1 --max-unavailable 0` (`.github/workflows/deploy.yml:516-538`).
Auto-healing replaces an instance after three failed checks ten seconds apart,
with a five-minute initial delay so a booting node is not replaced for being
slow to fetch its secrets (`main.tf:270-290,449-457`). Host maintenance
terminates the instance rather than live-migrating it, and it restarts
(`main.tf:368-377`): a pause invisible to everything except the workload whose
purpose is microseconds is replaced by an event the deployment can see.

Two machines briefly holding venue sessions for one cell is why the surge is
one and the standing size is one (`variables.tf:130-134`).

## Making a brain scale, properly

Both brains run a singleton loop. Running more than one needs leader election
in the binary: a lease, a fenced hand-off, and a rule for the cycle in flight
when leadership moves. **None of it exists**, and no Terraform change
substitutes for it. Until it does:

* Scale the deep brain **vertically**. Its shape is `cpu = "4"`,
  `memory = "8Gi"` in `catalogue.tf:201-202`, and a change there is a plan a
  reviewer reads. Nobody has measured where in that band a cycle sits, and
  there is no recommendation-mode autoscaler on this runtime to measure it
  for you.
* The fast brain's instance bound is a decision, not an open question: the
  catalogue pins it at one (`catalogue.tf:152-154`), for the reasons above.
  Raising that ceiling is the leader-election work, not a plan.

## Related

* [Disaster recovery](disaster-recovery.md) — what is irreplaceable, and why
  positions are reconciled rather than restored.
* [Deploying a new edge cell](deploying-an-edge-cell.md) — the correct way to
  add edge capacity.
* [Multi-region](multi-region.md) — the same singleton argument, across
  regions.
* [External dependencies](external-dependencies.md) — the standing list of
  what this build cannot reach.
