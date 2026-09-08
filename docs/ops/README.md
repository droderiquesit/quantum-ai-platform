# Operations, security and policy

* [Security](security/README.md) — threat model, controls, what is not covered
* [Policies](policies/README.md) — model, agent and data governance; change management
* [Observability](observability/README.md) — what is instrumented and what to watch

Runbooks are in [docs/operations](../operations/README.md), and the one
deployment path is [docs/operations/deployment-path.md](../operations/deployment-path.md).

Dated records of what was run against something real, and what the run does
not prove:

* [The ECB reference rates through the loop, live (2026-09-06)](live-source-frankfurter-2026-09-06.md)
* [Execution measurements](execution-measurements.md)

Both registers of what is switched off or missing were consolidated into
[`../DELIVERY-STATUS.md`](../DELIVERY-STATUS.md) on 2026-09-07 and deleted.
They were "kept as a pair so that they cannot disagree about one switch",
which is the argument for one document rather than two: what the architecture
of record requires and the tree does not provide is now the §41–§46 rows of
the delivery status, and the switches held closed are a section of it.
