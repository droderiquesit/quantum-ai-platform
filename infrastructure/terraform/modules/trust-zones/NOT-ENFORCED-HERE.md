# What this module does not enforce

The trust-zone model in blueprint §46.1 is thirteen zones, default deny
between them, and an exhaustive list of what may leave the VPC. A network and
an IAM policy can hold some of that. This file is the rest, written down here
because a boundary a reader believes in and the platform does not have is
worse than a gap somebody is tracking.

Read it before citing this module as evidence for anything.

## Wired, and now planned — and the planning found two things

The module is instantiated from `infrastructure/terraform/main.tf` under ADR
0024, with the zones, paths, allowlist and ingress read from each
environment's tfvars. Every Cloud Run workload in the catalogue attaches to
its zone's subnet and carries its zone's network tag on its VPC interface, so
the rules here bind those instances.

This section said, until 2026-09-14, that `terraform` was not installed where
the module was written and that `fmt -check`, `validate` and `plan` had
therefore **never been run** against it — so every `lifecycle.precondition`
below was an assertion about an assertion. All three have now been run.
`tests/zone-model.tftest.hcl` is a real plan of this module against a mocked
provider: it needs no credential, reaches no project, creates nothing, and it
exercises every precondition here from both sides, which is what the
infrastructure rules ask for and what could not be produced against a torn-down
project any other way. `terraform test` from this directory runs it.

Two findings, because a precondition nobody has run is a precondition nobody
has tested:

  * **The Cloud NAT precondition could not fire in the case it was written
    for.** It refuses a zone that declares external egress from a region other
    than this module's, and both NAT resources counted on `local.nat_zones` —
    the egress zones *narrowed to this region*. One out-of-region zone empties
    that list, `count` goes to zero, and a precondition on a resource that is
    never planned is never evaluated: the zone would have received its egress
    firewall rules, no translation at all, and the message explaining exactly
    that would never have been printed. It fired only when a second, in-region
    zone happened to be declared beside it. Both counts now read
    `local.egress_zones`, so the resource is planned whenever any zone asks
    for egress and the precondition gets to speak.
  * **A sibling module's variable validation could not run at all.**
    `modules/network`'s `console_egress_cidr` guard handed a null to `split`,
    and Terraform evaluates both operands of `||`. That is not this module,
    but it is the same lesson and it is why this section changed: the value is
    null in three of the four environments, so `terraform plan` was impossible
    in all three, and a Rust acceptance test asserting the rule "fires and
    admits" passed throughout because it proved the rule by re-implementing
    the arithmetic in Rust. A mirror of an expression is not the expression.

The two-sided pair the infrastructure rules name specifically — an
`ibm-quantum` entry on any zone but `optimisation` refused, then the same
entry on `optimisation` admitted — is
`an_ibm_destination_on_cognition_is_refused` and
`an_ibm_destination_on_optimisation_is_admitted` in that file.

What a plan still cannot prove is an apply. Nothing in this module has been
created in any project; the dev environment was torn down on 2026-09-13 and
`docs/DELIVERY-STATUS.md` is the register of what remains. A mocked plan says
the configuration is coherent and the refusals fire. It does not say Google
accepted a single one of these resources.

## Four zones hold workloads; nine hold nothing

The catalogue places `qip-api` in `application-identity`, `qip-deepbrain` in
`cognition` and `qip-fastbrain` in `intelligence`, and — where an
environment turns them on — OpenObserve and the control plane's three
controllers in `management`. Count the placements rather than trusting this
sentence: `grep -c 'trust_zone *= *"' infrastructure/terraform/catalogue.tf`
is the catalogue's and OpenObserve's, and
`grep -c 'gitops_control_plane\[0\]\.[a-z]*_service_account_email,$' infrastructure/terraform/catalogue.tf`
is the control plane's — the trailing comma matters, because without it the
same pattern also finds the two places the deployer's email is handed to
`modules/cloudrun` and prints five. This heading said three until 2026-09-19, and
`docs/DELIVERY-STATUS.md` repeated it; the fourth zone had held OpenObserve
since ADR 0028 and the control plane's identities since ADR 0036, and only
OpenObserve was listed in `zone_identities` — so a `permitted_paths` entry
from `management` would have granted the dashboard's account and not the
deployer's. The control plane's cluster takes its subnet and its tag from
this module's outputs, so its identities are in the zone in every sense a
firewall can see; they are now placed in it in the one sense a grant can.
The root's `tests/zone-identities.tftest.hcl` plans the count.

The other nine zones, `optimisation` among them, have no workload, no
identity and — unless an environment declares a subnet for them — no subnet.
A zone with nothing in it constrains nothing. Note that `execution` is among
the nine on purpose rather than by oversight: `modules/execution-node` cuts
its own subnet and writes its own tag and rules, so a node is not on any
subnet this module creates, and its identity is deliberately not placed here
until the two modules share one boundary — an identity in a zone whose rules
do not reach its instance would be the paper boundary this file exists to
refuse.

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

## Identities are the root's, and one-zone-per-identity is not checked here

There is no zone service account. A zone's identities are the accounts of
the workloads the root places in it, passed in through `zone_identities`,
and the ledger and fabric grants are made to those. Whether one account
appears under two zones cannot be validated at plan time — the emails are not
known until the accounts exist — so the property rests on the root:
every catalogue workload names exactly one zone and `modules/cloudrun`
creates exactly one account per workload, and the management zone's four are
merged in by name. The acceptance suite asserts the first half by reading
`catalogue.tf`.

What *is* checked here, since 2026-09-19: an identity placed in a zone this
deployment did not declare is refused. The variable's description had
promised that refusal for as long as the variable existed, and the only
validation checked the thirteen names — so an identity under `execution` in
an environment with no execution subnet was admitted, earned no grant, sat
under no rule, and was listed in the root's output as if governed. A promise
in a description is a control that cannot fire. The second validation on
`zone_identities` is the promise made real; the harness plans it from both
sides and proves the admitted identity becomes a grant. The root refuses the
same mistake first, in `terraform_data.identities_are_placed`, and narrows
the map it hands over to declared zones — because a `terraform test` run
can expect a failure only from a root object, and a refusal no harness can
name is a refusal nobody has watched fire.

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
