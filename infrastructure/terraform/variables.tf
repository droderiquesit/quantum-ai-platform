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
}

variable "subnet_cidr" {
  description = "The primary subnet range for nodes."
  type        = string
  default     = "10.0.0.0/20"
}

variable "pod_cidr" {
  description = "The secondary range for pods."
  type        = string
  default     = "10.4.0.0/14"
}

variable "service_cidr" {
  description = "The secondary range for services."
  type        = string
  default     = "10.8.0.0/20"
}

variable "authorised_networks" {
  description = <<-EOT
    Which CIDR blocks may reach the Kubernetes control plane.

    Empty by default, which means nobody: a cluster reachable from anywhere is
    not a private cluster, and defaulting to the whole internet so that the
    first apply succeeds is how that happens.
  EOT
  type = list(object({
    cidr_block   = string
    display_name = string
  }))
  default = []

  validation {
    condition = alltrue([
      for network in var.authorised_networks :
      network.cidr_block != "0.0.0.0/0"
    ])
    error_message = "0.0.0.0/0 is not an authorised network. Name the ranges that need access."
  }
}

variable "node_count" {
  description = <<-EOT
    Nodes per zone in the pool **at creation**.

    This used to be the size, full stop. Now it is the starting point and the
    autoscaler owns the rest: the node pool ignores later changes to it on
    purpose, because `initial_node_count` forces replacement and editing this
    line in a tfvars file would otherwise destroy the pool and recreate it,
    draining every pod in the cluster at once, in a plan whose summary reads
    "1 to add, 1 to destroy".

    It must sit inside `min_node_count` and `max_node_count`. The cluster
    module has a precondition that says so at plan time rather than letting the
    API refuse it after the cluster exists.
  EOT
  type        = number
  default     = 2

  validation {
    condition     = var.node_count >= 1 && var.node_count <= 20
    error_message = "The node count must be between 1 and 20."
  }
}

# --- Node pool autoscaling --------------------------------------------------
#
# Both are **per zone**, and this is a regional cluster, so the real range is
# three times each number. Reading them as regional totals sizes the pool at a
# third of what was meant.
#
# The gap these close: `qip-api` has a HorizontalPodAutoscaler with
# `maxReplicas: 6` and the pool had a fixed `node_count`, so nothing in the
# system could add a node. Past the capacity the committed nodes could hold, the
# autoscaler's answer to load was a pod in `Pending` — which looks like a
# scheduling fault and is a sizing one.

variable "min_node_count" {
  description = <<-EOT
    The smallest the pool may shrink to, per zone.

    Two, not zero and not one. Scaling down means draining, and the quiet
    period on this platform is a market that is closed followed by one that
    opens. A floor that lets the pool collapse overnight saves a few hours of
    a smaller bill and pays for it with cold starts and a wave of rescheduling
    at the open.
  EOT
  type        = number
  default     = 2

  validation {
    condition     = var.min_node_count >= 1 && var.min_node_count <= 20
    error_message = "The minimum node count must be between 1 and 20, per zone."
  }
}

variable "max_node_count" {
  description = <<-EOT
    The largest the pool may grow to, per zone.

    A ceiling, not a target: nothing scales towards it unless pods cannot be
    scheduled. It exists because the alternative to a bound is an autoscaler
    that answers a wedged workload — one stuck in a crash loop requesting four
    CPUs, say — by buying nodes until somebody reads the bill.

    Six per zone is eighteen regionally against two per zone committed: room
    for the API to reach its `maxReplicas`, for cells to be rescheduled off a
    lost node and for an upgrade to surge, without being room for an accident
    to run all day.
  EOT
  type        = number
  default     = 6

  validation {
    condition     = var.max_node_count >= 1 && var.max_node_count <= 50
    error_message = "The maximum node count must be between 1 and 50, per zone."
  }
}

variable "maintenance_exclusions" {
  description = <<-EOT
    Dated periods during which no cluster maintenance happens at all, keyed by
    name.

    Empty by default, and necessarily so: a GKE maintenance exclusion is a
    fixed pair of timestamps rather than a recurring rule, so "never during
    market hours" cannot be expressed here. The cluster's weekly window already
    puts ordinary upgrades on a Sunday, when no venue this platform trades is
    open. This is for the specific dated freeze — a quarterly roll, an exchange
    migration weekend, the fortnight around a go-live.

    At most three, and `NO_MINOR_OR_NODE_UPGRADES` may not exceed 180 days.
    `scope` defaults to `NO_MINOR_OR_NODE_UPGRADES`, which freezes the nodes —
    where the workload is — while still letting the control plane take a patch.
  EOT

  type = map(object({
    start_time = string
    end_time   = string
    scope      = optional(string, "NO_MINOR_OR_NODE_UPGRADES")
  }))

  default = {}
}

variable "enable_confidential_nodes" {
  description = <<-EOT
    Whether the cluster's nodes run as Confidential VMs, with memory encrypted
    by an AMD SEV key the host cannot read.

    **Off, and off is the decision rather than the default.** The hardening is
    real and defensible. The reason it is not simply on is the name sitting next
    to it: `crates/libs/qip-confidential` is **not** confidential computing. It
    is statistical disclosure control — a k-anonymity gate, a monotone privacy
    budget, calibrated noise — and its own module documentation says in its
    first paragraph that there is no enclave, no attestation and no hardware
    isolation.

    Enabling this alongside a crate with that name lets the two together be read
    as a guarantee neither one makes. Nothing in this platform attests a node,
    and no decision anywhere is gated on a node having been attested. Turn it on
    as defence in depth if that is what you want; do not turn it on and conclude
    that fabric D is now confidential computing.

    It is never a one-line change. The machine family must be AMD — n2d, c2d or
    c3d — and neither the `n2-standard-4` default nor production's
    `e2-standard-16` qualifies; the cluster module refuses the combination at
    plan time. Enabling it also replaces the cluster.

    modules/data/NOT-PROVISIONED.md carries the full argument.
  EOT
  type        = bool
  default     = false
}

variable "machine_type" {
  description = "The node machine type."
  type        = string
  default     = "n2-standard-4"
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

variable "edge_cells" {
  description = <<-EOT
    The edge cells this environment runs, keyed by cell id.

    ADR 0008 calls for seven, next to the venues they trade. This is a map so
    that the eighth is an entry rather than a directory: seven copies of a
    network policy is seven places for one of them to be wrong, and the wrong
    one is the one nobody reads.

    Empty by default. A cell that has not been asked for is not created, and an
    environment with no cells is a central plane on its own, which is a working
    configuration rather than a broken one.

    The seven planned locations, their cell ids and the regions they would run
    in are in docs/operations/deploying-an-edge-cell.md, together with the two
    of them that have no Google Cloud region in the right metropolitan area and
    what that costs.

    Every field is deliberate:

      * `region` is chosen for its distance to the venues, not for convenience.
      * The three CIDRs must not overlap another cell's. Overlapping ranges
        route to whichever subnet was created first, silently.
      * `venues` is empty until somebody has the venue's published address
        ranges in front of them. An empty map is no venues, not all of them.
  EOT

  type = map(object({
    region       = string
    subnet_cidr  = string
    pod_cidr     = string
    service_cidr = string
    venues = map(object({
      cidr = string
      port = number
    }))
  }))

  default = {}

  validation {
    condition = alltrue([
      for cell in values(var.edge_cells) : alltrue([
        for venue in values(cell.venues) : venue.cidr != "0.0.0.0/0" && venue.cidr != "::/0"
      ])
    ])
    error_message = "A venue range of the whole internet is not a venue range. Name the ranges the venue publishes."
  }

  validation {
    condition     = length(distinct([for cell in values(var.edge_cells) : cell.subnet_cidr])) == length(var.edge_cells)
    error_message = "Two cells share a subnet range. Overlapping ranges route to whichever subnet was created first."
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

variable "backup_location" {
  description = <<-EOT
    Where journal backups are stored. Empty means the cluster's own region.

    That default covers a failed disk, a deleted PersistentVolume, a corrupted
    journal and an operator error — four of the five losses the disaster
    recovery runbook lists, and not the fifth: a backup held in the same region
    as the cluster does not survive losing the region.

    Naming another region buys the fifth and costs cross-region transfer on
    every backup and a slower restore while everyone waits. It is a
    deployment's call rather than a default, and whichever way it goes, the
    `journal_backup` output reports which of the two this deployment has.
  EOT
  type        = string
  default     = ""
}

variable "backup_schedule" {
  description = <<-EOT
    When a journal backup is taken, as a UTC cron expression.

    Daily. A volume backup here is a persistent disk snapshot — incremental
    after the first and taken without pausing the writer — so unlike a node
    upgrade it is not constrained by market hours.

    The minute is not zero on purpose: everything scheduled on the hour in a
    Google Cloud project contends with everything else scheduled on the hour.

    Shortening it does not shorten the runbook's stated RPO for the journal,
    which is the shipping interval to the cell's mirror. This is the durable
    copy behind that, not a replacement for it.
  EOT
  type        = string
  default     = "17 3 * * *"
}

variable "backup_paused" {
  description = "Suspends the backup schedule while keeping the plan, its key and its retention. For a cluster genuinely holding nothing worth keeping — not an off switch for the control."
  type        = bool
  default     = false
}

variable "backup_retain_days" {
  description = "How long a journal backup is kept. Thirty-five days, so a corruption noticed at a month-end reconciliation still has a clean copy behind it. Deliberately not seven years: the evidence bucket is the long-horizon record, under a locked retention policy."
  type        = number
  default     = 35
}

variable "backup_delete_lock_days" {
  description = "How long a journal backup cannot be deleted by anyone, including whoever holds the permission to delete it. The window in which an operator error, or an account acting on someone else's behalf, cannot also remove the evidence of what it did."
  type        = number
  default     = 7
}

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

variable "node_disk_type" {
  description = "The central cluster's node boot disk type. See modules/cluster; development uses pd-standard to fit a fresh project's 250GB SSD quota."
  type        = string
  default     = "pd-ssd"
}

variable "node_disk_size_gb" {
  description = "The central cluster's node boot disk size, per node."
  type        = number
  default     = 100
}

variable "cluster_deletion_protection" {
  description = <<-EOT
    Whether the cluster may be destroyed at all — see modules/cluster. True
    by default; set false in any environment `infra.yml down` tears down
    between sessions, or the teardown, and any recovery from a tainted
    cluster, is refused by the provider rather than by this configuration.
  EOT
  type        = bool
  default     = true
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
