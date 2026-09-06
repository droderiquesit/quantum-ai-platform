# Operations, security and policy

* [Security](security/README.md) — threat model, controls, what is not covered
* [Policies](policies/README.md) — model, agent and data governance; change management
* [Observability](observability/README.md) — what is instrumented and what to watch

Runbooks are in [docs/operations](../operations/README.md), and the one
deployment path is [docs/operations/deployment-path.md](../operations/deployment-path.md).

The two registers of what is switched off or missing, kept as a pair so that
they cannot disagree about one switch:

* [Missing infrastructure](missing-infrastructure-register.md) — what the
  architecture of record requires and the tree does not yet provide
* [Off gates](off-gates-register.md) — every control that exists and is
  closed in every environment
