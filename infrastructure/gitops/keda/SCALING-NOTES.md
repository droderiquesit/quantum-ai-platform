# KEDA and the qip-api autoscaler: what a swap would look like, and why not yet

`infrastructure/kubernetes/base/api.yaml` carries the only autoscaler in this
repository — an `autoscaling/v2` HorizontalPodAutoscaler, CPU-only at
`averageUtilization: 300` against a 250m request, 2–6 replicas, permissive
scale-up and deliberately slow scale-down. Every one of those numbers is
argued in that file's comments, and nothing below changes any of them.

## The drop-in, if it were wanted

```yaml
apiVersion: keda.sh/v1alpha1
kind: ScaledObject
metadata:
  name: qip-api
  namespace: qip
spec:
  scaleTargetRef:
    # `kind` defaults to Deployment; writing it would put a literal
    # `kind: Deployment` line in this document, which api.yaml's HPA
    # comment already explains the acceptance suite misreads as a
    # workload. Omit it here for the same reason the HPA inlines it.
    name: qip-api
  minReplicaCount: 2
  maxReplicaCount: 6
  triggers:
    - type: cpu
      metricType: Utilization
      metricName: cpu
      metadata:
        value: "300"
  advanced:
    horizontalPodAutoscalerConfig:
      # Carried over verbatim from the HPA's `behavior` block — the slow
      # scale-down exists because each departing replica takes its
      # rate-limit counters and cell-report registry with it.
      behavior:
        scaleUp:
          stabilizationWindowSeconds: 60
          policies:
            - type: Percent
              value: 100
              periodSeconds: 60
        scaleDown:
          stabilizationWindowSeconds: 600
          policies:
            - type: Pods
              value: 1
              periodSeconds: 300
```

The existing HPA cannot stay alongside this: KEDA materialises its own HPA
(`keda-hpa-qip-api`) from the ScaledObject, and two HPAs steering one
Deployment fight each other with no error surfaced. A migration is therefore
"delete the HPA and add the ScaledObject in one commit", or adopt the
existing object with `scaledobject.keda.sh/transfer-hpa-ownership: "true"`
plus `advanced.horizontalPodAutoscalerConfig.name: qip-api`.

## The caveat that decides it

KEDA's `cpu` scaler does not measure anything KEDA-ish. It writes the same
`type: Resource` CPU metric into the HPA it manages that api.yaml already
declares by hand; the metric still flows kubelet → metrics-server → HPA
controller, never through KEDA's adapter. The swap changes who owns the HPA
object, not how a single scaling decision is made. What it adds today is
machinery in the decision path — an operator, an aggregated APIService, a
webhook — that must be vendored, attested, scanned, and network-policed
(this directory), for zero behavioural difference. A component that changes
nothing but adds failure modes is not hardening, it is surface.

## When it becomes warranted

When a trigger exists that the built-in HPA cannot express: queue depth, SSE
connection count, mesh spool backlog — a signal of *work waiting* rather
than *CPU spent*. Two prerequisites, in order:

1. **The platform must emit the metric.** Nothing currently writes to
   `Telemetry` (`.claude/rules/domains/observability.md` says so in bold),
   so today there is no queue-depth or connection-count series for any
   scaler to read. The emitting code comes first; adopting KEDA before it
   exists would be installing a thermostat in a house with no thermometer.
2. **The route must be granted.** `base/policies.yaml` here deliberately
   gives the keda namespace egress to the control plane only. A scaler
   polling qip-api's metrics endpoint needs an explicit egress rule to that
   pod and port, added in the same commit as the ScaledObject — the
   control-plane-egress comment names this as the intended procedure.

Until both hold, the HPA in api.yaml stays. This directory makes KEDA
deployable the day a real event-driven trigger arrives; it does not argue
for switching before then.
