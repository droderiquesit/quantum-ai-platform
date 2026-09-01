# What this module does not enforce

The trust-zone model in blueprint §46.1 is thirteen zones, default deny
between them, and an exhaustive list of what may leave the VPC. A network and
an IAM policy can hold some of that. This file is the rest, written down here
because a boundary a reader believes in and the platform does not have is
worse than a gap somebody is tracking.

Read it before citing this module as evidence for anything.

## Not wired

The module is not instantiated. `infrastructure/terraform/main.tf` does not
reference it, so it creates nothing, changes no plan and constrains no
deployment today. That was deliberate: wiring it moves live traffic between
subnets and belongs in a pass that carries its own plan as evidence rather
than riding on this one. §Wiring below says what that pass has to do.

Consequently every claim in this file about what the module refuses is a claim
about what it will refuse once wired, and a claim about a plan nobody has run
yet — see §Not validated.

## Not validated

`terraform` is not installed in the environment this module was written in, so
`terraform fmt -check` and `terraform validate` have **not been run against
it**, and neither has `terraform plan`. The first person with a Terraform
binary should run all three before trusting any of the refusals below, and
should start with the `lifecycle.precondition` blocks in `main.tf`: those are
where the zone model is enforced, and an unrun precondition is an assertion
about an assertion.

## The zones constrain no workload yet

Nothing in `infrastructure/kubernetes/` carries a zone identity or a zone
network tag. The tags this module exports are applied by whoever creates a
node pool, and no node pool applies one today. Until that happens the rules
here bound subnets that hold nothing.

The sharpest case is Optimisation. There is no optimisation workload, identity
or pod label anywhere in `infrastructure/` — the IBM Quantum ports (9103,
9104) are currently granted to `qip-deepbrain` in
`infrastructure/kubernetes/base/egress.yaml`, the research workload that also
holds Vertex, Cloud Storage and BigQuery egress. The blueprint's narrowest
zone is merged into its broadest. This module encodes Optimisation as a
separate zone with its own identity, and makes `ibm-quantum` a purpose no
other zone can declare, which is the shape the platform should converge on —
but until a workload actually runs as the optimisation zone's service account
inside the optimisation zone's subnet, **the IBM-only constraint binds
nothing**. It is a control that cannot fire. Do not report it as one that can.

Worse, the egress proxy that the current IBM allowlist depends on is itself
not running: in `egress.yaml` the ServiceAccount, Deployment and
PodDisruptionBudget for `qip-egress` are commented out, and the file says so
and names the acceptance tests that would fail. No pod carries the
`app: qip-egress` label, so the Service selects nothing and the
NetworkPolicies naming it select nothing either. The host and path allowlist
pinning `quantum.cloud.ibm.com` and `api.quantum.ibm.com` is a ConfigMap
nothing mounts. Against §46.1 the IBM-only constraint is absent today, not
merely weak, and this module does not change that until it is wired and a
workload is placed.

## Firewall rules are about addresses, not intent

A firewall rule permits a TCP connection between two ranges on a port. It
cannot tell a read from a write, a query from a mutation, or an intent from a
command. The `mode` on a permitted path therefore does two things: it drives
the ledger and control-fabric IAM grants, where the distinction is real, and
it documents the path everywhere else.

Specifically not enforced by any rule here:

- **`intent` really meaning intent.** The application and identity zone may
  raise an intent with Intelligence and Treasury and may not command them.
  That is an application property, held by the API those zones expose. The
  rules here permit the connection and say nothing about what travels over it.
- **`append` really meaning append.** Spanner has no append-only role. A zone
  with an `append` path to the ledger receives `roles/spanner.databaseUser`,
  which can update and delete as well. Append-only is a property of the schema
  and the application. The grant is named honestly at the resource.
- **`read` on a path to a zone whose service does not distinguish.** If the
  destination exposes one port that both reads and writes, the mode is a
  comment.

## The wallet and treasury separation is two things, and this module holds one

The wallet read path and the treasury write path get different service
accounts, different subnets, and no sanctioned path between them in either
direction. That much is structural: there is no expression in this module that
gives them one identity, one range or one route.

**The property that actually matters is not in this module at all.** "Wallet
code cannot link signing code" is a compile-time fact about the Rust
workspace, established by the read path not depending on the signing crate and
verified by dependency audit. A binary that linked both would violate it while
passing every rule here, because a network cannot see a linker. If you are
looking for the enforcement of that invariant, look at the dependency graph,
not at a VPC.

## Per-API restriction needs a perimeter this repository does not have

Private Google access sends `*.googleapis.com` to `199.36.153.8/30`, and a
firewall rule or a network policy cannot tell `secretmanager.googleapis.com`
from `aiplatform.googleapis.com` — they are the same four addresses and the
same port. The Kubernetes manifests admit this about themselves already.

Restricting a zone to particular Google APIs needs a VPC Service Controls
perimeter. **No perimeter exists**: no `.tf` file in this repository declares
`google_access_context_manager_service_perimeter`, and only prose mentions
one. A perimeter is organisation-scoped and belongs with the access context
manager policy, not inside a per-environment network module, so this module
does not attempt it and could not honestly claim it.

Related, and a prerequisite for the private path being used at all:
`enable_private_service_connect` defaults to `false` in
`infrastructure/terraform/variables.tf` and no environment turns it on. PSC is
declared and never enabled.

Until both exist, treat "this zone reaches only the Google APIs it needs" as
an aspiration and not a control.

## No public addresses is an organisation policy, not a subnet setting

This module creates no instances and no external addresses, and every subnet
has private Google access so that none is needed. It cannot stop somebody
attaching an external address to an instance in one of its subnets. The
control for that is the organisation policy constraint
`constraints/compute.vmExternalIpAccess`, which is organisation-scoped in the
same way the SCC module's caveats are.

## Wiring

For the pass that instantiates this module. Every item is a change to a file
this module's author did not own:

1. `infrastructure/terraform/main.tf` — a `module "trust_zones"` block with
   `source = "./modules/trust-zones"`, `network_id = module.network.network_id`,
   `region = var.region`, and `depends_on = [module.services]` so nothing is
   created before its API is on.
2. `infrastructure/terraform/variables.tf` — `trust_zones`, `permitted_paths`,
   `external_egress` and `public_ingress` variables, passed straight through.
   Ranges belong in `infrastructure/environments/<env>/terraform.tfvars`, not
   in a default: an address range chosen as a convenience is the one that
   collides.
3. Address plan first. Thirteen subnets need thirteen ranges that overlap
   neither each other nor the primary, pod, service, console-egress or
   edge-cell ranges already allocated. That is the work item most likely to be
   underestimated.
4. Node pools and workloads: apply the exported zone network tag to the pool
   and the exported Kubernetes service account name to the workload, or the
   rules bound nothing. Start with Optimisation, because that is the zone
   whose absence currently gives IBM egress to the research workload.
5. Kubernetes network policies mirroring the `permitted_paths` output, so the
   pod-level boundary and the subnet-level boundary say the same thing.
6. Run `terraform plan` and show it: one plan with a deliberately bad value
   proving the gate refuses it — an `ibm-quantum` entry on any zone but
   optimisation is the cheapest — and one with a good value proving it admits
   that. A gate only proven to refuse is indistinguishable from a gate that
   refuses everything.
