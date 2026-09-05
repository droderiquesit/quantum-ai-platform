# The Google APIs this configuration uses.
#
# Every other module in this repository assumed its API was already on. That
# assumption fails exactly once per project, and it fails in the worst possible
# place: partway through the first apply, after the VPC and its subnets exist,
# with a `SERVICE_DISABLED` error naming an API rather than anything about what
# was being built. Half a network and no cluster is a state somebody then has
# to reason about under time pressure.
#
# Nothing here is behind an on/off flag, for the same reason the Binary
# Authorization module is not: the off position of that switch is the gap. An
# API that is already enabled is adopted by this resource without a call, so
# managing them costs an existing project nothing and saves a new one an
# interrupted apply.
#
# What this cannot do is bootstrap itself. See BOOTSTRAP.md.

locals {
  # Always. Each of these has at least one resource in this configuration that
  # cannot be created without it, named so that deleting a module and leaving
  # its API behind is visible rather than inferred.
  always = {
    # `google_project_service` itself, and every other Service Usage call. Held
    # here so that a later `terraform destroy` cannot leave the project unable
    # to manage its own services.
    "serviceusage.googleapis.com" = "this module, and every API enablement in it"
    # Every `google_project_iam_member` in the configuration, and the project
    # lookups the provider performs before most of them.
    "cloudresourcemanager.googleapis.com" = "project-level IAM bindings"
    # The service accounts: one per Cloud Run workload, one per execution
    # node, the pipeline's two, and the console's.
    "iam.googleapis.com" = "google_service_account, in cloudrun/ execution-node/ cicd/ secrets/"
    # Short-lived credentials. The pipeline impersonates its account rather
    # than holding a key, and the exchange happens here.
    "iamcredentials.googleapis.com" = "workload identity federation and impersonation"
    # The GitHub OIDC token exchange in modules/cicd. Without it the pool and
    # provider exist and no token can be redeemed against them.
    "sts.googleapis.com" = "the workload identity pool's token exchange"
    # The VPC, its subnets, every firewall rule, the routers and NATs, the
    # execution nodes' instance templates and groups, and the interconnect
    # attachments when they are on. Direct VPC egress for Cloud Run is a
    # Compute interface too, so this is what the catalogue attaches through.
    "compute.googleapis.com" = "modules/network, modules/trust-zones, modules/execution-node, modules/connectivity, and Cloud Run's direct VPC egress"
    # Every service in the catalogue, and the jobs when there are any.
    "run.googleapis.com" = "modules/cloudrun, instantiated from catalogue.tf"
    # The private zone that sends `*.googleapis.com` to the restricted VIP,
    # so a workload with no external address reaches Google APIs without a
    # route to anything else.
    "dns.googleapis.com" = "the googleapis private zone in modules/network"
    # The key ring and every customer-managed key hanging off it.
    "cloudkms.googleapis.com" = "modules/secrets, and the keys other modules create in its ring"
    # The secrets, created empty.
    "secretmanager.googleapis.com" = "modules/secrets"
    # The one topic in the platform: Secret Manager will not accept a rotation
    # schedule without somewhere to announce a rotation is due.
    "pubsub.googleapis.com" = "the secret-rotation topic in modules/secrets"
    # The image repository the pipeline pushes to and Cloud Run pulls from.
    "artifactregistry.googleapis.com" = "modules/registry"
    # The evidence bucket, the egress proxy's bootstrap bucket, and the
    # training and archive buckets when they are on.
    "storage.googleapis.com" = "modules/evidence, modules/egress-proxy, modules/data, modules/ai"
    # The alerting policies, and every `metricWriter` binding.
    "monitoring.googleapis.com" = "modules/observability, and the telemetry bindings"
    # Every `logWriter` binding, and the execution node's Ops Agent.
    "logging.googleapis.com" = "the telemetry bindings"
    # The policy every Cloud Run revision is evaluated against.
    "binaryauthorization.googleapis.com" = "modules/binaryauthorization, evaluated by every service in catalogue.tf"
    # The note an attestation is attached to. The other half of the same pair.
    "containeranalysis.googleapis.com" = "the Container Analysis note in modules/binaryauthorization"
  }

  # Conditional, keyed the same way and merged in only when the flag that
  # creates the resource is set. An API enabled for a service nothing uses is
  # a quota surface and an audit-log stream nobody reads.
  optional = merge(
    var.enable_bigquery ? { "bigquery.googleapis.com" = "the research dataset in modules/data" } : {},
    var.enable_alloydb ? { "alloydb.googleapis.com" = "the AlloyDB cluster in modules/data" } : {},
    var.enable_bigtable ? { "bigtableadmin.googleapis.com" = "the Bigtable instance in modules/data" } : {},
    var.enable_memorystore ? { "redis.googleapis.com" = "the Memorystore instance in modules/data" } : {},
    var.enable_spanner ? { "spanner.googleapis.com" = "the Spanner instance in modules/data" } : {},
    var.enable_vertex_ai ? { "aiplatform.googleapis.com" = "modules/ai" } : {},
    # AlloyDB and Memorystore are reachable only over VPC peering, and the
    # peering is a Service Networking connection rather than a Compute one.
    (var.enable_alloydb || var.enable_memorystore) ? { "servicenetworking.googleapis.com" = "the private services access peering in modules/data" } : {},
    # Findings, custom detectors and mute configurations. See modules/scc:
    # this enables the project's half of a service whose activation is an
    # organisation-level act.
    var.enable_security_command_center ? { "securitycenter.googleapis.com" = "modules/scc" } : {},
    # The control-plane cluster (ADR 0036) and the two APIs a workflow reaches
    # its private endpoint through: the fleet it is registered to, and the
    # Connect gateway that proxies a kubectl call to it as an authenticated
    # Google API request rather than a route into the VPC.
    var.enable_gitops ? {
      "container.googleapis.com"      = "the control-plane cluster in modules/gitops-control-plane"
      "gkehub.googleapis.com"         = "the fleet membership the cluster registers with, in modules/gitops-control-plane"
      "connectgateway.googleapis.com" = "infra.yml's bootstrap step, which applies the controller manifests through the gateway"
    } : {},
  )

  services = merge(local.always, local.optional)
}

resource "google_project_service" "platform" {
  for_each = local.services

  project = var.project_id
  service = each.key

  # Never turn an API off because this configuration stopped needing it.
  #
  # Disabling `compute.googleapis.com` does not merely revoke access: Google
  # deletes every Compute resource in the project, including the ones this
  # configuration never created. In a project shared with anything else — a
  # sandbox VM, another team's load balancer, a bastion somebody depends on —
  # a `terraform destroy` aimed at this platform becomes somebody else's
  # outage, with no warning in the plan, because the plan shows one API being
  # disabled rather than the resources that go with it.
  #
  # The variable exists because a genuinely disposable per-change project is a
  # real thing to want. Its default is the one that cannot cause that outage.
  disable_on_destroy = var.disable_services_on_destroy

  # And never cascade. `disable_dependent_services` widens the blast radius of
  # the above from "the API named here" to "every API Google considers to
  # depend on it", which is a set this configuration does not enumerate and
  # cannot review.
  disable_dependent_services = false
}
