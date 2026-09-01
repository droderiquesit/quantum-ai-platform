# This module is not called from anywhere, on purpose

`grep -rn '"./modules/cloudrun"' infrastructure/terraform` returns nothing, and
that is the intended state today. Nothing in this directory changes a plan, an
apply, or a running deployment.

The reason is ADR 0020. It fixes the order the Cloud Run migration takes and
requires recorded human approval naming each step before that step begins. Step
2 — one scale-to-zero warm service running on both substrates, with the Secret
Manager CSI equivalent proven on Cloud Run — has not been approved and has not
happened. Writing the module is not that step; instantiating it is.

Keeping the two apart is the whole point. A module and its wiring landing in
one change means the review that should have been about "may this workload move"
is also about volume mount syntax, and the second question is the one that gets
answered.

## What a later pass has to do, and in what order

Everything below is outside this directory and belongs to whoever is granted the
step. It is written down so that pass does not have to rediscover it.

1. **Enable the API.** `modules/services/main.tf` enumerates every API this
   configuration depends on, with the module that needs it named beside it.
   `run.googleapis.com` is not in that list today, because nothing in the tree
   creates a Cloud Run resource. It has to be added, or the first apply stops
   with `SERVICE_DISABLED` on the first Cloud Run resource in the plan.

2. **Give the workloads somewhere to attach.** This module takes
   `egress_network` and `egress_subnet` and does not create either. The console
   already has a subnet of its own — `modules/network`'s `console_egress`, keyed
   off `console_egress_cidr` — and the same argument applies here: a range GKE
   also allocates from fails in whichever of the two asks second, so the Cloud
   Run catalogue gets its own range rather than a share of the primary. Whether
   that is one shared subnet or one per trust zone is a decision that pass has
   to make and record; this module is indifferent.

3. **Instantiate one workload, not seventy.** ADR 0020's step 2 is a single
   warm service running on both substrates at once. A `for_each` over a
   catalogue map is the eventual shape and is the wrong shape for the step that
   proves the identity, secret and egress path works at all.

4. **Pass the image by digest.** `deploy.yml` already resolves digests for the
   images it signs, and `modules/registry` keeps tags immutable. The digest
   belongs in the plan; a variable holding a tag is refused by
   `var.image_digest`'s validation and should stay refused.

5. **Grant what the workload needs where the resource is.** This module grants
   `roles/monitoring.metricWriter`, `roles/logging.logWriter`, and
   `roles/secretmanager.secretAccessor` on exactly the secrets mounted — and
   takes no parameter for anything wider. A workload that needs the evidence
   bucket gets `roles/storage.objectCreator` in `modules/evidence`, against the
   `service_account_email` this module exports, in a file where the grant is
   visible next to the thing it grants on.

6. **Show the plan, both ways.** The infrastructure rules require a validation
   change to be proven by a real plan that refuses a bad value *and* admits a
   good one. The preconditions in `main.tf` — the public edge refusing a trading
   workload, the floor of zero refusing to be raised without a reason, the
   customer class refusing the venue credential — are exactly that kind of gate,
   and none of them has been exercised against a real Google provider yet.
   `terraform` is not installed in the environment this module was written in,
   so `terraform fmt -check` and `terraform validate` have not been run on it
   either. That pass runs them first, before it runs anything else.

## What this module deliberately does not do

- **It does not create the execution node.** §41.4's one permitted VM is a C3
  running bare under systemd, always on, with `isolcpus` pinned. It is not a
  Cloud Run workload and `var.plane` refuses to name it, so a workload cannot be
  filed under `execution` while running on a substrate that scales to zero.
- **It does not delete anything.** ADR 0020 retires the Helm chart, Argo CD and
  Kargo at step 5, on that step's evidence, with approval, and not before.
- **It does not make anything publicly reachable.** There is no input that
  produces `INGRESS_TRAFFIC_ALL`, and `allUsers` and `allAuthenticatedUsers` are
  refused as invokers. A workload on the customer edge is reached through the
  load balancer, which is where the identity check already lives.
