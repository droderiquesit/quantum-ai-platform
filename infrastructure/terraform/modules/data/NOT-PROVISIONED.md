# What the architecture diagram shows and this configuration does not create

Three Google services appear on the canonical platform diagram and have no
Terraform here. Each omission is a decision, not an oversight, and each is
cheaper to defend in writing than to discover from a bill.

## Pub/Sub as the streaming backbone

The diagram places Google Pub/Sub between the data mesh and the regional
brains, carrying every state delta and capital envelope in the platform.

ADR 0011 replaced it with an in-tree HTTP/1.1 mesh (`crates/libs/qip-transport`),
and that decision is load-bearing rather than cosmetic: retries with seeded
jitter, a bounded outbound queue that refuses rather than drops, at-least-once
delivery with idempotency keys, dead-lettering, a circuit breaker and a durable
spool are all code in this repository that had to be right. Provisioning the
service they replaced would create a bus nothing publishes to, and a topic with
no publisher is indistinguishable from a topic whose publisher is broken.

There is exactly one Pub/Sub topic in this configuration, in `modules/secrets`,
and it carries no platform data: Secret Manager will not accept a rotation
schedule without somewhere to announce a rotation is due.

## Dataflow

The diagram lists it under the technology stack. No code in this repository
submits a Dataflow job, and the transformation work it would do — normalisation,
deduplication, enrichment — is implemented in `qip-normalization` and
`qip-market-ingestion` and runs in-process. Provisioning a pipeline runner with
no pipeline produces a service account, a subnet allocation and a quota, all
attached to nothing.

## Confidential VMs

The diagram lists them, and `crates/libs/qip-confidential` deliberately does
**not** provide confidential computing: it is statistical disclosure control —
a k-anonymity gate, a monotone privacy budget and calibrated noise — and its
module documentation says in its first paragraph that there is no enclave, no
attestation and no hardware isolation.

Enabling Confidential VMs on the node pool is a real hardening step and a
defensible one. It is omitted here because doing it would let the crate's name
and the cluster's configuration together imply a guarantee neither provides,
and the crate is explicit that it does not defend against a malicious operator
with host access. Turn it on as defence in depth if you like; do not turn it on
and conclude fabric D is now confidential computing.

## The general rule

Nothing here provisions infrastructure this build cannot reach. Every managed
data service in `modules/data` is default-false for the same reason, and the
`enabled_without_an_adapter` output reports at plan time any that were switched
on ahead of the code that would use them.
