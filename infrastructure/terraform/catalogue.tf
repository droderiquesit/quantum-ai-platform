# The Cloud Run catalogue: every warm binary the platform deploys, and where.
#
# One entry per workload, and one place. The Helm chart used to spread each
# workload across a Deployment, a Service, a ConfigMap, a SecretProviderClass
# and two NetworkPolicies, and every seam between those files was a place the
# two sides drifted — the acceptance suite's `manifest_wiring.rs` records
# three cases that shipped. Here a workload is one map entry: the binary it
# runs, the plane and trust zone it belongs to, whose traffic it carries, the
# variables it reads, and the secrets it mounts as files. `modules/cloudrun`
# holds everything that must be true of all of them.
#
# Three binaries today, as ADR 0010 records. Roughly seventy is the blueprint's
# count for the finished catalogue; an entry is added here when the binary
# exists, and the acceptance suite refuses an entry naming one that does not.
#
# What is deliberately not here:
#
#   * `qip-edge-node`. It is the execution node, runs bare under systemd on a
#     C3, and is `modules/execution-node`. `modules/cloudrun` refuses the
#     `execution` plane by name for that reason.
#   * `QIP_MESH_CELLS` on the API. The in-tree mesh binds one listener per
#     cell on its own port, and a Cloud Run service exposes exactly one port.
#     Unset, `qip-api` builds no mesh and `/api/v1/mesh` answers
#     `available: false` — which is the honest state, because no execution
#     node exists to speak it. The blueprint's control fabric is Pub/Sub
#     (§46.1); wiring the centre-to-node path on this runtime is that work,
#     recorded in ADR 0024, and not a port that cannot be published.
#   * A job. The chart carried no CronJob or Job, so there is none to move.
#     `modules/cloudrun` takes `kind = "job"` for the day there is.

locals {
  # The listener every workload with a proxy reaches Google APIs through.
  # Read from the proxy module so the port is written once, in the bootstrap.
  gcp_endpoint = module.egress_proxy.endpoints["gcp"]

  # The instrument universe the three central roots assemble the desk from,
  # read at start-up from `QIP_UNIVERSE_PATH`. Configuration, not a secret,
  # and committed: it is read here with `file()` so the bytes a revision
  # mounts are the bytes in the reviewed commit, and `modules/cloudrun` names
  # the object by their hash, so the answer to "which universe did that
  # revision trade" is in the plan. Read once, here, and handed to every
  # entry that reads it; the roots' default for the variable is the path the
  # module writes, `/etc/qip/universe.json`, so a root run outside this
  # catalogue reads the same file at the same place.
  universe_catalogue = file("${path.module}/../../data/datasets/universe.json")

  cloud_run_catalogue = {
    # The API and the operator interface. Customer traffic, reached only by
    # the console's identity, in the application-and-identity zone (§46.1):
    # it may read the ledger and raise intents, and may never reach a node, a
    # venue, a QPU or a key.
    api = {
      binary        = "qip-api"
      plane         = "experience-and-identity"
      trust_zone    = "application-identity"
      traffic_class = "customer"
      health_path   = "/api/v1"
      cpu           = "2"
      memory        = "1Gi"
      concurrency   = 80
      # Scales, because it is the one workload that can: it cycles only when
      # asked (`POST /cycle`) and serves stateless reads the rest of the time.
      # What a second instance costs it — per-process rate-limit counters,
      # the cell registry — is written down in
      # docs/operations/scaling-and-availability.md and is a degradation, not
      # a corruption.
      min_instances           = 0
      max_instances           = 4
      always_on_justification = ""
      # No collector. The API serves `/metrics` behind `Role::Monitor`, so a
      # sidecar with no token would be answered 401 every thirty seconds
      # and chart nothing; its cycle and orders are recorded by the brains'
      # registries, which are the ones scraped.
      metrics_collector = false
      # The audit chain's Cloud Storage adapter needs the proxy. Nothing in
      # `qip-api` reads `QIP_GCP_ENDPOINT` yet — `qip_storage::gcp` does, and
      # the composition root that constructs it is the change that sets the
      # variable. The proxy is attached now so that change is one line.
      egress_proxy = true
      invokers = compact([
        module.secrets.console_service_account_email == null ? "" : "serviceAccount:${module.secrets.console_service_account_email}",
      ])
      env = {
        QIP_API_ADDRESS    = "0.0.0.0:8080"
        QIP_STORAGE_TARGET = var.storage_target
        # The autonomy ceiling, from the one root variable whose validation
        # refuses the three live rungs at plan time. Every workload takes it
        # from here and never from a literal, so lowering or raising it is a
        # change to one reviewed value that appears in a diff.
        QIP_AUTONOMY_CEILING = var.autonomy_ceiling
      }
      config_files = {
        universe = {
          content           = local.universe_catalogue
          file_name         = "universe.json"
          content_type      = "application/json"
          env_file_variable = "QIP_UNIVERSE_PATH"
        }
      }
      secret_mounts = {
        token-operator = {
          secret_id         = module.secrets.secret_ids["qip-token-operator"]
          file_name         = "token-operator"
          env_file_variable = "QIP_TOKEN_OPERATOR_FILE"
        }
        token-approver = {
          secret_id         = module.secrets.secret_ids["qip-token-approver"]
          file_name         = "token-approver"
          env_file_variable = "QIP_TOKEN_APPROVER_FILE"
        }
        token-analyst = {
          secret_id         = module.secrets.secret_ids["qip-token-analyst"]
          file_name         = "token-analyst"
          env_file_variable = "QIP_TOKEN_ANALYST_FILE"
        }
        token-viewer = {
          secret_id         = module.secrets.secret_ids["qip-token-viewer"]
          file_name         = "token-viewer"
          env_file_variable = "QIP_TOKEN_VIEWER_FILE"
        }
        token-monitor = {
          secret_id         = module.secrets.secret_ids["qip-token-monitor"]
          file_name         = "token-monitor"
          env_file_variable = "QIP_TOKEN_MONITOR_FILE"
        }
        # The key this process signs capital envelopes with. Every node
        # verifies grants against it, so the centre signing with anything
        # else produces grants no node accepts.
        capital-envelope-key = {
          secret_id         = module.secrets.secret_ids["qip-capital-envelope-key"]
          file_name         = "capital-envelope-key"
          env_file_variable = "QIP_CAPITAL_ENVELOPE_KEY_FILE"
        }
      }
    }

    # The fast path: market data, microstructure, real-time risk, execution
    # against the simulator. Trading traffic, reachable from inside the VPC
    # only, and the one workload that could ever hold the venue credential —
    # `modules/secrets` binds it to this identity where the ceiling permits,
    # which no environment a plan can carry does.
    #
    # No egress proxy, deliberately. ADR 0008, consequence 3: nothing on the
    # hot path consults a model. The fast brain links `qip-ai` transitively
    # through `qip-kernel` and `qip-agents`, so what stops it calling one is
    # its start-up roster check and the fact that it can reach nothing that
    # serves one. Port 9102 on the proxy is exactly such a thing.
    fastbrain = {
      binary        = "qip-fastbrain"
      plane         = "capital-and-risk"
      trust_zone    = "intelligence"
      traffic_class = "trading"
      health_path   = "/health"
      cpu           = "2"
      memory        = "2Gi"
      # Holds per-process state: one instance's cycle is its own.
      concurrency  = 1
      egress_proxy = false
      invokers     = []
      # Exactly one instance, always. This binary opens the event log and
      # runs the cycle on its own clock (`QIP_FASTBRAIN_CYCLE_INTERVAL_MS`);
      # two instances would each run the cycle and each append to the same
      # hash-chained log, and a fork in the chain is the corruption the chain
      # exists to detect, not one it tolerates. The ceiling of one makes that
      # structural. The floor of one is the other half: nothing calls this
      # service — no scheduler, no invoker, and `POST /cycle` is the API's
      # own route — so an instance Cloud Run retired for want of a request
      # would never be started again and the cycle would simply stop. A
      # floor also keeps the CPU allocated between requests, which a loop
      # that never receives one needs.
      min_instances           = 1
      max_instances           = 1
      always_on_justification = "Runs the cycle on its own clock over one hash-chained log; nothing requests it, so a retired instance is a stopped cycle and a second one is a forked chain."
      # Scraped, once a collector digest is pinned: the kill-switch gauge,
      # the limit breaches and the order counters every central alert
      # policy queries are recorded here.
      metrics_collector = true
      env = merge(
        {
          QIP_FASTBRAIN_HEALTH_ADDRESS = "0.0.0.0:8080"
          QIP_STORAGE_TARGET           = var.storage_target
          QIP_AUTONOMY_CEILING         = var.autonomy_ceiling
        },
        # The live market-data connector, or nothing. Both keys or neither:
        # `connector_feed` refuses half a configuration by name rather than
        # falling back, and the root variable's type makes half impossible.
        # Absent, the node runs the synthetic exchange, which is what every
        # environment does today — nothing starts fetching a vendor because
        # this catalogue was applied.
        var.market_data_connector == null ? {} : {
          QIP_CONNECTOR_SOURCE   = var.market_data_connector.source
          QIP_CONNECTOR_BASE_URL = var.market_data_connector.base_url
        },
      )
      config_files = {
        universe = {
          content           = local.universe_catalogue
          file_name         = "universe.json"
          content_type      = "application/json"
          env_file_variable = "QIP_UNIVERSE_PATH"
        }
      }
      secret_mounts = {
        # The capital-envelope key, as a file. Absent, this process runs on
        # the seed-derived default — reproducible, mintable by anyone who
        # knows the seed, and refused outright once the ceiling permits live
        # trading.
        capital-envelope-key = {
          secret_id         = module.secrets.secret_ids["qip-capital-envelope-key"]
          file_name         = "capital-envelope-key"
          env_file_variable = "QIP_CAPITAL_ENVELOPE_KEY_FILE"
        }
      }
    }

    # The research workload: world model, discovery, reasoning, simulation,
    # learning. Cognition zone. It is the one workload that may call a
    # language model (ADR 0008), hosts the training port that reaches Vertex,
    # and holds the analytical and evidence stores — so it carries the proxy.
    # Its zone may hold no external-egress entry at all, so the IBM listeners
    # its sidecar declares reach nothing; `modules/trust-zones/NOT-ENFORCED-HERE.md`.
    deepbrain = {
      binary        = "qip-deepbrain"
      plane         = "cognition"
      trust_zone    = "cognition"
      traffic_class = "platform"
      health_path   = "/health"
      cpu           = "4"
      memory        = "8Gi"
      concurrency   = 1
      egress_proxy  = true
      invokers      = []
      # One instance, for the fast brain's reason: this binary opens the
      # event log and runs its loop on `QIP_CYCLE_INTERVAL_SECONDS` with
      # nothing to wake it, so a second instance is a second writer of the
      # same evidence and a zero floor is a research loop that ran until the
      # first idle retirement and never again.
      min_instances           = 1
      max_instances           = 1
      always_on_justification = "Runs the intelligence loop on its own clock over one hash-chained log; nothing requests it, so a retired instance is a stopped loop and a second one is a forked chain."
      # Scraped, once a collector digest is pinned, for the same series the
      # fast brain records from its own cycle.
      metrics_collector = true
      env = {
        QIP_DEEPBRAIN_HEALTH_ADDRESS = "0.0.0.0:8080"
        QIP_STORAGE_TARGET           = var.storage_target
        QIP_AUTONOMY_CEILING         = var.autonomy_ceiling
        QIP_CYCLE_INTERVAL_SECONDS   = var.cycle_interval_seconds
      }
      config_files = {
        universe = {
          content           = local.universe_catalogue
          file_name         = "universe.json"
          content_type      = "application/json"
          env_file_variable = "QIP_UNIVERSE_PATH"
        }
      }
      secret_mounts = {
        capital-envelope-key = {
          secret_id         = module.secrets.secret_ids["qip-capital-envelope-key"]
          file_name         = "capital-envelope-key"
          env_file_variable = "QIP_CAPITAL_ENVELOPE_KEY_FILE"
        }
      }
    }
  }

  # Each zone's identities, for the ledger and fabric grants in
  # modules/trust-zones: the accounts of the workloads placed there.
  zone_identities = {
    for zone in distinct([for workload in local.cloud_run_catalogue : workload.trust_zone]) :
    zone => sort([for name, workload in module.cloud_run : workload.service_account_email if workload.trust_zone == zone])
  }
}

# The plan refuses a catalogue that is not fully placed.
#
# Preconditions on a `terraform_data` rather than in the module, because the
# facts they check are the root's: whether the zone a workload names is one
# this environment declared a subnet for, and whether the pipeline has ever
# produced a digest for the binary. A lookup that simply failed would report
# an invalid index; these report the decision that is missing.
resource "terraform_data" "catalogue_is_placed" {
  input = sort(keys(local.cloud_run_catalogue))

  lifecycle {
    precondition {
      condition = alltrue([
        for workload in values(local.cloud_run_catalogue) : contains(keys(var.trust_zones), workload.trust_zone)
      ])
      error_message = "A catalogue workload names a trust zone this environment does not declare in `trust_zones`: ${join(", ", distinct([for workload in values(local.cloud_run_catalogue) : workload.trust_zone if !contains(keys(var.trust_zones), workload.trust_zone)]))}. A workload with no zone has no subnet, no tag and no rule; declare the zone's range in the tfvars."
    }

    precondition {
      condition = alltrue([
        for workload in values(local.cloud_run_catalogue) : contains(keys(var.image_digests), workload.binary)
      ])
      error_message = "No digest is recorded for ${join(", ", [for workload in values(local.cloud_run_catalogue) : workload.binary if !contains(keys(var.image_digests), workload.binary)])}. A service is created at the digest deploy.yml last attested; run the pipeline for this environment, which writes infrastructure/environments/<env>/images.tfvars."
    }

    # The fast path carries no proxy. Said here as well as in the catalogue
    # entry, because the entry is a value somebody edits and this is a plan
    # that stops.
    precondition {
      condition     = !local.cloud_run_catalogue.fastbrain.egress_proxy
      error_message = "The fast brain has been given the egress proxy. Port 9102 on it is a route to a language model API, and nothing on the hot path may consult a model (ADR 0008)."
    }
  }
}

module "cloud_run" {
  source   = "./modules/cloudrun"
  for_each = local.cloud_run_catalogue

  # Nothing here can be created before its API is on. See module "services".
  depends_on = [module.services]

  project_id  = var.project_id
  region      = var.region
  environment = var.environment
  labels      = local.labels

  name          = each.key
  kind          = "service"
  plane         = each.value.plane
  trust_zone    = each.value.trust_zone
  traffic_class = each.value.traffic_class

  # Internal, every one of them. The console reaches the API as a named
  # invoker over the VPC; nothing here has a URL the internet may ask for.
  ingress_posture = "internal"
  invokers        = each.value.invokers

  # The image, from the registry the pipeline pushes to, at the digest the
  # pipeline last attested for this environment. A missing digest is refused
  # by the module's validation and named by the precondition above.
  image_digest = "${module.registry.image_prefix}/${each.value.binary}@${lookup(var.image_digests, each.value.binary, "")}"

  # Placed in its trust zone: the zone's subnet is the interface, the zone's
  # tag is what every rule in modules/trust-zones targets.
  egress_network = module.network.network_id
  egress_subnet  = lookup(module.trust_zones.zone_subnets, each.value.trust_zone, null)
  network_tags   = compact([lookup(module.trust_zones.zone_network_tags, each.value.trust_zone, "")])

  cpu            = each.value.cpu
  memory         = each.value.memory
  concurrency    = each.value.concurrency
  container_port = 8080
  health_path    = each.value.health_path

  # Instance bounds, from the entry rather than the module's defaults. A
  # workload that runs the cycle over one journal is pinned to one instance
  # and kept warm; the module's precondition refuses a floor above zero that
  # the entry does not justify in writing.
  min_instances           = each.value.min_instances
  max_instances           = each.value.max_instances
  always_on_justification = each.value.always_on_justification

  env           = each.value.env
  secret_mounts = each.value.secret_mounts
  config_files  = each.value.config_files

  egress_sidecar = each.value.egress_proxy ? module.egress_proxy.sidecar : null

  # The managed-Prometheus collector, for the workloads that ask for one and
  # only once the root names a digest. Composed here from the registry
  # prefix and the bare digest, so the only image a plan can carry is the
  # mirrored, attested copy; null — the state of every environment today —
  # is no sidecar and `metrics_collected = false`.
  collector_image_digest = each.value.metrics_collector && var.metrics_collector_image_digest != null ? "${module.registry.image_prefix}/vendor/cloud-run-gmp-sidecar@${var.metrics_collector_image_digest}" : null

  # deploy.yml moves the service, as this account, and needs to act as the
  # service's own identity to create a revision.
  deployer_service_account = module.cicd.service_account_email
}

# The hash of the universe every central workload was given, so a person can
# say which committed catalogue a plan carries without reading the file out
# of a bucket. Beside the catalogue rather than in outputs.tf because the
# local it hashes is declared here, and a hash a file away from the bytes it
# names is a pair that drifts.
output "universe_catalogue_sha256" {
  description = "sha256 of the committed instrument universe mounted at /etc/qip/universe.json on the api, fastbrain and deepbrain workloads."
  value       = sha256(local.universe_catalogue)
}
