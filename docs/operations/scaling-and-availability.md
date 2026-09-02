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
| `qip-api` | 80 (`catalogue.tf:49`) | 0 to 4, the module defaults | Halves what each instance carries. This is the one that works. |
| `qip-fastbrain` | 1 (`catalogue.tf:124`) | 0 to 4, the module defaults | Polls the feed again, builds a second world model, decides again — and this is the workload permitted to reach a venue. |
| `qip-deepbrain` | 1 (`catalogue.tf:171`) | 0 to 4, the module defaults | Writes a second copy of the same evidence, indistinguishable from a replay. |
| execution node | — | 1, or 2 only during a replacement (`modules/execution-node/variables.tf:126-144`) | Trades a second book against the same signed capital envelope. |

Read the third column before the first. `concurrency` is the one scaling value
the catalogue sets, and it bounds *requests per instance*, not instances. The
catalogue passes neither `min_instances` nor `max_instances`, so every service
takes `modules/cloudrun`'s defaults: a floor of zero and a ceiling of four
(`modules/cloudrun/variables.tf:242-283`; applied at `modules/cloudrun/main.tf:359-364`).
Two consequences the previous runtime did not have:

* **A brain can scale to zero.** With `min_instances = 0`, an instance that
  receives no request is retired and its CPU is throttled between requests
  (`cpu_idle = var.min_instances == 0`, `modules/cloudrun/main.tf:393`). The
  brains run their own ingest-and-cycle loop; on this configuration that loop
  runs only while an instance exists and is serving. Raising the floor needs
  `always_on_justification`, refused empty at plan time
  (`modules/cloudrun/main.tf:181-190`), and the catalogue does not carry one
  for either brain.
* **A brain is not pinned to one instance.** Nothing in the catalogue says
  `max_instances = 1` for the brains. Concurrency of one plus a ceiling of four
  means the fourth concurrent request starts a fourth instance, which is the
  divergent-world-model failure in the second row. The reasoning against a
  second replica sits beside each `concurrency` in `catalogue.tf`; the bound
  itself is not expressed, and this page does not pretend it is.

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
  `memory = "8Gi"` in `catalogue.tf:169-170`, and a change there is a plan a
  reviewer reads. Nobody has measured where in that band a cycle sits, and
  there is no recommendation-mode autoscaler on this runtime to measure it
  for you.
* The fast brain's instance bound is an **open question**, not a decision:
  the catalogue reasons against a second replica and does not pin the
  ceiling at one. It needs whoever owns the execution path, and the change
  is a `max_instances` the catalogue passes through to the module.

## Related

* [Disaster recovery](disaster-recovery.md) — what is irreplaceable, and why
  positions are reconciled rather than restored.
* [Deploying a new edge cell](deploying-an-edge-cell.md) — the correct way to
  add edge capacity.
* [Multi-region](multi-region.md) — the same singleton argument, across
  regions.
* [External dependencies](external-dependencies.md) — the standing list of
  what this build cannot reach.
