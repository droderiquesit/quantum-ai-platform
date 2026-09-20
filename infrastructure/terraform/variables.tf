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

# --- The runtime (ADR 0022, ADR 0024, ADR 0036) ---------------------------------
#
# Every warm binary is a Cloud Run service — its identity and grants from
# `catalogue.tf`, its `RunService` manifest under
# `infrastructure/gitops/envs/<env>/` — and the execution node is a Compute
# Engine machine from `execution_nodes`; both attach to the trust zones
# declared below. The one cluster in this configuration runs controllers and
# no trading binary; it is the module behind `gitops_enabled`, and ADR 0036
# is the record that brought it back after ADR 0024 retired the last one.
#
# There is no `image_digests` variable any more. The digest a service runs at
# is in its manifest, written by Kargo's promotion commit and reconciled by
# Argo CD; `environments/<env>/images.tfvars` left with it.

variable "gitops_enabled" {
  description = <<-EOT
    Whether this environment has a GitOps control plane (ADR 0036): a GKE
    Autopilot cluster in the management trust zone running Config Connector,
    Argo CD and Kargo, and the three identities they act as.

    False by default, and false is the closed state: no cluster, no
    controller identity, no bootstrap. Nothing about the trading runtime
    reads this — Cloud Run services and execution nodes are what they are
    either way — but with it false nothing reconciles a `RunService`
    manifest into the environment, so the services in it are whatever they
    were before the release. Turning it on needs the `management` zone
    declared in `trust_zones` and a `gitops_master_ipv4_cidr_block`; the plan
    refuses either missing by name.
  EOT
  type        = bool
  default     = false
}

variable "gitops_master_ipv4_cidr_block" {
  description = <<-EOT
    The /28 the control-plane cluster's private endpoint is allocated from,
    or null where `gitops_enabled` is false.

    No default, because an address range chosen as a convenience is the one
    that collides: it must overlap neither a trust zone, the console's
    subnet nor an execution node's block (environments/README.md has the
    ladder). Null is admitted so an environment without a control plane
    need not invent one; the module's precondition refuses null the moment
    a cluster is asked for.
  EOT
  type        = string
  default     = null

  validation {
    condition     = var.gitops_master_ipv4_cidr_block == null || can(regex("/28$", coalesce(var.gitops_master_ipv4_cidr_block, "0.0.0.0/0")))
    error_message = "GKE allocates the private endpoint from exactly a /28; any other size is refused at apply, after the network peering exists."
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
    # Which of this node's venues are abroad, and under what discipline this
    # region mirrors each instrument (§31.1). Empty is every venue at home,
    # which is what every node has run as and what a node naming no file runs
    # as. It can never widen what the node trades: `venues` above is the only
    # list that decides that, and the binary refuses a declaration naming a
    # venue outside it rather than adding one.
    cross_region_mirror_path = optional(string, "")
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
    edit here, reviewed together. The default is the seven hosts the adapters
    name and the bootstrap declares; an environment that needs fewer narrows
    the bootstrap, not this list.

    Five of the seven are Google's or IBM's — infrastructure this platform
    runs on. `api.frankfurter.dev` is the first that is neither: a market-data
    vendor, reached on one path by one connector whose licensing posture is
    evaluated in `qip-data-finder`'s catalogue before the feed opens. It is
    listed here rather than folded in silently because it is the entry that
    changes what this list *is* — no longer only the platform's own
    dependencies — and the acceptance suite fails if this set and the
    bootstrap disagree in either direction.

    `router.huggingface.co` is the first that is a model vendor (ADR 0037):
    Hugging Face Inference Providers' router, reached on one path and one
    method — `POST /v1/chat/completions` — by `HuggingFaceModel` in
    `qip-reasoning-engine`, which only the deep brain constructs. What
    crosses it is the REASON stage's evidence blocks for the instruments
    under review, never a credential in the URL and never anything from
    risk, execution, capital or the edge; what comes back is narrative that
    `NumericGuard` refuses to read a number from. It is listed while nothing
    sets the three `QIP_LANGUAGE_MODEL_*` variables or mounts `QIP_HF_TOKEN`
    on any environment, so the route exists and is dark; the terms of the
    providers a chosen model resolves to are read before any environment
    turns it on, and `HuggingFaceModel::UPSTREAM_HOST`, this entry and the
    bootstrap's cluster are held to one value by the acceptance suite.
  EOT

  type = list(string)
  default = [
    "storage.googleapis.com",
    "bigquery.googleapis.com",
    "europe-west2-aiplatform.googleapis.com",
    "quantum.cloud.ibm.com",
    "api.quantum.ibm.com",
    "api.frankfurter.dev",
    "router.huggingface.co",
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

variable "deepbrain_connector" {
  description = <<-EOT
    The catalogued connector sources the deep brain polls beside its own
    stream, or null for none — every environment today.

    The deep brain is the one brain that can reach a vendor: it carries the
    egress sidecar and the fast brain deliberately does not (ADR 0008, ADR
    0024), so the pair `market_data_connector` sets on the fast brain
    configures a fetch that cannot happen while this pair configures one that
    can. `sources` is a list because rule 31 (§56.3) asks for two independent
    vendors behind a subject before promotion past validation, and a process
    fed one connector can never hold two; each entry is a manifest's
    `source_id`, rendered comma-separated as `QIP_CONNECTOR_SOURCE`, and the
    node refuses a source the licensing catalogue has not admitted before it
    builds a transport. `base_url` is the egress proxy's
    `http://127.0.0.1:<port>` address and never the vendor's —
    `qip_transport::http` refuses `https` by name — and that proxy reaches
    only the hosts its bootstrap names, so selecting a source means its
    listener exists in `infrastructure/egress/envoy.yaml` in the same change
    (today only the ECB rates' does). Both keys or neither, which the object
    type makes structural. A replay of a connector's data is not a vendor and
    never was, whatever its header says (ADR 0057, amended); this is the only
    way a deep brain's reference ledger holds a vendor's standing.
  EOT

  type = object({
    sources  = list(string)
    base_url = string
  })

  default = null

  validation {
    condition     = var.deepbrain_connector == null ? true : startswith(var.deepbrain_connector.base_url, "http://127.0.0.1:")
    error_message = "The deep brain's connector base URL is the egress proxy on loopback, http://127.0.0.1:<port>. `qip_transport::http` refuses https by name, and an address off the instance is a route that does not exist."
  }

  validation {
    condition     = var.deepbrain_connector == null ? true : length(var.deepbrain_connector.sources) > 0 && alltrue([for source in var.deepbrain_connector.sources : can(regex("^[a-z0-9]+(-[a-z0-9]+)*$", source))])
    error_message = "The deep brain's connector sources are one or more manifest ids, each lower-case letters, digits and single hyphens (e.g. frankfurter-ecb-reference-rates); an empty list names nothing to fetch and a value with a comma or a space would be rendered into QIP_CONNECTOR_SOURCE as two sources."
  }
}

variable "venue_registrations_file" {
  description = <<-EOT
    A committed JSON file of `RegistrationRecord`s the API mounts and reads as
    `QIP_VENUE_REGISTRATIONS_PATH`, or null for a deployment where nobody has
    registered with a venue.

    A path in this repository, read with `file()` at plan time exactly as the
    instrument universe is, so the records a revision carries are the records
    a reviewer read and the plan names them by hash. Null renders no
    configuration file and therefore no variable at all, and an unset variable
    is what makes the API's shipped registry hold nobody: every source whose
    requirement is an account stays refused, which is the honest state of a
    deployment where nobody registered.

    Setting this does not register anybody. `docs/operations/registering-a-venue.md`
    is the order: a named person reads the venue's terms, registers under
    their own identity, writes the credential into its Secret Manager slot
    with `gcloud secrets versions add`, and only then commits the record —
    whose `secret` field names the deployment variable the credential is read
    under and never the value. `RegistrationRecord`'s only constructor refuses
    a blank operator and its deserialiser goes through that constructor, so a
    file this names cannot say "registered" and name nobody: the API refuses
    to start on it, naming the field.
  EOT

  type    = string
  default = null

  validation {
    condition     = var.venue_registrations_file == null ? true : (can(regex("^data/[A-Za-z0-9._/-]+\\.json$", var.venue_registrations_file)) && !strcontains(var.venue_registrations_file, ".."))
    error_message = "The venue registrations file is a repository-relative path under data/ ending in .json — the data domain of ADR 0016, read with file() from the commit. An absolute path, a parent-directory hop or a path elsewhere in the tree would let a plan mount bytes no reviewer of this repository read. A `..` segment is refused outright rather than left to the character class above, which admits both `.` and `/` and so admits it too."
  }
}

variable "wallet_statement_file" {
  description = <<-EOT
    A committed JSON wallet statement the API mounts and reads as
    `QIP_WALLET_STATEMENT_PATH`, or null where no custodian has reported.

    Same convention as the universe and the registrations above: a path in
    this repository, read with `file()`, null renders no variable and the API
    then says in its banner that there is no feed and answers `/wallet` with
    `assembled: false` — the truthful answer for a process nothing has
    reported to.

    What a person setting this must know, because the platform will not soften
    it: a statement is a dated document from a counterparty, the kernel holds
    one fresh for a day, and the API refuses to start on a statement it
    considers stale or dated in the future. So a committed statement is a
    same-day act — commit the day's file, apply, and expect the refusal the
    day after — and no environment leaves it set. Every environment trades on
    the in-process simulated venue (ADR 0003), which issues no statement, so
    every tfvars leaves this null for a reason recorded there rather than
    because nobody thought about it.
  EOT

  type    = string
  default = null

  validation {
    condition     = var.wallet_statement_file == null ? true : (can(regex("^data/[A-Za-z0-9._/-]+\\.json$", var.wallet_statement_file)) && !strcontains(var.wallet_statement_file, ".."))
    error_message = "The wallet statement file is a repository-relative path under data/ ending in .json — the data domain of ADR 0016, read with file() from the commit. An absolute path, a parent-directory hop or a path elsewhere in the tree would let a plan mount bytes no reviewer of this repository read. A `..` segment is refused outright rather than left to the character class above, which admits both `.` and `/` and so admits it too."
  }
}

variable "capital_fabric_file" {
  description = <<-EOT
    A committed JSON capital-fabric declaration the API mounts and reads as
    `QIP_CAPITAL_FABRIC_PATH`, or null where the desk has declared no
    destination, corridor or transfer intent.

    Same convention as the universe, the registrations and the statement
    above: a path in this repository, read with `file()`. Null renders no
    variable, and the API then says in its banner that nothing is declared and
    answers `/transfer-gate` with `last_assessment: null` — the truthful
    answer for a platform to which no corridor has been proposed, and the
    answer every deployment gave before the declaration existed because no
    production caller could put one there at all.

    What a person setting this must know, because the platform will not soften
    it. The file is a **ledger of acts, appended to**: every command in it
    becomes a record on the hash-chained event log the first time the API
    applies it, and from then on the API refuses to start, and refuses the
    cycle, if a command that has already been journalled is edited or removed.
    Correcting a mistake means appending the act that corrects it — a revoke,
    a suspend — exactly as it would with a counterparty. A sealed record is
    not edited, and a declaration is the operator's copy of that history.

    It is also the one place a transfer *intent* is stated. The gate that
    assesses it can only veto: ADR 0021 permits the deterministic gate and
    refuses the engine behind it, an admitted verdict carries no way to
    execute, and no code in this workspace consumes one. Setting this cannot
    move money, and no value of it can.

    Every environment leaves this null today, for the reason recorded in each
    tfvars: the desk holds no external destination, and a declaration naming
    one would put an act on the chain that nobody had performed.
  EOT

  type    = string
  default = null

  validation {
    condition     = var.capital_fabric_file == null ? true : (can(regex("^data/[A-Za-z0-9._/-]+\\.json$", var.capital_fabric_file)) && !strcontains(var.capital_fabric_file, ".."))
    error_message = "The capital fabric file is a repository-relative path under data/ ending in .json — the data domain of ADR 0016, read with file() from the commit. An absolute path, a parent-directory hop or a path elsewhere in the tree would let a plan mount bytes no reviewer of this repository read. A `..` segment is refused outright rather than left to the character class above, which admits both `.` and `/` and so admits it too."
  }
}

variable "central_horizons_file" {
  description = <<-EOT
    A committed JSON §23.4 horizon policy the deep brain mounts and reads as
    `QIP_CENTRAL_HORIZONS_PATH`, or null where the desk has stated no view on
    how the whole-book risk budget divides across the four blueprint horizons
    or which horizon each strategy sits at.

    Same convention as the universe, the registrations, the statement and the
    capital fabric above: a path in this repository, read with `file()`. Null
    renders no variable, and `CentralPlane::arm_horizons` — the gate this
    document is the only production input to — stays unarmed for want of a
    stated policy, exactly the state every deployment has been in since
    `PlatformConfig::with_central` gained a caller with nothing to overlay.

    What a person setting this must know: the policy is overlaid onto
    `CentralConfig::default()` rather than replacing it, so a file need only
    state what this desk actually claims, and a claim naming a strategy the
    desk has not promoted to a capital-holding rung is refused separately
    (§23.4). Present but malformed — not valid JSON, or valid JSON that does
    not hold a `HorizonPolicy` — stops the process at start-up rather than
    arming the gate on a policy nobody actually stated, the same posture
    `wallet_statement_file` and `capital_fabric_file` already take.

    Every environment leaves this null today: no strategy here has been given
    a stated horizon, and a policy naming one would arm a gate against a claim
    nobody made.
  EOT

  type    = string
  default = null

  validation {
    condition     = var.central_horizons_file == null ? true : (can(regex("^data/[A-Za-z0-9._/-]+\\.json$", var.central_horizons_file)) && !strcontains(var.central_horizons_file, ".."))
    error_message = "The central horizons file is a repository-relative path under data/ ending in .json — the data domain of ADR 0016, read with file() from the commit. An absolute path, a parent-directory hop or a path elsewhere in the tree would let a plan mount bytes no reviewer of this repository read. A `..` segment is refused outright rather than left to the character class above, which admits both `.` and `/` and so admits it too."
  }
}

variable "risk_limits_file" {
  description = <<-EOT
    A committed JSON `LimitSet` every central root mounts and reads as
    `QIP_RISK_LIMITS_PATH`, or null where the desk runs the shipped
    `LimitSet::conservative_default` — every environment today.

    Same convention as the universe and the files above: a path in this
    repository, read with `file()`, so the bytes a revision mounts are the
    bytes in the reviewed commit and `modules/cloudrun` names the object by
    their hash. It reaches all three central roots — api, fastbrain and
    deepbrain — because each assembles its own `Platform` on its own limit
    set, and a bound moved on one brain and not the other would be two desks
    with one name.

    What a person setting this must know (ADR 0061,
    docs/operations/recalibrating-a-limit.md): this is the **only** path by
    which a bound reaches a running process. A recalibration is proposed by
    the LEARN stage from counterfactual regret, signed by two operators
    through the API, and what the second signature produces is a file — the
    running set with exactly one bound replaced — committed under
    `data/risk-limits/` and named here. Nothing installs a set after boot.
    The file is validated at start-up against the shipped set: it may move a
    bound, and it may never remove a control — a file that drops a limit the
    shipped set carries stops the process rather than running one control
    short. Present but malformed — not valid JSON, a bound that is not a
    positive number, a duplicated name — stops the process too, the posture
    every optional file here takes.

    Every environment leaves this null: no recalibration has been signed, so
    there is no artefact to mount, and the shipped set is the one every
    process has ever run under.
  EOT

  type    = string
  default = null

  validation {
    condition     = var.risk_limits_file == null ? true : (can(regex("^data/[A-Za-z0-9._/-]+\\.json$", var.risk_limits_file)) && !strcontains(var.risk_limits_file, ".."))
    error_message = "The risk limits file is a repository-relative path under data/ ending in .json — the data domain of ADR 0016, read with file() from the commit. An absolute path, a parent-directory hop or a path elsewhere in the tree would let a plan mount bytes no reviewer of this repository read. A `..` segment is refused outright rather than left to the character class above, which admits both `.` and `/` and so admits it too."
  }
}

variable "source_candidates_file" {
  description = <<-EOT
    A committed JSON catalogue of source-discovery candidates (blueprint
    §7.4-§7.6.2) the deep brain mounts and reads as
    `QIP_DEEPBRAIN_SOURCE_CANDIDATES_PATH`, or null where no candidate has
    been named.

    Same convention as the universe and the files above: a path in this
    repository, read with `file()`, deserialised as a list of
    `qip_data_finder::catalogue::CandidateEntry` — each a candidate beside
    the loopback base URL of the reviewed egress route it is probed through.
    Null renders no variable and `DiscoveryDesk` runs its pass, on whatever
    cadence `deepbrain_discover_every` names, against an empty list — the
    same outcome as today, before this caller existed at all. Present but
    malformed — including an entry naming no route, an `https` route, or a
    discovery instant after the load — stops the process rather than running
    discovery against half the candidates the desk believed it stated.

    **Setting this is the second step, not the first.** Every entry names the
    egress route its source is reached through, and under ADR 0060 a source is
    probed only where a reviewed route already exists — an Envoy cluster and a
    listener in `egress/envoy.yaml`, because the proxy is a reverse proxy and
    a process cannot name a destination in a request that has no field for one.
    Mounting a catalogue whose routes this environment's proxy does not serve
    gives the node a list it can load and cannot reach.

    What a person setting this must know: this is a list of hosts worth
    asking about, not a registration and not a source the catalogue admits.
    Nothing in this workspace discovers a candidate on its own — the CRAWL
    stage blueprint §7.4 also asks for stays unbuilt, and cannot be built
    behind a reverse proxy — so setting this cannot widen what the platform
    may reach. The routes are the boundary; this file selects among them and
    adds none.

    Every environment leaves this null today: the committed catalogue at
    `data/datasets/source-candidates.json` names one source on a route the
    bootstrap does serve, and nothing has yet been observed making that call
    from a deployed process, so mounting it would put a claim in the banner
    no run has earned.
  EOT

  type    = string
  default = null

  validation {
    condition     = var.source_candidates_file == null ? true : (can(regex("^data/[A-Za-z0-9._/-]+\\.json$", var.source_candidates_file)) && !strcontains(var.source_candidates_file, ".."))
    error_message = "The source-candidates file is a repository-relative path under data/ ending in .json — the data domain of ADR 0016, read with file() from the commit. An absolute path, a parent-directory hop or a path elsewhere in the tree would let a plan mount bytes no reviewer of this repository read. A `..` segment is refused outright rather than left to the character class above, which admits both `.` and `/` and so admits it too."
  }
}

variable "deepbrain_discover_every" {
  description = <<-EOT
    How many deep-brain research cycles run between source-discovery passes,
    read as `QIP_DEEPBRAIN_DISCOVER_EVERY`, or null to leave the desk's own
    default of zero — no discovery pass at all — in force.

    A string because it is an environment value, the same reason
    `cycle_interval_seconds` is one; `DiscoveryConfig::from_lookup` parses it
    as a cycle count and stops the process on anything else, naming the value
    it was given, rather than silently disabling the pass. Zero is a caller
    stating "never" rather than "unset" and is accepted here for the same
    reason: an operator who means it should be able to say so as a reviewed
    value rather than by deleting the line.

    Null today in every environment, and every environment also leaves
    `source_candidates_file` null, so a cadence with nothing to assess would
    run a pass that finds nothing to decide about — turning this on without a
    candidate list is a knob with no effect, and turning on both together is
    the reviewed pair.
  EOT

  type    = string
  default = null

  validation {
    condition     = var.deepbrain_discover_every == null ? true : can(regex("^[0-9]+$", var.deepbrain_discover_every))
    error_message = "The discovery cadence is a whole, non-negative number of cycles; zero disables the pass explicitly rather than by omission."
  }
}

variable "region_dark_after" {
  description = <<-EOT
    How many seconds every cell of a region may be silent before the centre
    derives the region dark (ADR 0079), read by the API as
    `QIP_REGION_DARK_AFTER`, or null to leave the derivation off.

    A string because it is an environment value, the same reason
    `cycle_interval_seconds` is one; `qip-api` parses it as whole seconds and
    stops the process on anything else, naming the variable, rather than
    running healthy with the control silently disarmed. Zero and a negative
    are refused here and at the process — a region silent for no time at all
    is every region between two reports. The process also refuses a window
    above the kernel's envelope ceiling (`MAXIMUM_ENVELOPE_VALIDITY` in
    `qip-capital`), which this validation does not restate: a copy of that
    bound here would be a second claim about one fact, and the two would
    drift.

    Null today in every environment, and deliberately so. No default,
    because there is no measurement in this tree to pick a window from (the
    ADR says so under "What it costs"). And null rather than a guess,
    because the API on Cloud Run serves no mesh (`QIP_MESH_CELLS` is unset —
    `manifest_wiring.rs` records why), so a window stated here today would
    arm a derivation over a centre that can hear no cell, and
    `/api/v1/regions` would render the window as though somebody were
    looking. Set it beside the mesh, when the fabric exists, and read the
    `region.dark` and `region.lit` journal topics to learn whether the
    number was right.
  EOT

  type    = string
  default = null

  validation {
    condition     = var.region_dark_after == null ? true : can(regex("^[1-9][0-9]*$", var.region_dark_after))
    error_message = "The dark-region window is a whole, positive number of seconds. Zero and a negative are refused because a region silent for no time at all is every region between two reports; a fraction is refused because the API reads whole seconds; and unset (null) is the derivation off, said so on the API's banner."
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

# --- Key protection level ---------------------------------------------------
#
# Blueprint §45.1 lists Cloud HSM beside Secret Manager and KMS. Google Cloud
# has no separate HSM resource: Cloud HSM *is* a KMS key whose version
# template declares `protection_level = "HSM"`. Declaring that row therefore
# means giving this configuration's keys a protection level somebody may
# choose, not adding a resource. A search for `google_cloud_hsm` will never
# match anything, in this tree or any other, because the provider has no such
# resource type — it is a question that cannot return yes, and §45.1's own
# evidence command asked it for a while.
#
# ADR 0069 declined to declare Cloud HSM on the grounds that "no custody key
# material, no signing key and no asymmetric key of any kind exists anywhere".
# That premise is not true, and was not true on the day it was written:
# `modules/binaryauthorization` has held an `ASYMMETRIC_SIGN` key since
# 2026-09-02, twelve days earlier, and ADR 0043 names it in terms — "the
# platform already meets an asymmetric-signature obligation, today". ADR 0069
# set its own reversal condition as "any asymmetric key material", to be
# checked by a grep for a caller rather than by a judgement about phase:
# `grep -rn 'purpose *= *"ASYMMETRIC_SIGN"' infrastructure/terraform
# --include=*.tf` prints that key. This is that condition firing, not the
# decision being reopened.
#
# Note the shape of that command, because the obvious version of it is broken
# in a way this repository has been bitten by before. A bare
# `grep -rln ASYMMETRIC_SIGN` over the same tree prints *two* files: the module
# and this one, because this paragraph names the string. A recount command
# that matches its own citation is a measurement instrument that reads itself,
# and it inflates by one the moment somebody quotes it. Matching the `purpose`
# assignment rather than the bare token keeps prose out of the count.
#
# `SOFTWARE` by default, and that default is the conservative one in the sense
# that matters here: it is the level all four keys already declared as
# literals, so a plan taken after this variable existed is identical to one
# taken before it. Nothing changes shape or cost until somebody sets it. An
# HSM key version bills at roughly ten times a software one, and three of the
# four keys rotate every ninety days, so each new version is a standing charge
# rather than a one-off.
#
# One value for the whole configuration, threaded to every key, because a
# posture that is HSM for the attestor and software for the evidence key reads
# to anybody asking as "the platform uses Cloud HSM" while being false of the
# key they meant. Making the mixed posture unrepresentable is worth more than
# a rule in a document saying not to build one.
#
# What this variable does not do, and it is the sharp edge: raising it on an
# environment that has already been applied does not upgrade a key.
# `version_template` is immutable on a crypto key, so Terraform plans to
# *replace* it, and all four carry `prevent_destroy = true` — the apply stops
# rather than proceeding. That is the safe failure and it is still a wedged
# apply, and the unsafe version of it would make every object under the
# evidence key unreadable while leaving the objects in place. Choose the level
# before an environment's first apply. **Terraform cannot gate this**: a
# variable validation is handed the value and never the prior state, so this
# paragraph is documentation, and the harness at
# `terraform/tests/kms-protection.tftest.hcl` does not claim to prove it.
# Saying so here is the alternative to a check that would read as protection
# and could never fire.

variable "kms_protection_level" {
  description = "Protection level for every KMS key in this configuration: SOFTWARE or HSM. One value for all four keys, so a mixed posture cannot be expressed."
  type        = string
  default     = "SOFTWARE"

  validation {
    # Cloud KMS also accepts EXTERNAL and EXTERNAL_VPC, and both are refused
    # here rather than passed through. Each needs a `google_kms_ekm_connection`
    # and a key management partner outside Google, and this configuration
    # declares neither. An environment setting one would fail deep inside the
    # provider, naming a key URI nobody configured rather than the connection
    # nobody made. A value this configuration cannot honour is refused at the
    # point where the reason is still legible.
    condition     = contains(["SOFTWARE", "HSM"], var.kms_protection_level)
    error_message = "kms_protection_level must be exactly \"SOFTWARE\" or \"HSM\". Cloud KMS is case-sensitive, so \"hsm\" is not \"HSM\". EXTERNAL and EXTERNAL_VPC are refused deliberately: each needs a google_kms_ekm_connection this configuration does not declare."
  }
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
    metrics. False until something is proven to have scraped a process; every
    workload alert policy in `modules/observability` exists only when it is
    true, because Cloud Monitoring refuses a policy naming a metric it has
    never seen. Flip it in the tfvars once there is evidence of an ingested
    descriptor — not once a deployment merely exists — and re-apply.

    This paragraph said "the four workload alert policies" for long enough
    that a reader could believe four was the number. It is not, and the count
    moves as policies are added, so recount rather than trusting a number
    written here:

      grep -c '^resource "google_monitoring_alert_policy"' modules/observability/main.tf
      grep -c 'count *=.*workload_metrics_exist'           modules/observability/main.tf

    Both answered 9 on 2026-09-06. The second must equal the first: a policy
    that escaped the gate is one that will fail the apply, and an operator
    reading a stale count in this file is how the two drifted apart before.
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

    Null by default and null is the closed state: no OpenObserve identity or
    grant in any environment until an operator has reviewed the mirrored
    digest for that environment and named it here. The service itself is
    the `RunService` manifest at infrastructure/gitops/envs/<env>/openobserve.yaml
    (ADR 0036), which must name this same digest; the parity test refuses
    the two disagreeing, and a manifest applied where this is null names an
    identity that does not exist.
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

variable "image_bake_subnet_cidr" {
  description = <<-EOT
    CIDR of the subnet the throwaway machine `.github/workflows/image.yml`
    bakes the execution node's boot image on. Null means this environment
    bakes no image, and `modules/image-bake` creates nothing at all — no
    bucket, no identity, no subnet.

    Null everywhere by default and on purpose. Three of the four environments
    will never bake an image: ADR 0035 authorises one node, in dev. A default
    range would create a subnet, a bucket and a service account in all four
    because a variable had a value, which is the shape `console_egress_cidr`
    already refuses.

    Setting it is what unblocks the boot image ADR 0035 needs and ADR 0024
    records as remaining work. The bake's own preflight refuses with this
    variable's name and the file to edit rather than failing on a bucket that
    is not there.
  EOT

  type    = string
  default = null
}

# --- The public edge (blueprint §40.5, §40.14) --------------------------------

variable "public_edge" {
  description = <<-EOT
    The customer-facing edge: Cloud Armor, the global HTTPS load balancer and
    Cloud CDN, and the hostnames they answer on.

    No hostnames by default, and no hostnames means `modules/public-edge`
    creates nothing at all — no address, no certificate, no forwarding rule,
    no policy and no bucket. Every environment leaves it that way, because no
    customer surface is deployed anywhere: an edge created because a variable
    had a default is a public address on the internet that nobody decided to
    open, and it would be reachable before anything was behind it.

    An object rather than four loose variables so that the decision is one
    thing a reviewer reads in one place. Its default is the off state rather
    than `null`, deliberately: a null here would make every reference to it a
    guarded dereference, and this repository has already shipped one guard
    that Terraform evaluated through anyway — see the note on
    `console_egress_cidr` in `modules/network/variables.tf`.

      * `hostnames` is the switch and the certificate's domain list. Google
        will not issue until each name resolves to the address the module
        allocates, so a name here is a commitment to a DNS record.
      * `application_backend` names the one Cloud Run service the edge may
        proxy to and the trust zone the catalogue places it in. The module
        refuses, at plan time, any zone but the two §46.1 marks
        client-reachable. Omit it for an edge that serves only the static
        shell.
      * `rate_limit_requests_per_minute` is §40.14's rate limit, per client
        address.
      * `permitted_regions` is §40.14's geographic policy, as ISO 3166-1
        alpha-2 codes. Empty means geography is not a control here, which is
        an honester state than an allowlist somebody guessed.
  EOT

  type = object({
    hostnames = optional(list(string), [])
    application_backend = optional(object({
      service_name = string
      trust_zone   = string
    }))
    rate_limit_requests_per_minute = optional(number, 600)
    permitted_regions              = optional(list(string), [])
  })

  default = {}

  validation {
    # The one rule this root holds rather than the module: an edge with a
    # backend and no hostnames is a backend service, a network endpoint group
    # and a security policy that no listener reaches. The module would create
    # none of them and the declaration would read, in the tfvars, as an API
    # that had been published.
    condition     = var.public_edge.application_backend == null || length(var.public_edge.hostnames) > 0
    error_message = "public_edge names an application backend and no hostnames. Nothing would be created and the tfvars would read as though the API were published. Declare the hostnames the edge answers on, or remove the backend."
  }
}

# --- the GitOps control plane's front door ------------------------------------

variable "gitops_gateway_enabled" {
  type        = bool
  description = <<-EOT
    Whether the GitOps control plane gets a public front door: a global
    external Application Load Balancer, a Google-managed certificate, and
    Identity-Aware Proxy deciding who may pass.

    False by default, and the default is the argument. ADR 0036 built this
    cluster with a private endpoint and no public address; turning this on is
    a deliberate widening of that, environment by environment, and an
    environment that never sets it keeps exactly the posture the ADR
    described. Nothing about enabling it opens the API server — the nodes and
    the endpoint stay private, and what becomes reachable is two controller
    UIs, behind an identity check Google performs before the request enters
    the VPC.
  EOT
  default     = false
}

variable "gitops_argocd_hostname" {
  type        = string
  description = "The public name Argo CD answers on. Only read when gitops_gateway_enabled."
  default     = ""
}

variable "gitops_kargo_hostname" {
  type        = string
  description = "The public name Kargo answers on. Only read when gitops_gateway_enabled."
  default     = ""
}

variable "gitops_iap_members" {
  type        = list(string)
  description = <<-EOT
    Exactly who may pass IAP and reach Argo CD or Kargo, as IAM members.

    The module refuses `allUsers` and `allAuthenticatedUsers`. This is the
    entire access list for a controller that can reconcile arbitrary
    manifests into the cluster, so it is written out per environment and
    never defaulted to something convenient.
  EOT
  default     = []
}
