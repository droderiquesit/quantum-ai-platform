# OpenObserve's access posture — ADR 0033.
#
# ADR 0030 put an anonymous invoker on a service that held nothing, and wrote
# its own expiry into the record: the store stops being empty the moment a
# deployment sets `QIP_OPENOBSERVE_URL`, and that change must move the service
# behind Identity-Aware Proxy or re-argue the exposure. ADR 0033 is that
# condition firing. This module is where the choice is made, so that "who may
# read the platform's telemetry" is one value in one file rather than a
# posture a reader infers from a manifest's `ingress:` line.
#
# What this module does is decide and refuse. From `access_posture` it derives
# the ingress the RunService must carry and the `roles/run.invoker` members the
# manifest must name, and it refuses the anonymous posture for `prod` at plan
# time.
#
# What it does not do, written here because a reader who assumes otherwise is
# exactly the person this module would mislead:
#
#   * It creates nothing. There is no external load balancer, no serverless
#     NEG, no backend service and no IAP setting in this tree. ADR 0033 names
#     that infrastructure and prices it — "IAP is another moving part" — and
#     none of it has been built. Selecting `authenticated` does not configure
#     IAP; it declares that IAP is the way in and narrows the invoker set to
#     named principals, which is the half that can be expressed today.
#   * It writes no IAM binding. Since ADR 0036 decision 5 the invoker binding
#     left Terraform for `infrastructure/gitops/envs/<env>/invokers.yaml`, and
#     a module that wrote one would be Terraform and Config Connector both
#     claiming one object. What this module emits is the member list that
#     manifest must carry; a disagreement between them is the parity test's
#     finding, not a second source of truth.
#   * It has no caller yet. Nothing in the root instantiates it, so no plan
#     evaluates the refusal below and no environment's posture changes by this
#     file existing. `docs/ops/missing-infrastructure-register.md` records
#     that, and the root wiring is what closes it.

terraform {
  # The refusal below reads a second variable inside a `validation` block.
  # That is a 1.9 feature; on 1.8 it is not a weaker check, it is a syntax
  # error, and this line is what turns "the gate silently is not there" into
  # "the configuration will not load".
  required_version = ">= 1.9.0"
}

locals {
  authenticated = var.access_posture == "authenticated"

  # The RunService's `ingress`, as the manifest spells it.
  #
  # `INGRESS_TRAFFIC_INTERNAL_LOAD_BALANCER` under the authenticated posture,
  # because IAP sits on the load balancer: a service that still answers on its
  # own `run.app` URL has a route around the check, and an authentication
  # layer with a bypass is the control that reads as protection and is not.
  # `INGRESS_TRAFFIC_ALL` is ADR 0030's value and stays reachable by anyone.
  run_service_ingress = local.authenticated ? "INGRESS_TRAFFIC_INTERNAL_LOAD_BALANCER" : "INGRESS_TRAFFIC_ALL"

  # Who may invoke the service, for the manifest to name.
  #
  # Under the authenticated posture: the principals the caller named, and
  # nothing else — `access_principals` refuses the two anonymous ones, so no
  # value of that input can widen this list past named identities.
  #
  # Under the anonymous posture this module names nobody. ADR 0030's binding
  # is the manifest's and stays there under the record that argued it; this
  # module's `access_posture` output is how a reader and a parity test tell
  # the two postures apart without a second copy of the principal.
  invoker_members = local.authenticated ? var.access_principals : []
}
