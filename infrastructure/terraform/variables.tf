# Inputs.
#
# Every variable that could make the deployment less safe has a restrictive
# default and a validation rule. A variable whose default is dangerous is a
# variable someone will forget to set.

variable "project_id" {
  description = "The Google Cloud project."
  type        = string

  validation {
    condition     = can(regex("^[a-z][a-z0-9-]{4,28}[a-z0-9]$", var.project_id))
    error_message = "The project id must be a valid Google Cloud project identifier."
  }

  # An environment whose tfvars still say `unprovisioned` has no project, and
  # this is where that stops — at plan time, with a message naming the act
  # that is missing. The alternative is what this replaced: a plausible-looking
  # id pointing at a deleted project, which fails much later with an
  # authentication error about an audience nobody can explain. The marker is a
  # valid project-id *shape*, so the check above admits it and only this one
  # refuses it; that is deliberate, because the shape check should keep saying
  # what it says and this should say what it says.
  validation {
    condition     = var.project_id != "unprovisioned"
    error_message = "This environment is not provisioned: its tfvars still carry the `unprovisioned` marker. Create a project for it — its own, never one another environment already uses — record the id and number in infrastructure/environments/<env>/terraform.tfvars, and give it a state bucket."
  }
}

variable "region" {
  description = "The region everything is deployed to."
  type        = string
  default     = "europe-west2"
}

variable "environment" {
  description = "Which environment this is: dev, test, stage or prod."
  type        = string

  validation {
    # Four, and short. The platform's environments are DEV/TEST/STAGE/PROD, and
    # `test` had no permitted value at all — an environment named in the
    # architecture that no configuration could express.
    #
    # Short on purpose. These names are interpolated into Google resource ids
    # with hard length limits, and `production` was already one character over
    # for an edge cell: `qip-edge-frankfurt-1-production` is 31 characters
    # against a service account's limit of 30. That combination passed variable
    # validation and failed at apply, so the platform as configured could not
    # deploy a cell to production. See the length validation in
    # `modules/edge-cell`, which now catches the class rather than this case.
    condition     = contains(["dev", "test", "stage", "prod"], var.environment)
    error_message = "The environment must be dev, test, stage or prod."
  }
}

variable "autonomy_ceiling" {
  description = <<-EOT
    The highest autonomy level this environment's platform may reach.

    Paper trading by default, and deliberately an input rather than something
    derived from the environment name: an environment called "production" that
    trades on paper is a perfectly reasonable thing to want, and inferring the
    ceiling would take that choice away.

    Setting this above paper_trading does not enable live trading. It permits
    two authenticated operators to enable it, which is a separate act.

    None of the four environments may set it above paper_trading, and the
    validation below refuses one that tries. See that validation for why the
    refusal is here rather than only in the application.
  EOT
  type        = string
  default     = "paper_trading"

  validation {
    condition = contains([
      "observation",
      "advisory",
      "paper_trading",
      "supervised_live",
      "limited_autonomous_live",
      "autonomous_live",
    ], var.autonomy_ceiling)
    error_message = "The autonomy ceiling must be one of the six declared levels."
  }

  # The three live levels are declared above because the platform's domain
  # model has six rungs and a value that is merely misspelt should fail
  # differently from one that is spelt correctly and forbidden. This second
  # validation is the forbidding one.
  #
  # It exists as its own gate rather than by shortening the list above so that
  # the error an operator reads names the reason. "Must be one of the six
  # declared levels" sent to somebody who typed `autonomous_live` — a level
  # that is one of the six — would be a message that contradicts itself, and
  # they would spend the next ten minutes checking their spelling.
  #
  # This is the earliest of the layers that refuse a live configuration, not
  # the only one: it stops a bad value at `terraform plan`, before it reaches
  # the QIP_AUTONOMY_CEILING every catalogue workload reads. It does not stop
  # a service updated by hand, which is why the composition roots refuse the
  # same values at start-up. Neither layer is redundant — this one catches the
  # reviewed, committed mistake, and that one catches the unreviewed live edit.
  validation {
    condition = !contains([
      "supervised_live",
      "limited_autonomous_live",
      "autonomous_live",
    ], var.autonomy_ceiling)
    error_message = <<-EOT
      This platform is paper-trading only, and the autonomy ceiling names a
      level at which orders reach a real venue. No environment may be applied
      with it. Lower the ceiling to paper_trading, advisory or observation.
    EOT
  }
}

# --- The runtime (ADR 0022, ADR 0024) ------------------------------------------
#
# Every warm binary is a Cloud Run service from `catalogue.tf`; the execution
# node is a Compute Engine machine from `execution_nodes`; both attach to the
# trust zones declared below. There is no cluster variable left here, and
# none may return without an ADR: the GKE runtime and everything that
# configured it were retired under ADR 0024.

variable "image_digests" {
  description = <<-EOT
    The image digest each catalogue binary is deployed at, keyed by binary
    name, as `sha256:<64 hex>`.

    Written by `.github/workflows/deploy.yml` into
    `infrastructure/environments/<env>/images.tfvars` after it has built,
    scanned, signed and attested the image and moved the service to it —
    never typed by a person. Terraform creates a service at the digest
    recorded here and thereafter ignores the image, because the pipeline owns
    it; `modules/cloudrun` says why. A binary with no entry is refused at
    plan time by `catalogue.tf`, which names the pipeline run that is
    missing.

    Empty by default: an environment nothing has ever deployed to has no
    digest to record, and inventing one would be a service created at bytes
    nobody attested.
  EOT

  type    = map(string)
  default = {}

  validation {
    condition     = alltrue([for digest in values(var.image_digests) : can(regex("^sha256:[a-f0-9]{64}$", digest))])
    error_message = "Every image digest is `sha256:<64 hex>`. A tag is a name someone can move after the attestation was signed."
  }
}

variable "trust_zones" {
  description = <<-EOT
    The trust zones this environment declares, keyed by the thirteen names of
    blueprint §46.1, each with the region its subnet lives in and its own
    range. `modules/trust-zones` refuses a name outside the thirteen and a
    range shared by two zones.

    Every zone the catalogue places a workload in must be declared here, or
    the plan refuses with the zone named: a workload with no zone has no
    subnet, no tag and no rule. Ranges belong in the tfvars, not in a default
    — an address range chosen as a convenience is the one that collides.
  EOT

  type = map(object({
    region      = string
    subnet_cidr = string
  }))

  default = {}
}

variable "permitted_paths" {
  description = <<-EOT
    The zone-to-zone paths that exist, keyed by a short name. Empty means no
    zone may reach any other, which is the fail-closed reading. A pair and a
    mode must both be sanctioned by `modules/trust-zones` or the plan refuses
    them; see that module's variable for the fields.
  EOT

  type = map(object({
    from  = string
    to    = string
    mode  = string
    ports = list(number)
    note  = string
  }))

  default = {}
}

variable "external_egress" {
  description = <<-EOT
    Every destination outside the VPC any zone may reach, one entry per
    destination. Empty is a platform that reaches nothing external, which is
    the correct state for connectivity nobody has confirmed. `ibm-quantum`
    may be declared only for `optimisation`; `modules/trust-zones` refuses
    every other spelling at plan time.
  EOT

  type = map(object({
    zone    = string
    cidr    = string
    port    = number
    purpose = string
    note    = string
  }))

  default = {}
}

variable "public_ingress" {
  description = "Where a client may arrive: Google's load-balancer ranges to one zone on one port, refused for every zone but the public edge and application-and-identity. Empty by default."

  type = map(object({
    zone = string
    port = number
    note = string
  }))

  default = {}
}

variable "execution_nodes" {
  description = <<-EOT
    The execution nodes this environment runs, keyed by node id — which is
    the cell id the binary is configured with.

    Blueprint §41.4 calls for one dedicated C3 per region. This is a map so
    that the next one is an entry rather than a directory, and it is empty by
    default and in every environment: a node must be configured for at least
    one venue, `qip-edge-node` refuses an empty `QIP_VENUES`, and no venue's
    published address ranges are recorded anywhere in this repository. The
    first entry is a venue decision and the plan that carries it is the
    evidence ADR 0020's step 3 asks for.

    Every field is deliberate:

      * `region` and `zone` are chosen for distance to the venues.
      * `subnet_cidr` must overlap neither another node's nor any trust
        zone's. Overlapping ranges route to whichever subnet was created
        first, silently.
      * `machine_type` is one of the C3/C3D high-CPU shapes §41.4 permits;
        the module refuses anything else.
      * `boot_image` is one image by self-link, never a family. The image is
        the other half of the node — modules/execution-node/README.md.
      * `venues` is not guessed. It comes from the venue's own connectivity
        documentation, and in shadow mode the node still cannot reach them.
      * `create_egress_nat` is true only where the node's region has no NAT
        of its own; two NATs on one subnet in one region is an apply error.
  EOT

  type = map(object({
    region       = string
    zone         = string
    subnet_cidr  = string
    machine_type = string
    boot_image   = string
    venues = map(object({
      cidr = string
      port = number
    }))
    create_egress_nat = optional(bool, false)
    # The capital this node may reserve across its strategies, as a positive
    # decimal string. Required per entry and never defaulted: `qip-edge-node`
    # refuses to start without QIP_REGION_ALLOCATION, and a value that
    # appeared from nowhere would be the one number in a cell's envelope no
    # reviewer had read.
    region_allocation = string
    # How the node prices the intents of every strategy it deploys, and where
    # it reads the compiled plan from. Empty is "deploy nothing", which is
    # what the node does with them unset; they are written into `node.env`
    # regardless so the choice is visible here rather than nowhere. The
    # module validates both.
    default_pricing    = optional(string, "")
    strategy_plan_path = optional(string, "")
  }))

  default = {}

  validation {
    condition     = length(distinct([for node in values(var.execution_nodes) : node.subnet_cidr])) == length(var.execution_nodes)
    error_message = "Two execution nodes share a subnet range. Overlapping ranges route to whichever subnet was created first."
  }
}

variable "egress_allowed_upstreams" {
  description = <<-EOT
    The hosts the egress proxy may dial, checked at plan time against the
    hosts `infrastructure/egress/envoy.yaml` actually dials. The two must be
    the same set, so widening the proxy is an edit to the bootstrap and an
    edit here, reviewed together. The default is the six hosts the adapters
    name and the bootstrap declares; an environment that needs fewer narrows
    the bootstrap, not this list.

    Five of the six are Google's or IBM's — infrastructure this platform runs
    on. `api.frankfurter.app` is the first that is neither: a market-data
    vendor, reached on one path by one connector whose licensing posture is
    evaluated in `qip-fastbrain`'s catalogue before the feed opens. It is
    listed here rather than folded in silently because it is the entry that
    changes what this list *is* — no longer only the platform's own
    dependencies — and the acceptance suite fails if this set and the
    bootstrap disagree in either direction.
  EOT

  type = list(string)
  default = [
    "storage.googleapis.com",
    "bigquery.googleapis.com",
    "europe-west2-aiplatform.googleapis.com",
    "quantum.cloud.ibm.com",
    "api.quantum.ibm.com",
    "api.frankfurter.app",
  ]
}

# --- The workloads' shared, non-secret settings ---------------------------------
#
# What the `qip-config` ConfigMap carried on GKE. Values only; every credential
# reaches a workload as a mounted file through `secret_mounts` in catalogue.tf.

variable "storage_target" {
  description = <<-EOT
    Which store every central workload uses, read as `QIP_STORAGE_TARGET`.

    `memory` is a statement rather than a placeholder: a Cloud Run instance
    keeps nothing across a restart and has no volume to keep it on. The three
    implemented targets are memory, file and engine; the six managed ones are
    ports that refuse construction, so naming one here stops a service
    starting rather than upgrading its durability — at
    `StorageSettings::preflight`, before it serves anything, which is the
    intended direction.
  EOT
  type        = string
  default     = "memory"

  validation {
    condition     = contains(["memory", "file", "engine"], var.storage_target)
    error_message = "The storage target is memory, file or engine — the three targets this build implements. A managed store here is a service that refuses to start."
  }
}

variable "cycle_interval_seconds" {
  description = "How often the deep brain runs a cycle, read as `QIP_CYCLE_INTERVAL_SECONDS`. A string because it is an environment value."
  type        = string
  default     = "300"

  validation {
    condition     = can(regex("^[1-9][0-9]*$", var.cycle_interval_seconds))
    error_message = "The cycle interval is a whole number of seconds."
  }
}

variable "market_data_connector" {
  description = <<-EOT
    The live market-data connector the fast brain selects, or null for the
    synthetic exchange every environment runs today.

    Both keys or neither, which the object type makes structural:
    `connector_feed` refuses half a configuration by name rather than falling
    back, because the fallback is the synthetic exchange wearing a configured
    look. `base_url` is the egress proxy's `http://127.0.0.1:<port>` address
    and never the vendor's — `qip_transport::http` refuses `https` by name —
    and that proxy reaches only the hosts its bootstrap names, so selecting a
    source means adding its listener there in the same change. It also means
    a licensing decision recorded before the source is used
    (.claude/rules/domains/data-and-streaming.md); this variable does not
    stand in for one.
  EOT

  type = object({
    source   = string
    base_url = string
  })

  default = null

  validation {
    condition     = var.market_data_connector == null ? true : startswith(var.market_data_connector.base_url, "http://127.0.0.1:")
    error_message = "The connector's base URL is the egress proxy on loopback, http://127.0.0.1:<port>. `qip_transport::http` refuses https by name, and an address off the instance is a route that does not exist."
  }
}

variable "notification_channels" {
  description = "Where alerts are sent. An alert with nowhere to go is not an alert."
  type        = list(string)
  default     = []
}

variable "github_repository" {
  description = <<-EOT
    The GitHub repository permitted to deploy, as `owner/name`.

    No default. A default here would be a repository somebody else could be
    running, and the consequence of getting it wrong is that their pipeline can
    push images and apply manifests in this project.
  EOT

  type = string

  validation {
    condition     = can(regex("^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$", var.github_repository))
    error_message = "The repository is owner/name, with no scheme and no trailing path."
  }
}

variable "project_number" {
  description = <<-EOT
    The project's numeric id, or null to look it up.

    Distinct from `project_id` and not inferable by reading it: Google's own
    service agents are named by number, so the IAM grants that let Secret
    Manager publish a rotation notice and GKE Backup write to its bucket need
    this. Terraform can ask Cloud Resource Manager for it, which is what
    happens when this is null, and that is the normal case — a number typed in
    by hand is a number that can disagree with `project_id`, and the failure
    then is an IAM binding granted to a service agent in someone else's
    project.

    Set it explicitly only where the lookup cannot run: Cloud Resource Manager
    disabled, or an identity without `resourcemanager.projects.get`. Read it
    from `gcloud projects describe <project_id> --format='value(projectNumber)'`.
  EOT
  type        = number
  default     = null
}

# --- Managed data services --------------------------------------------------
#
# All default false. `qip_storage::provider::StorageTarget::is_implemented`
# returns true for three targets — memory, local files, and the in-tree engine
# — and the six below are ports that refuse construction while naming what
# they still need.
#
# The flag means "an adapter exists and I have wired it", not "I would like
# this service". Turning one on beforehand produces a healthy, empty, billable
# instance and an architecture diagram that overstates the platform. The
# `data` module's `enabled_without_an_adapter` output reports exactly that
# mismatch at plan time.

variable "enable_bigquery" {
  description = "Research warehouse. Requires an adapter for StorageTarget::BigQuery."
  type        = bool
  default     = false
}

variable "enable_cloud_storage" {
  description = "Event-log archive and model artifacts. Requires an adapter for StorageTarget::CloudStorage."
  type        = bool
  default     = false
}

variable "enable_alloydb" {
  description = "Transactional records. Requires an adapter and a Postgres driver this build does not have."
  type        = bool
  default     = false
}

variable "enable_bigtable" {
  description = "Tick and order-book history. Requires an adapter for StorageTarget::Bigtable."
  type        = bool
  default     = false
}

variable "enable_memorystore" {
  description = "Hot cache. Requires an adapter for StorageTarget::Memorystore."
  type        = bool
  default     = false
}

variable "enable_spanner" {
  description = "Cross-region transactions. The last to enable, not the first — AlloyDB is cheaper everywhere a transaction stays in one region."
  type        = bool
  default     = false
}

variable "enable_vertex_ai" {
  description = "Managed training. The Vertex port in qip-training has no client, no credential and no egress path, so enabling this provisions somewhere to train without making this build able to submit a job."
  type        = bool
  default     = false
}

# --- Private connectivity ---------------------------------------------------
#
# Both default false, for a reason one step beyond the managed data services
# above. A database enabled early is a bill and an attack surface. An
# interconnect attachment enabled early is those, plus a private path that
# appears in the project and in every diagram and does not exist: a VLAN
# attachment carries nothing until a partner has provisioned a circuit against
# its pairing key, and Terraform cannot order a cross-connect.
#
# See modules/connectivity/NOT-ORDERED.md for the four things a deployment
# must arrange first, and environments/prod/terraform.tfvars for why three cells
# need them.

variable "enable_partner_interconnect" {
  description = "Cloud Router and VLAN attachments for Partner Interconnect. Requires a partner, a circuit and a pairing key handed over — none of which Terraform can create."
  type        = bool
  default     = false
}

variable "partner_interconnects" {
  description = <<-EOT
    The VLAN attachments to create, keyed by a short name.

    Empty by default. Two entries per site, in different edge availability
    domains, or the redundant pair is one circuit twice — a single metro
    maintenance window takes both.
  EOT

  type = map(object({
    region                   = string
    edge_availability_domain = string
    admin_enabled            = optional(bool, false)
    description              = optional(string, "")
  }))

  default = {}
}

variable "cloud_router_asn" {
  description = "The VPC side's BGP ASN. Private, and not the one the colocated equipment uses: two ends claiming one ASN never establish a session."
  type        = number
  default     = 64514
}

variable "enable_private_service_connect" {
  description = "An internal endpoint answering for Google APIs, so the far end of an interconnect reaches them without a route to the internet. Needs DNS on the far end, which is not a resource here."
  type        = bool
  default     = false
}

variable "private_service_connect_address" {
  description = "The endpoint's internal address. No default: it must overlap neither this VPC's subnets nor the far end's ranges, and only the deployment knows both."
  type        = string
  default     = ""
}

variable "private_service_connect_target" {
  description = "Which bundle the endpoint reaches: vpc-sc (restricted, the set a VPC Service Controls perimeter can protect) or all-apis."
  type        = string
  default     = "vpc-sc"
}

# --- API enablement ---------------------------------------------------------

variable "disable_services_on_destroy" {
  description = <<-EOT
    Whether `terraform destroy` turns the project's Google APIs back off.

    **False**, and the asymmetry here is total rather than a judgement.

    Disabling a Google API is not a permissions change. Disabling
    `compute.googleapis.com` deletes every Compute resource in the project —
    instances, disks, networks, firewall rules — whether or not this
    configuration created them. The plan gives no hint: it shows one API being
    disabled, not the resources that go with it. In a project holding anything
    besides this platform, a destroy aimed here becomes somebody else's outage.

    Leaving an API enabled after a destroy costs nothing. Google does not bill
    for an enabled API with nothing under it, and the next apply adopts it.

    Set it true only where the project exists for one change and is deleted
    whole afterwards, so the destroy is the project going away and there is
    nothing else in it to damage.
  EOT
  type        = bool
  default     = false
}

# --- Security Command Center ------------------------------------------------

variable "enable_security_command_center" {
  description = <<-EOT
    Whether to create this project's Security Command Center resources: two
    custom Security Health Analytics detectors, and any mute configurations
    declared below.

    **Off**, and not because the resources cost anything — they are free, and
    the detectors are ones this platform would genuinely benefit from. They
    watch for a cluster whose Binary Authorization enforcement has been turned
    off and one whose control plane has been made public: two properties the
    acceptance suite refuses in the repository and nothing watches in the
    project, where they are each a single field in a console.

    It is off because everything here only evaluates if Security Command Center
    is **activated at the organisation this project belongs to**, at Premium or
    Enterprise. That is not a project-level act, this configuration has no
    organisation id by design, and nothing here can check it. Turning it on
    inside an organisation that has not activated SCC creates two detectors that
    are accepted, stored, never run, and read in the console as a project being
    watched.

    That failure is worse than the gap it replaces. An absent control is visibly
    absent; a control that never fires looks like a clean result.

    modules/scc/ORGANISATION-SCOPED.md lists what must be true first and what
    stays out of reach afterwards — including why there is deliberately no
    notification config or BigQuery export here.
  EOT
  type        = bool
  default     = false
}

variable "scc_muted_findings" {
  description = <<-EOT
    Security Command Center findings this deployment has decided not to act on,
    keyed by mute config id.

    Empty, and it should stay small. Each entry stops a class of finding being
    shown to anybody, so the `description` is the load-bearing field: it is the
    only record of who decided and why, and what a reviewer reads when the muted
    thing turns out to have mattered. The module refuses an entry whose
    description is shorter than twenty characters.

    They live here rather than in a console because a mute clicked in a console
    has no author, no date and no argument attached, and a year later is
    indistinguishable from a finding nobody ever saw.
  EOT

  type = map(object({
    filter      = string
    description = string
    type        = optional(string, "DYNAMIC")
  }))

  default = {}
}

# --- Backups ----------------------------------------------------------------
#
# There is no `enable_backup` here, deliberately.
# `docs/operations/disaster-recovery.md` recorded the absence of a snapshot
# schedule on the edge cell journals as a gap the platform has; a flag whose
# default is off would leave that gap exactly where it was and add a line to the
# configuration implying otherwise. `backup_paused` is the honest form of "not
# right now": it keeps the plan, the key and the retention and suspends the
# schedule.

variable "snapshot_start_time" {
  description = <<-EOT
    When the disk-level journal snapshot schedule runs, as `HH:MM` in UTC.

    Offset from `backup_schedule`: two snapshot mechanisms reading the same
    disks in the same minute is avoidable I/O on a volume a cell is actively
    journalling to, and neither is urgent enough to contend for it.
  EOT
  type        = string
  default     = "05:00"
}

variable "snapshot_retain_days" {
  description = <<-EOT
    How long a journal disk snapshot is kept. Ninety days, longer than the GKE
    backup plan's retention and deliberately so.

    These are the copies that keep covering a journal after its claim has been
    deleted — a cell taken out of service, whose disk is `Released` and whose
    decision record somebody may still be asked about. That question is a
    compliance one rather than an operational one, so the window is months
    rather than weeks. Snapshots are incremental, so this costs far less than
    the number suggests for a volume that appends.
  EOT
  type        = number
  default     = 90
}

variable "workload_metrics_exist" {
  description = <<-EOT
    Whether this project has ever ingested the platform's own Prometheus
    metrics. False until the first deployment runs; the four workload alert
    policies exist only when it is true, because Cloud Monitoring refuses a
    policy naming a metric it has never seen. Flip it in the tfvars after the
    first deployment and re-apply.
  EOT
  type        = bool
  default     = false
}

variable "metrics_collector_image_digest" {
  description = <<-EOT
    The digest of the managed-Prometheus collector the catalogue's scraped
    workloads run as a sidecar, as `sha256:<64 hex>`, or null for none.

    Google's `cloud-run-gmp-sidecar` is the Cloud Run form of the
    `PodMonitoring` that left with the cluster (ADR 0024). Binary
    Authorization admits only what the platform's attestor signed, so the
    image is adopted the way the Envoy proxy was: a reviewed line in
    `infrastructure/egress/vendored-images.txt`, mirrored and attested by
    `vendor.yml`, and its digest recorded here. `catalogue.tf` composes the
    value with the registry prefix, so the upstream repository cannot be
    named and an unmirrored image cannot reach a plan.

    Null by default, and null is the closed state: no sidecar on any
    workload, and every service's `metrics_collected` output is false.
    Setting this declares a collector; it does not make
    `workload_metrics_exist` true, which stays a separate fact flipped on
    evidence a descriptor was ingested. modules/observability/NOT-SCRAPED.md
    is the record of which of the two holds.
  EOT

  type    = string
  default = null

  validation {
    condition     = var.metrics_collector_image_digest == null || can(regex("^sha256:[a-f0-9]{64}$", var.metrics_collector_image_digest))
    error_message = "The metrics collector digest is `sha256:<64 hex>` or null. A tag is a name someone can move after the attestation was signed."
  }
}

variable "vendored_openobserve_image_digest" {
  description = <<-EOT
    The digest of OpenObserve — the platform's metrics, logs and traces
    backend (ADR 0028) — as `sha256:<64 hex>`, or null to deploy nothing.

    The same adoption shape `metrics_collector_image_digest` uses, for the
    same reason: Binary Authorization admits only what the platform's own
    attestor signed, so a third-party image is adopted by mirroring it —
    a reviewed line in `infrastructure/egress/vendored-images.txt`, copied
    and attested by `vendor.yml` — rather than by exempting its upstream
    repository from the policy. `catalogue.tf` composes the value with the
    registry prefix and passes it to `modules/cloudrun` as
    `vendored_image_digest` on the one workload whose `source` is
    `"vendored"`, so the upstream repository cannot be named here and an
    unmirrored image cannot reach a plan.

    Null by default and null is the closed state: no OpenObserve service, in
    any environment, until an operator has reviewed the mirrored digest for
    that environment and named it here. Unlike the metrics collector, this is
    not a sidecar attached to an existing workload — it is its own top-level
    Cloud Run service, created only once this is set.
  EOT

  type    = string
  default = null

  validation {
    condition     = var.vendored_openobserve_image_digest == null || can(regex("^sha256:[a-f0-9]{64}$", var.vendored_openobserve_image_digest))
    error_message = "The OpenObserve digest is `sha256:<64 hex>` or null. A tag is a name someone can move after the attestation was signed."
  }
}

variable "enable_identity_platform" {
  description = "Run Google Cloud Identity Platform for customer sign-in in this environment. Customer identity only — the admin surface uses IAP and workforce identity, a separate trust model on purpose."
  type        = bool
  default     = false
}

variable "identity_authorized_domains" {
  description = "Domains customer authentication may redirect back to. Populated with real deployment outputs (Cloud Run hostnames) and, at migration, the algorik.ai domains. Never a wildcard."
  type        = list(string)
  default     = ["localhost"]
}

variable "identity_mfa_state" {
  description = "Customer MFA posture: OFF, ENABLED (optional), or MANDATORY. MANDATORY locks out every unenrolled account when it applies."
  type        = string
  default     = "ENABLED"
}

variable "console_egress_cidr" {
  description = "CIDR of the subnet Cloud Run attaches the console to for direct VPC egress. Null means the console has no route to the platform and says so on every page, which is the state this variable exists to end."
  type        = string
  default     = null
}
