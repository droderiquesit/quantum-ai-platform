# What this module does not enforce

The trust-zone model in blueprint §46.1 is thirteen zones, default deny
between them, and an exhaustive list of what may leave the VPC. A network and
an IAM policy can hold some of that. This file is the rest, written down here
because a boundary a reader believes in and the platform does not have is
worse than a gap somebody is tracking.

Read it before citing this module as evidence for anything.

## Wired, and not validated

The module is instantiated from `infrastructure/terraform/main.tf` under ADR
0024, with the zones, paths, allowlist and ingress read from each
environment's tfvars. Every Cloud Run workload in the catalogue attaches to
its zone's subnet and carries its zone's network tag on its VPC interface, so
the rules here bind those instances.

`terraform` is not installed in the environment this was written in, so
`terraform fmt -check`, `terraform validate` and `terraform plan` have **not
been run** against it, before or after the wiring. The first person with a
Terraform binary runs all three before trusting any refusal below, and starts
with the `lifecycle.precondition` blocks in `main.tf`: those are where the
zone model is enforced, and an unrun precondition is an assertion about an
assertion. The plan to show is the two-sided one the infrastructure rules
require — an `ibm-quantum` entry on any zone but `optimisation` refused, then
the same entry on `optimisation` admitted.

## Three zones hold workloads; ten hold nothing

The catalogue places `qip-api` in `application-identity`, `qip-deepbrain` in
`cognition` and `qip-fastbrain` in `intelligence`. The other ten zones,
`optimisation` among them, have no workload, no identity and — unless an
environment declares a subnet for them — no subnet. A zone with nothing in it
constrains nothing.

The sharpest case is still Optimisation. The IBM Quantum listeners are in the
one egress bootstrap every proxy rendering mounts, so the deep brain's sidecar
declares them; what stops the deep brain reaching IBM is that `cognition` may
hold no external-egress entry at all and `ibm-quantum` may be declared only
under `optimisation`. That is a real network refusal now, not an aspiration —
and it is also the reason no IBM call can succeed from anywhere: **no
optimisation workload exists**, so the only zone permitted to reach IBM has
nothing in it to do so. `qip-deepbrain` links `qip-optimization-engine` and
would need to be split for the constraint to be both enforced and useful.
Until then the IBM-only rule binds, and it binds nothing that wants to pass.

## Identities are the catalogue's, and one-zone-per-identity is not checked here

There is no zone service account. A zone's identities are the Cloud Run
accounts of the workloads placed in it, passed in through `zone_identities`,
and the ledger and fabric grants are made to those. Whether one account
appears under two zones cannot be validated at plan time — the emails are not
known until the accounts exist — so the property rests on the catalogue:
every workload names exactly one zone and `modules/cloudrun` creates exactly
one account per workload. The acceptance suite asserts the first half by
reading `catalogue.tf`.

## Firewall rules are about addresses, not intent

A firewall rule permits a TCP connection between two ranges on a port. It
cannot tell a read from a write, a query from a mutation, or an intent from a
command. The `mode` on a permitted path therefore does two things: it drives
the ledger and control-fabric IAM grants, where the distinction is real, and
it documents the path everywhere else.

Specifically not enforced by any rule here:

- **`intent` really meaning intent.** An application property, held by the
  API those zones expose.
- **`append` really meaning append.** Spanner has no append-only role. A zone
  with an `append` path to the ledger receives `roles/spanner.databaseUser`,
  which can update and delete as well. Append-only is a property of the schema
  and the application.
- **`read` on a path to a zone whose service does not distinguish.**

## The wallet and treasury separation is two things, and this module holds one

The wallet read path and the treasury write path get different subnets and no
sanctioned path between them in either direction. The property that actually
matters — "wallet code cannot link signing code" — is a compile-time fact about
the Rust workspace, verified by dependency audit, and a network cannot see a
linker.

## Per-API restriction needs a perimeter this repository does not have

Every zone may reach `199.36.153.8/30` on 443, and `modules/network`'s private
zone resolves every `*.googleapis.com` to it. A firewall rule cannot tell
`secretmanager.googleapis.com` from `aiplatform.googleapis.com` at that range;
they are the same four addresses and the same port. Restricting a zone to
particular Google APIs needs a VPC Service Controls perimeter, which is
organisation-scoped and belongs with the access context manager policy. **No
perimeter exists.** Related: `enable_private_service_connect` defaults to
`false` and no environment turns it on.

Until both exist, treat "this zone reaches only the Google APIs it needs" as
an aspiration and not a control.

## No public addresses is an organisation policy, not a subnet setting

This module creates no instances and no external addresses, and every subnet
has private Google access so that none is needed. It cannot stop somebody
attaching an external address to an instance in one of its subnets. The
control for that is `constraints/compute.vmExternalIpAccess`, which is
organisation-scoped.
