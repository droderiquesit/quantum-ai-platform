# How qip scales, layer by layer, and what each layer is actually measuring

Four mechanisms decide how much of this platform runs. They are easy to
confuse with each other, and the confusion is expensive — two of them writing
the same number is an outage, and three of them are frequently blamed for a
problem belonging to the fourth.

| Layer | What it changes | Where it lives |
|---|---|---|
| KEDA `ScaledObject` | replica count of `qip-api` | `infrastructure/kubernetes/base/api.yaml` |
| Node pool autoscaler | nodes inside the committed pool | `modules/cluster`, per-zone min/max |
| Node auto-provisioning + `ComputeClass` | new node pools when no existing pool fits | `modules/cluster`, `base/autoscaling.yaml` |
| VPA recommenders | nothing — they measure and report | `base/autoscaling.yaml`, `updateMode: "Off"` |

## The HPA became a ScaledObject, and what that did and did not buy

An earlier draft of this document argued against the swap. The desk directed
it, and it shipped: `qip-api`'s `autoscaling/v2` HorizontalPodAutoscaler is
now a KEDA `ScaledObject` carrying the identical decision — same 2–6 bounds,
same CPU target of 300, same permissive scale-up and deliberately slow
scale-down, comments moved intact — adopted onto the existing HPA object with
`scaledobject.keda.sh/transfer-hpa-ownership: "true"` and
`advanced.horizontalPodAutoscalerConfig.name: qip-api`. Live, the HPA's
`ownerReferences` names `ScaledObject/qip-api`: one owner, which is the point.
Two HPAs steering one Deployment do not error, they argue.

The caveat that argued against it is still true and worth keeping, because it
is what stops anyone reading the swap as a performance change: KEDA's `cpu`
scaler measures nothing KEDA-ish. It writes the same `type: Resource` metric
into the HPA it manages, flowing kubelet → metrics-server → HPA controller,
never through KEDA's adapter. **The swap changed who owns the HPA object, not
how a single scaling decision is made.** What it bought is the seam: when a
real event-driven signal exists it becomes one more entry under `triggers`
rather than a second scaling system arguing with the first.

Those prerequisites have not moved:

1. **The platform must emit the metric.** Nothing currently writes to
   `Telemetry` (`.claude/rules/domains/observability.md` says so in bold), so
   there is no queue-depth or connection-count series for any scaler to read.
2. **The route must be granted.** `base/policies.yaml` gives the keda
   namespace egress to the control plane only. A scaler polling a pod's
   metrics endpoint needs an explicit egress rule to that pod and port, added
   in the same commit as the trigger.

## The layer that is actually mis-set: requests

Measured on the dev cluster at 01:30 UTC on 2026-08-31, after the working-set
bounds landed and `qip-fastbrain`'s cycle fell from 310ms to 2.3ms:

| Workload | Reserved (requests × replicas) | Observed use |
|---|---|---|
| `qip-api` | 500m / 512Mi (250m × 2) | 2m, ~0Mi |
| `qip-deepbrain` | 1000m / 2Gi | 1m, ~0Mi |
| `qip-fastbrain` | 2000m / 2Gi | 63m, 93Mi |
| **total** | **3500m of 11760m allocatable — 30% of the cluster** | **~66m** |

Thirty percent of a three-node cluster is reserved to do roughly nothing, and
that reservation — not utilisation — is what the scheduler packs on, what
decides whether `qip-api` can reach six replicas, and what node
auto-provisioning would buy machines to satisfy. It is the live scaling
problem on this cluster; the autoscalers above are all behaving correctly.

Two reasons it is not simply cut here:

- **The VPA numbers are not yet clean.** The recommender reported
  `qip-fastbrain` at 1150m/1204Mi, but its window spanned the pre-fix
  pathology when every cycle burned 310ms. A recommendation measured across a
  bug is a recommendation about the bug. Right-sizing waits for a window that
  is entirely post-fix.
- **`requests == limits` is a contract, not an accident.** The fast path sets
  them equal so the pod is Guaranteed QoS and is never throttled or evicted —
  `fastbrain.yaml` argues it, and `modules/cluster` picks the `BALANCED`
  autoscaling profile for the same reason. Lowering *both* together keeps the
  guarantee; lowering only requests destroys it. Whatever the numbers say, the
  edit keeps them equal.

Headroom above observed peak matters more here than in a typical service: the
cycle is bursty by construction, with SIMULATE and REASON spiking well above
the steady state that `kubectl top` samples.

## Why the recommenders never write

`updateMode: "Off"` on all three VPAs. A VPA that rewrote requests would be a
second writer to the very pods KEDA scales, and it would silently overwrite a
reviewed decision — the requests-equal-limits contract above is exactly the
kind of value a recommender would drift away from while looking helpful.
Reading a recommendation is `kubectl describe vpa -n qip`; acting on one is a
reviewed edit to the workload's manifest, like any other resource change.
