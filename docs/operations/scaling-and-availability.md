# Scaling and availability

**Written for the retired runtime.** Replicas, KEDA and the autoscaler this page describes were retired under [ADR 0024](../adr/0024-the-blueprint-runtime-is-provisioned-in-code-and-the-gitops-runtime-is-retired.md). The rule at the top is unchanged and is now enforced by the catalogue: only the API takes `concurrency = 80`; the fast brain and the deep brain run at `concurrency = 1`, and Cloud Run's own scale-to-zero is bounded by `modules/cloudrun`. Rewriting the rest for Cloud Run is open work.

**Only the API scales horizontally. Adding replicas to anything else here is a
correctness change, not a capacity change.** That one sentence is the whole
page; the rest is what to do when something makes you want to ignore it.

## What scales, and what happens if you scale it anyway

| Workload | Replicas | Scaling | What a second replica does |
| --- | --- | --- | --- |
| `qip-api` | 2–6, autoscaled on CPU | **Horizontal.** Requests arrive from outside and a replica serves one. | Halves what each replica carries. This is the one that works. |
| `qip-fastbrain` | 2, fixed | **None.** Runs its own ingest-and-cycle loop. | Polls the feed again, builds a second world model, decides again — and this is the workload permitted to reach a venue. |
| `qip-deepbrain` | 1, fixed | **None.** Same loop, slower. | Writes a second copy of the same evidence, indistinguishable from a replay. |
| edge cell | 2 per cell, fixed | **None.** Capacity comes from *new cells*. | Trades a second book against the same signed capital envelope. |

The reasoning for each refusal used to sit in each manifest, at the point
where somebody would go to add an autoscaler. The manifests are gone
(ADR 0024); it now sits beside each workload's `concurrency` in
[catalogue.tf](../../infrastructure/terraform/catalogue.tf), and for the
cell in [modules/execution-node](../../infrastructure/terraform/modules/execution-node/main.tf),
which provisions exactly one machine per entry.

## The API is pinned at six replicas

1. Confirm it is CPU and not a stuck metric — an autoscaler with no metrics
   reports `unknown` and holds its last count rather than climbing.
2. **Do not raise `maxReplicas` first.** Six is a capacity number: the pool is
   two nodes per zone across three zones with no cluster autoscaler, so a
   seventh replica may simply be `Pending`, which reduces capacity rather than
   adding it. Check for free CPU on the pool before changing the ceiling.
3. Know what you are buying. Two things in the API are per-process and get
   worse with every replica:
   * **Rate-limit counters.** Six replicas means a caller's real allowance is
     six times what one process believes it is enforcing.
   * **The cell registry.** A cell's report reaches whichever replica the
     Service picked. The more replicas, the smaller the share each one sees,
     and the likelier the console served by another calls a cell stale that is
     not. It is display rather than risk arithmetic, but it is what an
     operator is reading during an incident.
4. If the load is real and sustained, the fix is to move that state out of the
   process — not to raise the ceiling.

## A zone was lost

Expect: roughly a third of the API replicas gone, and the pods that were there
rescheduling into the surviving zones. Every spread constraint on the platform
is `ScheduleAnyway`, deliberately, so the survivors accept those pods even
though it breaches the skew. A badly balanced API is the intended outcome; a
`Pending` one is not.

* **`qip-deepbrain` has one replica and no zone redundancy.** If it was in the
  lost zone it reschedules and comes back `warming` — there is no world model
  until its first cycle lands. That is expected and is not a fault.
* **A cell that lost both pods stops trading for its venues** until they
  reschedule, and will not trade at all until it holds a verified envelope.
* If pods are `Pending` rather than rescheduling, the pool has no room in the
  surviving zones. That is a node-count problem, not a manifest problem.

Do not "fix" a skew warning by changing `whenUnsatisfiable` to
`DoNotSchedule`. During exactly this event, that setting is what would refuse
to place the replacements.

## A node drain is stuck

Every disruption budget here allows one voluntary eviction at a time and none
of them blocks a drain. If a drain is stuck on a `qip-` pod, the budget has
been edited — most likely to `minAvailable` equal to the replica count, which
forbids every eviction. Node auto-upgrade is enabled and drains without asking,
so that edit does not protect the workload, it stalls the upgrade against it.

`qip-deepbrain` uses `maxUnavailable: 1` because it has a single replica;
`minAvailable: 1` there would make its node undrainable. `qip-api` uses
`maxUnavailable: 1` because its replica count is not fixed — a fixed
`minAvailable` under an autoscaler permits a drain to take five of six at once.

## Making a brain scale, properly

Both brains run a singleton loop. Running more than one needs leader election
in the binary: a lease, a fenced hand-off, and a rule for the cycle in flight
when leadership moves. **None of it exists**, and no manifest change
substitutes for it. Until it does:

* Scale the deep brain **vertically** — it is one replica with a four-fold gap
  between its requests and its limits, and nobody has measured where in that
  band a cycle actually sits. `deepbrain.yaml` carries a
  `VerticalPodAutoscaler` in recommendation mode, commented out, with the two
  preconditions for turning it on. The cluster has no VPA controller today, so
  committing it live would fail the deploy rather than produce a
  recommendation.
* The fast brain's `replicas: 2` is an **open question**, not a decision. The
  foot of `fastbrain.yaml` sets out what two replicas produce and the two
  honest resolutions. It needs whoever owns the execution path.

## Related

* [Disaster recovery](disaster-recovery.md) — what is irreplaceable, and why
  positions are reconciled rather than restored.
* [Deploying a new edge cell](deploying-an-edge-cell.md) — the correct way to
  add edge capacity.
* [External dependencies](external-dependencies.md) — the standing list of
  what this build cannot reach.
