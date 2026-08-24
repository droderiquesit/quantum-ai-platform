# What the architecture diagram shows and this configuration does not create

Three Google services appear on the canonical platform diagram and are not
simply provisioned here. Two of them have no Terraform at all; the third —
Confidential VMs — became a default-false variable rather than an absence, and
its section below says why that is the same decision written somewhere it can
be disagreed with.

Each is a decision, not an oversight, and each is cheaper to defend in writing
than to discover from a bill.

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

## Confidential VMs — now a choice, and still off

This section used to say Confidential VMs were absent. They are no longer
absent; they are **opt-in and default false**, behind
`enable_confidential_nodes` in the root variables and in `modules/cluster`. The
reasoning did not change, so it now lives in the variable's description where
somebody deciding will actually read it, and the rest of this section is the
long form.

`crates/libs/qip-confidential` deliberately does **not** provide confidential
computing: it is statistical disclosure control — a k-anonymity gate, a
monotone privacy budget and calibrated noise — and its module documentation
says in its first paragraph that there is no enclave, no attestation and no
hardware isolation.

Enabling Confidential VMs on the node pool is a real hardening step and a
defensible one. It stays off by default because turning it on lets the crate's
name and the cluster's configuration together imply a guarantee neither
provides, and the crate is explicit that it does not defend against a malicious
operator with host access. Nothing in this platform attests a node, and no
decision anywhere is gated on a node having been attested — so a cluster with
this on is a cluster whose node memory is encrypted, and not a platform whose
computations are verifiable. Turn it on as defence in depth if you like; do not
turn it on and conclude fabric D is now confidential computing.

Why it became a flag rather than staying absent: the difference between "we
decided against this" and "nobody thought of it" is invisible in a
configuration that simply lacks a setting. A default-false variable carrying
the argument is the same decision, written where it can be disagreed with.

Two costs, both refused at plan time rather than at apply, by a precondition on
the node pool:

  * **The machine family must be AMD** — n2d, c2d or c3d, for AMD SEV. Neither
    the `n2-standard-4` default nor production's `e2-standard-16` qualifies, so
    this is never a one-line change; the flag and the machine type move
    together or the plan fails with both values in the message.
  * **The cluster is replaced.** `confidential_nodes` forces replacement, so
    this is a decision made before a cluster exists or during a rebuild, not on
    a Tuesday afternoon.

## The general rule

Nothing here provisions infrastructure this build cannot reach. Every managed
data service in `modules/data` is default-false for the same reason, and the
`enabled_without_an_adapter` output reports at plan time any that were switched
on ahead of the code that would use them.
