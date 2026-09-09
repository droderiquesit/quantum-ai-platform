# The Cloud Run catalogue: every warm binary the platform deploys, and where.
#
# One entry per workload, and one place. The Helm chart used to spread each
# workload across a Deployment, a Service, a ConfigMap, a SecretProviderClass
# and two NetworkPolicies, and every seam between those files was a place the
# two sides drifted — the acceptance suite's `manifest_wiring.rs` records
# three cases that shipped. Here a workload is one map entry: the binary it
# runs, the plane and trust zone it belongs to, whose traffic it carries, the
# variables it reads, and the secrets it mounts as files.
#
# Since ADR 0036 the entry is the source of truth for two artefacts rather
# than one. `modules/cloudrun` creates the workload's identity, its grants
# and the buckets its files are published to; the Cloud Run service itself
# is a Config Connector `RunService` manifest under
# `infrastructure/gitops/envs/<env>/`, reconciled by Argo CD on the
# control-plane cluster, and every value below that the module no longer
# consumes — the ingress posture, the invokers, the instance floor and
# ceiling, the CPU and memory, the health path, whether the proxy rides
# beside it — is what the acceptance suite's parity test holds the manifest
# to. An entry is edited here; a manifest that disagrees with it fails the
# build.
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
#     The day there is, it is a `RunJob` manifest beside the services and an
#     identity from the same module.

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

  # The committed files an environment may mount *beside* the universe, keyed
  # by the workload that reads them, and the one rule that governs them: a file
  # exists here only where a root variable names a path in this repository, and
  # a null variable renders no entry at all — so the process sees no variable
  # and takes the absent behaviour it documents, rather than opening a file
  # somebody left empty to make a mount work.
  #
  # Read with `file()` like the universe, for the same reason: the bytes a
  # revision mounts are the bytes in the reviewed commit, and `modules/cloudrun`
  # names the object by their hash, so a plan says which document a revision
  # was given.
  #
  # Declared out here rather than inside the entry because the entry's
  # `config_files` block is a parity contract: every `env_file_variable` in it
  # is one every `RunService` under `gitops/envs/<env>/` must carry, and these
  # are carried by none of them while every tfvars leaves their variables null.
  # The entry merges this map in, so what the module receives is still one map;
  # what a rendered manifest omits is exactly what that environment did not ask
  # for.
  #
  # This said "both variables" while there were two, and the capital-fabric
  # declaration made it three on 2026-09-08. The count is deliberately not
  # written down again: `grep -c env_file_variable` over this local answers it,
  # and a number in a comment is a number that goes stale without ever becoming
  # false enough for anybody to notice. What holds the contract is the
  # acceptance suite's parity test, not this paragraph.
  optional_config_files = {
    api = merge(
      # The venue registrations the API's registry ships with (ADR 0034,
      # ADR 0040). Unset, the shipped table records nobody and every source
      # that needs an account stays refused — the honest state of a deployment
      # where nobody has registered. Setting it is the last step of
      # docs/operations/registering-a-venue.md and not a substitute for the
      # steps before it.
      var.venue_registrations_file == null ? {} : {
        venue-registrations = {
          content           = file("${path.module}/../../${var.venue_registrations_file}")
          file_name         = "venue-registrations.json"
          content_type      = "application/json"
          env_file_variable = "QIP_VENUE_REGISTRATIONS_PATH"
        }
      },
      # The custodian's wallet statement the LEARN stage reconciles against.
      # Unset, the API's banner says there is no feed and /wallet answers
      # `assembled: false`. Set, it is a dated document: the kernel holds a
      # statement fresh for one day and the root refuses a stale one at
      # start-up, so committing one is a same-day act by a person who has a
      # custodian to quote. Every environment trades on the in-process
      # simulated venue (ADR 0003), which issues no statement, so every
      # environment leaves this null and says so in its tfvars.
      var.wallet_statement_file == null ? {} : {
        wallet-statement = {
          content           = file("${path.module}/../../${var.wallet_statement_file}")
          file_name         = "wallet-statement.json"
          content_type      = "application/json"
          env_file_variable = "QIP_WALLET_STATEMENT_PATH"
        }
      },
      # The desk's capital-fabric declaration: the destinations, corridors and
      # transfer intents §37 and §38 are about, and the only production route
      # by which any of them reaches the chain. Unset, the API's banner says
      # nothing is declared and /transfer-gate answers `last_assessment: null`
      # — which was every deployment's answer before this mount existed, not
      # because the desk had declared nothing but because no caller could
      # declare anything.
      #
      # Set, it is an append-only ledger of acts, and the API refuses to start
      # if a command already on the chain has been edited or removed: a sealed
      # record is not rewritten, and the correction is another act. It cannot
      # move money whatever it says — ADR 0021 permits the gate and refuses
      # the engine, and an admitted verdict carries no way to execute.
      #
      # Every environment leaves this null: the desk holds no external
      # destination, and declaring one would journal an act nobody performed.
      var.capital_fabric_file == null ? {} : {
        capital-fabric = {
          content           = file("${path.module}/../../${var.capital_fabric_file}")
          file_name         = "capital-fabric.json"
          content_type      = "application/json"
          env_file_variable = "QIP_CAPITAL_FABRIC_PATH"
        }
      },
    )
    # The candidate data sources the deep brain assesses, each beside the
    # reviewed egress route it is reached through (ADR 0054).
    #
    # The route is why this is a per-environment choice rather than a constant.
    # A candidate is probed only where an Envoy cluster and a listener already
    # exist for its host — the proxy is a reverse proxy and a process cannot
    # name a destination — so mounting a catalogue whose routes this
    # environment's proxy does not serve gives the node a list it can load and
    # not reach. Setting this is therefore the *second* step: the first is a
    # cluster in `egress/envoy.yaml`.
    #
    # Unset, the node assesses nothing and its banner says so. That is not a
    # degraded state: no control reads the source catalogue, so an absent one
    # starves nothing — unlike an absent universe, which feeds no exposure
    # bucket and hides two limits that can then never fire, and is refused at
    # start-up for exactly that reason.
    deepbrain = var.source_candidates_file == null ? {} : {
      source-candidates = {
        content           = file("${path.module}/../../${var.source_candidates_file}")
        file_name         = "source-candidates.json"
        content_type      = "application/json"
        env_file_variable = "QIP_SOURCE_CANDIDATES_PATH"
      }
    }
  }

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
      # The universe every root reads, and whatever optional files the tfvars
      # named for this workload. A comprehension rather than a bare `merge`
      # so the block still opens with `config_files = {`: three acceptance
      # walks read the universe's mount out of the lines under that opening,
      # and a `merge(` on this line would make each of them read nothing and
      # stop checking rather than fail.
      config_files = {
        for name, document in merge(
          {
            universe = {
              content           = local.universe_catalogue
              file_name         = "universe.json"
              content_type      = "application/json"
              env_file_variable = "QIP_UNIVERSE_PATH"
            }
          },
          local.optional_config_files.api,
        ) : name => document
      }
      secret_mounts = {
        token-operator = {
          secret_id         = module.secrets.secret_ids["qip-token-operator"]
          file_name         = "token-operator"
          env_file_variable = "QIP_TOKEN_OPERATOR_FILE"
        }
        # There is no token-approver mount. The approver role authorised no
        # route in `qip-api`, so this mounted a credential that granted its
        # holder exactly what the analyst token granted, in all four
        # environments; the binary now refuses to start if either spelling of
        # QIP_TOKEN_APPROVER reaches it, so putting the mount back would stop
        # the revision rather than quietly restore a dead control.
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
        # The two halves of the Alpaca market-data credential, as files. The
        # shipped manifest names `QIP_ALPACA_API_SECRET_KEY` and its companion
        # `QIP_ALPACA_API_KEY_ID`, `qip_core::secret` resolves the `_FILE`
        # variant of each, and `GET /registrations` prints the one command that
        # fills the slot — `gcloud secrets versions add qip-alpaca-api-secret-key
        # --data-file=-`. That command has to name a container that exists, and
        # the process has to be able to read what a person puts in it; those are
        # the two halves, and `main.tf` holds the first.
        #
        # No value is created here, ever: Terraform creates the container and a
        # person writes the version out of band (ADR 0040,
        # docs/operations/registering-a-venue.md). Two consequences, said out
        # loud rather than discovered. A mount is not a registration — the
        # source stays refused by the licensing gate and by the registration
        # gate until the terms are read and a record is committed. And a Cloud
        # Run revision cannot start on a secret with no enabled version, so the
        # slot must be filled before the manifest beside this reconciles; that
        # is the same order the runbook already gives (fill the slot, then
        # deploy), and it is why the slots exist rather than appearing on the
        # day somebody registers.
        alpaca-api-key-id = {
          secret_id         = module.secrets.secret_ids["qip-alpaca-api-key-id"]
          file_name         = "alpaca-api-key-id"
          env_file_variable = "QIP_ALPACA_API_KEY_ID_FILE"
        }
        alpaca-api-secret-key = {
          secret_id         = module.secrets.secret_ids["qip-alpaca-api-secret-key"]
          file_name         = "alpaca-api-secret-key"
          env_file_variable = "QIP_ALPACA_API_SECRET_KEY_FILE"
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
        # No venue credential here, and the omission is the decision rather
        # than an oversight. This is the workload `var.market_data_connector`
        # names, so mounting the Alpaca key looks right until you read the
        # line above: `egress_proxy = false`, deliberately, because port 9102
        # on this sidecar is a route to a language model API and nothing on
        # the hot path may consult one (ADR 0008, and the `precondition` on
        # `local.cloud_run_catalogue.fastbrain` that refuses the proxy by
        # name). Without the proxy this process has no outbound HTTPS path at
        # all, so a credential mounted here is one it could never spend — a
        # secret readable in a container that cannot reach the vendor it
        # authenticates to, which widens the blast radius and buys nothing.
        # `the_fast_brain_cannot_reach_anything_that_could_serve_a_language_model`
        # in the infrastructure suite pins this to the envelope key alone; it
        # caught exactly this mount being added, so the guard is not
        # hypothetical. Reaching a live vendor from the fast brain is a
        # topology question — which workload runs the connector, and what it
        # is allowed to dial — and it is answered in an ADR before it is
        # answered in a secret mount.
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
      # The universe every root reads, and the candidate sources this
      # environment named, if any. A comprehension rather than a bare `merge`
      # for the reason the API's block gives: three acceptance walks read the
      # universe's mount out of the lines under `config_files = {`, and a
      # `merge(` on that line makes each of them read nothing and stop
      # checking rather than fail. That is exactly what happened when this was
      # first written as a merge — the walk reported the deep brain mounting
      # no config_files at all.
      config_files = {
        for name, document in merge(
          {
            universe = {
              content           = local.universe_catalogue
              file_name         = "universe.json"
              content_type      = "application/json"
              env_file_variable = "QIP_UNIVERSE_PATH"
            }
          },
          local.optional_config_files.deepbrain,
        ) : name => document
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
  #
  # OpenObserve is merged in rather than folded into the comprehension: it is
  # not a member of `cloud_run_catalogue` (see the module below for why), so a
  # comprehension reading only that map would silently omit the one workload
  # in the management zone. No `permitted_paths` names `management` as a
  # source or destination in any environment today, so nothing yet reads this
  # entry — but an identity a zone's own module cannot see is an identity a
  # future path grant would silently miss.
  zone_identities = merge(
    {
      for zone in distinct([for workload in local.cloud_run_catalogue : workload.trust_zone]) :
      zone => sort([for name, workload in module.cloud_run : workload.service_account_email if workload.trust_zone == zone])
    },
    {
      "management" = sort([for workload in module.openobserve : workload.service_account_email])
    }
  )
}

# The plan refuses a catalogue that is not fully placed.
#
# Preconditions on a `terraform_data` rather than in the module, because the
# facts they check are the root's: whether the zone a workload names is one
# this environment declared a subnet for. A lookup that simply failed would
# report an invalid index; these report the decision that is missing. The
# precondition that once refused a workload with no attested digest left
# with `image_digests`: the digest is in the manifest now, and a manifest
# naming one the attestor never signed is refused at admission and reads as
# a `Degraded` Application rather than a failed plan (ADR 0036 decision 8).
resource "terraform_data" "catalogue_is_placed" {
  input = sort(keys(local.cloud_run_catalogue))

  lifecycle {
    precondition {
      condition = alltrue([
        for workload in values(local.cloud_run_catalogue) : contains(keys(var.trust_zones), workload.trust_zone)
      ])
      error_message = "A catalogue workload names a trust zone this environment does not declare in `trust_zones`: ${join(", ", distinct([for workload in values(local.cloud_run_catalogue) : workload.trust_zone if !contains(keys(var.trust_zones), workload.trust_zone)]))}. A workload with no zone has no subnet, no tag and no rule; declare the zone's range in the tfvars."
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

# The workload's identity, grants and published files — everything the
# manifest cannot carry. What it used to pass and no longer does is the
# service: the posture, the invokers, the image, the resources, the instance
# bounds and the probes are the manifest's, held to this catalogue by the
# parity test (ADR 0036 decision 4).
module "cloud_run" {
  source   = "./modules/cloudrun"
  for_each = local.cloud_run_catalogue

  # Nothing here can be created before its API is on. See module "services".
  depends_on = [module.services]

  project_id     = var.project_id
  project_number = local.project_number
  region         = var.region
  environment    = var.environment
  labels         = local.labels

  name          = each.key
  plane         = each.value.plane
  trust_zone    = each.value.trust_zone
  traffic_class = each.value.traffic_class

  # The zone's tag, which the manifest puts on the interface and every rule
  # in modules/trust-zones targets. Recorded here so the root's output and
  # the parity test read it from one place.
  network_tags = compact([lookup(module.trust_zones.zone_network_tags, each.value.trust_zone, "")])

  container_port = 8080

  env           = each.value.env
  secret_mounts = each.value.secret_mounts
  config_files  = each.value.config_files

  egress_sidecar = each.value.egress_proxy ? module.egress_proxy.sidecar : null

  # The managed-Prometheus collector, for the workloads that ask for one and
  # only once the root names a digest. Composed here from the registry
  # prefix and the bare digest, so the only image a plan can carry is the
  # mirrored, attested copy; null — the state of every environment today —
  # is no scrape document and `metrics_collected = false`.
  collector_image_digest = each.value.metrics_collector && var.metrics_collector_image_digest != null ? "${module.registry.image_prefix}/vendor/cloud-run-gmp-sidecar@${var.metrics_collector_image_digest}" : null

  # Config Connector creates every revision now (ADR 0036 decision 5) and
  # must act as the service's own identity to do it. Null where there is no
  # control plane, which is a service nothing can move — the honest state of
  # an environment that has not turned the reconciler on.
  deployer_service_account = var.gitops_enabled ? module.gitops_control_plane[0].kcc_service_account_email : null
}

# The plan refuses to name a digest for a zone this environment never
# declared. `catalogue_is_placed` above catches the same shape for the built
# catalogue; this is that check's other half, gated on the one condition that
# makes OpenObserve exist at all rather than on an unconditional map lookup,
# because the zone genuinely need not be declared while the digest is null.
resource "terraform_data" "openobserve_is_placed" {
  count = var.vendored_openobserve_image_digest != null ? 1 : 0
  input = "openobserve"

  lifecycle {
    precondition {
      condition     = contains(keys(var.trust_zones), "management")
      error_message = "vendored_openobserve_image_digest is set but trust_zones does not declare \"management\"; OpenObserve has no subnet and no tag without it. Declare the zone's range in the tfvars alongside the digest."
    }
  }
}

# --- OpenObserve (ADR 0028) --------------------------------------------------
#
# The platform's metrics, logs and traces backend, adopted as a deliberate,
# named exception to blueprint §2.1 (ADR 0028 decision 1) — not part of
# `local.cloud_run_catalogue` above, and deliberately so: every entry there is
# a binary `deploy.yml` builds, signs and attests, its digest read from
# `var.image_digests`, and `catalogue_workloads()` in the acceptance suite
# asserts the map holds exactly those three. OpenObserve has no such
# pipeline — its digest is mirrored and attested by `vendor.yml` from the
# reviewed line in `infrastructure/egress/vendored-images.txt` — so this is
# the one instantiation in the tree that exercises `modules/cloudrun`'s
# `source = "vendored"` path (ADR 0028 decision 3) directly, rather than
# folding a second image lifecycle into a `for_each` built for one.
#
# `count`, not `for_each`, because there is exactly one of these and its
# existence is a single yes/no: null in `vendored_openobserve_image_digest`
# is the closed state described there, and no identity is created at all —
# the same shape `execution_nodes` uses for "no node configured yet".
#
# The service itself is `infrastructure/gitops/envs/<env>/openobserve.yaml`
# (ADR 0036 decision 4), and the three facts ADR 0028, 0030 and 0031 record
# about it live there and in the parity test rather than as module inputs:
#
#   * anonymous on the public internet, on the owner's instruction (ADR
#     0030, amending ADR 0028 decision 5). The manifest is the only one under
#     envs/ whose ingress is `INGRESS_TRAFFIC_ALL`, and its `allUsers`
#     invoker is an `IAMPolicyMember` beside it; the test admits exactly that
#     one and refuses a second. The trigger ADR 0030 set for itself stands:
#     the service is empty today and stops being empty the moment any
#     deployment sets QIP_OPENOBSERVE_URL, and that change is the one that
#     must move it behind IAP or re-argue the exposure.
#   * the image is the mirrored, attested copy at the digest the root names
#     below, `<registry>/vendor/openobserve@<digest>`; a manifest naming the
#     upstream repository, a tag, or a digest vendored-images.txt never
#     reviewed fails the test.
#   * 5080 and /healthz are OpenObserve's own defaults, confirmed against its
#     published quick-start; two CPUs, 2Gi, twenty concurrent requests, and a
#     floor of zero because a warm instance holding ephemeral storage is a
#     warm instance whose dashboards vanish anyway.
module "openobserve" {
  source = "./modules/cloudrun"
  count  = var.vendored_openobserve_image_digest != null ? 1 : 0

  # Nothing here can be created before its API is on. See module "services".
  depends_on = [module.services]

  project_id     = var.project_id
  project_number = local.project_number
  region         = var.region
  environment    = var.environment
  labels         = local.labels

  name = "openobserve"

  # `data-and-observability` is the one plane this workload could name; there
  # is no matching entry in blueprint §46.1's thirteen trust zones, so the
  # zone is named explicitly. `management` is the zone built for exactly this
  # shape of workload: reached by an operator with a binding, not by another
  # workload's traffic (ADR 0028 decision 5 — no path from any zone into this
  # one is sanctioned today, and none is added here).
  plane         = "data-and-observability"
  trust_zone    = "management"
  traffic_class = "platform"

  network_tags = compact([lookup(module.trust_zones.zone_network_tags, "management", "")])

  container_port = 5080

  # Ephemeral storage, on purpose and by instruction (ADR 0028 decision 4).
  # OpenObserve's only durable backend is S3-compatible (`ZO_S3_*`, confirmed
  # against its own published environment-variable reference), which this
  # platform cannot reach without a GCS HMAC access/secret key pair — the
  # class of static, long-lived credential
  # `.claude/rules/01-security-and-safety.md` forbids outright. Both
  # variables here are already OpenObserve's own defaults; they are written
  # explicitly, the way the metrics collector's scrape interval is, so the
  # choice is a line in this diff and not a fact left to the image. A cold
  # start loses every dashboard this deployment ever held — named here, not
  # hidden, per the ADR's own "what it costs".
  env = {
    ZO_LOCAL_MODE         = "true"
    ZO_LOCAL_MODE_STORAGE = "disk"
  }

  # The initial admin login (OpenObserve's own, never a cloud credential), as
  # environment values, which ADR 0031 permits for a vendored workload and
  # refuses for every built one.
  #
  # This was a `secret_mounts` block until that record. The mount satisfied
  # `.claude/rules/01-security-and-safety.md` and did nothing: the image
  # carries no shell, so no entrypoint can read a file and exec, and no symbol
  # in the binary offers `_FILE` indirection for the credential -- both
  # checked against `openobserve@sha256:88fb692a...` rather than assumed. The
  # file was projected at 0400, the `_FILE` variable held its path, and the
  # process opened neither. Keeping it beside a working env var would have
  # been a second control that reads as protection and is not.
  #
  # What is still true: the value is in no committed file, no plan and no
  # state -- Terraform carries the secret's name and Cloud Run resolves the
  # version at container start. What is not: it is in the container's
  # environment, and ADR 0031 names the crash dump that leaves open. What
  # this module holds of it is the accessor grant; the `secretKeyRef` is the
  # manifest's, and the parity test refuses one on any manifest whose image
  # `deploy.yml` builds.
  secret_env = {
    ZO_ROOT_USER_EMAIL = {
      secret_id = module.secrets.secret_ids["qip-openobserve-root-email"]
    }
    ZO_ROOT_USER_PASSWORD = {
      secret_id = module.secrets.secret_ids["qip-openobserve-root-password"]
    }
  }

  # Config Connector creates this service's revisions too, from the manifest
  # beside the three built workloads'. Null where there is no control plane.
  deployer_service_account = var.gitops_enabled ? module.gitops_control_plane[0].kcc_service_account_email : null
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
