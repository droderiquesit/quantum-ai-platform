# Outputs.
#
# Nothing here is a secret. The service URLs and the service-account emails
# are needed to deploy; the credentials they authenticate with are in Secret
# Manager and never in state or output.

output "cloud_run_services" {
  description = <<-EOT
    Every workload in the catalogue: its Cloud Run URL, its identity, the
    trust zone it attaches through, whether it carries the egress proxy,
    whether a metrics collector is declared beside it — declared, which is
    not scraped; `workload_metrics_exist` is the fact about ingestion — and
    the environment, secret paths and configuration paths its manifest must
    carry, so the parity test reads them from one place.

    The URL is internal — every catalogue service is
    `INGRESS_TRAFFIC_INTERNAL_ONLY` in its manifest — so a request arriving
    at it from the internet is refused before the container sees it. It is
    computed from Cloud Run's deterministic form rather than read back: the
    service resource is Config Connector's (ADR 0036).
  EOT

  value = {
    for name, workload in module.cloud_run : name => {
      uri                 = workload.uri
      service_account     = workload.service_account_email
      trust_zone          = workload.trust_zone
      has_egress_proxy    = workload.has_egress_proxy
      metrics_collected   = workload.metrics_collected
      network_tags        = workload.network_tags
      environment         = workload.environment
      secret_file_paths   = workload.secret_file_paths
      config_file_paths   = workload.config_file_paths
      config_files_bucket = workload.config_files_bucket
    }
  }
}

output "gitops_control_plane" {
  description = <<-EOT
    The control-plane cluster and the three identities on it (ADR 0036), or
    null where `gitops_enabled` is false.

    `infra.yml`'s bootstrap derives the cluster name the same way the module
    does and refuses to apply if the cluster is absent; the identities are
    what the bootstrap writes into the controllers' service accounts.
  EOT

  value = length(module.gitops_control_plane) == 0 ? null : {
    cluster_name    = module.gitops_control_plane[0].cluster_name
    location        = module.gitops_control_plane[0].cluster_location
    kcc_identity    = module.gitops_control_plane[0].kcc_service_account_email
    argocd_identity = module.gitops_control_plane[0].argocd_service_account_email
    kargo_identity  = module.gitops_control_plane[0].kargo_service_account_email
    etcd_key        = module.gitops_control_plane[0].etcd_key_id
  }
}

output "service_account_emails" {
  description = "The Cloud Run identities, one per deployable, keyed by catalogue name."
  value       = { for name, workload in module.cloud_run : name => workload.service_account_email }
}

output "autonomy_ceiling" {
  description = <<-EOT
    The highest autonomy level this environment's platform may reach.

    Surfaced as an output so an operator can answer "could this deployment
    trade live" from the infrastructure rather than by reading a service's
    environment.
  EOT
  value       = var.autonomy_ceiling
}

output "live_capable" {
  description = <<-EOT
    Whether this environment is permitted to reach a real venue at all.

    False for every ceiling a plan can carry, because `variables.tf` refuses
    the three that are not. It was `var.autonomy_ceiling != "paper_trading"`,
    which is that sentence backwards: it answered true for `observation` and
    `advisory` — the two rungs below paper trading, and the ones an operator
    reaches for when hardening an environment. See `ceiling_reaches_a_venue` in
    main.tf, which is now the only expression that answers this question.
  EOT
  value       = local.ceiling_reaches_a_venue
}

output "image_prefix" {
  description = <<-EOT
    The prefix every image reference starts with.

    Needed by the pipeline to tag what it pushes and by the catalogue to name
    what it runs, so it comes from the infrastructure rather than being
    written down twice.
  EOT

  value = module.registry.image_prefix
}

output "evidence_bucket" {
  description = "The write-once evidence bucket, for the mesh's evidence configuration."
  value       = module.evidence.bucket_name
}

output "workload_identity_provider" {
  description = <<-EOT
    The provider the pipeline authenticates against. Informational: deploy.yml,
    vendor.yml and infra.yml derive it from the committed tfvars rather than
    from a repository variable, and the acceptance suite refuses a workflow
    that reads one.
  EOT

  value = module.cicd.workload_identity_provider
}

output "deploy_service_account" {
  description = "The pipeline's account. Derived by the workflows from the tfvars; surfaced here for an operator checking a grant."
  value       = module.cicd.service_account_email
}

output "infra_service_account" {
  description = <<-EOT
    The account infra.yml — the manually dispatched workflow that plans,
    applies and tears down the execution nodes — authenticates as, so an
    operator or an agent can iterate the infrastructure from the repository
    with no key in existence. See modules/cicd for what bounds it.
  EOT
  value       = module.cicd.infra_service_account
}

output "binary_authorization_attestor" {
  description = "The attestor the pipeline signs for. Derived by deploy.yml from the tfvars; surfaced here for an operator checking the policy."
  value       = module.binary_authorization.attestor_name
}

output "binary_authorization_key_version" {
  description = "The fully qualified KMS key version the pipeline signs with; the private half never leaves KMS."
  value       = module.binary_authorization.attestor_key_version
}

output "egress_proxy" {
  description = <<-EOT
    The egress proxy every rendering runs: the mirrored image by digest, the
    hosts the published bootstrap dials, and the loopback address each
    listener answers on. The whole external surface of the platform's
    outbound path in one place, which is the form a review is done on.
  EOT

  value = {
    image     = module.egress_proxy.image
    upstreams = module.egress_proxy.dialled_upstreams
    endpoints = module.egress_proxy.endpoints
  }
}

output "trust_zones" {
  description = <<-EOT
    Each declared zone's subnet, its network tag and the identities placed in
    it; the paths that exist between zones; every destination outside the
    VPC any zone may reach; and which zones hold any route out at all.

    `zones_with_external_egress` is expected to be a short list and to stay
    one. A zone appearing there that was not expected to is the finding.
  EOT

  value = {
    subnets                    = module.trust_zones.zone_subnets
    network_tags               = module.trust_zones.zone_network_tags
    identities                 = module.trust_zones.zone_identities
    permitted_paths            = module.trust_zones.permitted_paths
    external_egress            = module.trust_zones.external_egress_destinations
    zones_with_external_egress = module.trust_zones.zones_with_external_egress
  }
}

output "execution_nodes" {
  description = <<-EOT
    Each node's identity, subnet, network tag, instance group, isolated core
    range, and whether it is in shadow mode and whether the venue credential
    is bound to it.

    Empty in every environment today. `shadow_mode` is what a report of ADR
    0020 step 3's state cites rather than asserts, and `venue_credential_bound`
    is false unless the ceiling permits live trading *and* the node is out of
    shadow mode *and* a secret was named.
  EOT

  value = {
    for id, node in module.execution_node : id => {
      service_account        = node.service_account_email
      subnet_id              = node.subnet_id
      node_tag               = node.node_tag
      instance_group         = node.instance_group
      isolated_cpus          = node.isolated_cpus
      shadow_mode            = node.shadow_mode
      venue_credential_bound = node.venue_credential_bound
    }
  }
}

output "interconnect_pairing_keys" {
  description = <<-EOT
    Attachment name to the pairing key its partner needs, or empty when partner
    interconnect is off.

    Sensitive because it is a bearer token in everything but name: whoever
    holds it can attach a circuit of theirs to a VLAN attachment of this
    project's. Read it deliberately, with
    `terraform output -json interconnect_pairing_keys`, and hand it over
    through the partner's ordering process rather than a CI log.
  EOT

  value     = module.connectivity.pairing_keys
  sensitive = true
}

output "interconnect_attachments" {
  description = "Each attachment's region, edge availability domain and state. `PENDING_PARTNER` means Google is still waiting for the partner's half."
  value       = module.connectivity.interconnect_attachments
}

output "private_connectivity_still_needed" {
  description = <<-EOT
    What a deployment must still arrange elsewhere for the private path to
    carry traffic: a circuit against each pairing key, somebody enabling an
    attachment after reviewing its far end, and DNS at the colocated site.
  EOT

  value = module.connectivity.still_needs_arranging_out_of_band
}

output "enabled_apis" {
  description = <<-EOT
    Every Google API this configuration manages, mapped to the resource that
    needs it.

    The answer to "why is this API on in our project", which is what a security
    review asks and what an enablement performed by hand cannot answer. It is
    also the list to read before a destroy: none of these is turned off by one,
    because disabling an API deletes the resources under it rather than merely
    revoking access.
  EOT
  value       = module.services.enabled
}

output "journal_backup" {
  description = <<-EOT
    What the journal snapshots cover, and where that stops.

    `covers_before_attach` is the field to read: the schedule protects a disk
    only once `journal_snapshot_attachment_command` has been run for it, and
    until then the answer is nothing. modules/backup/NOT-COVERED.md says what
    is deliberately excluded, including the positions and open orders that
    the disaster-recovery runbook insists are reconciled from the venue and
    never restored.
  EOT
  value = merge(
    module.backup.coverage,
    {
      snapshot_schedule = module.backup.snapshot_schedule_name
    },
  )
}

output "journal_snapshot_attachment_command" {
  description = <<-EOT
    The command that attaches the journal snapshot schedule to every journal
    disk, and the reason it is an output instead of a resource.

    A Compute Engine resource policy attaches to a disk. A node's disk is
    created by its managed instance group when the instance is built — after
    any apply, under a name the group chose — and the instance template labels
    it `qip_journal=true` for exactly this reason; this is the other end of
    that arrangement. Run it after a node's first boot, and again after every
    replacement. `docs/operations/disaster-recovery.md` carries it as a
    numbered step.
  EOT
  value       = module.backup.snapshot_attachment_command
}

output "security_command_center_still_needs_an_organisation" {
  description = <<-EOT
    What Security Command Center cannot do from a project-scoped configuration.

    A gap read at plan time beats one inferred from an empty findings list
    months later. The entry that matters most is the first — nothing this
    project defines evaluates at all until SCC is activated at the
    organisation, and a project cannot tell whether it has been.
  EOT
  value       = module.scc.still_needs_an_organisation
}

output "identity_frontend_environment" {
  description = "Environment keys the Algorik applications read for customer identity. Empty until an environment enables identity. The browser API key is deliberately not an output — it is delivered through configuration, never round-tripped through Terraform output into logs."
  value       = module.identity.frontend_environment
}

# --- The console's route to the platform (ADR 0018) --------------------------
#
# `scripts/deploy-frontends.sh` reads these. They are outputs rather than
# constants in the script because the script deploying against a value
# Terraform did not create is the drift this arrangement exists to prevent.

output "console_egress_subnet" {
  description = "The subnet the console attaches to, or null where it has no route to the platform."
  value       = module.network.console_egress_subnet
}

output "api_internal_base_url" {
  description = <<-EOT
    The value QIP_API_BASE_URL takes on the console: the API's own Cloud Run
    URL. Internal ingress, so it answers only a caller inside the VPC — the
    console's direct VPC egress — and only one the catalogue names as an
    invoker, which is the console's identity.

    It replaces the reserved internal-load-balancer address the GKE runtime
    needed: there is no load balancer between the console and the API now,
    and no address to reserve. The console speaks HTTPS to it; the platform's
    own binaries could not, and do not call the API.
  EOT
  value       = module.cloud_run["api"].uri
}

output "console_service_account_email" {
  description = "The identity scripts/deploy-frontends.sh must deploy the portal under. Null where the console has no platform to read."
  value       = module.secrets.console_service_account_email
}

# --- What the boot-image bake runs on ---------------------------------------
#
# `.github/workflows/image.yml` does not read these: it derives every name the
# way `deploy.yml` derives the attestor's, from the environment's committed
# tfvars and this module's naming rule, so nothing in the pipeline depends on
# an output somebody pasted. They are here for the plan a person reads before
# applying, and for `terraform output` to answer "did the bake's preflight
# refuse because this environment has none, or because the apply failed".

output "image_bake" {
  description = <<-EOT
    The staging bucket, the builder identity and the builder subnet the boot
    image is baked on, or null where `image_bake_subnet_cidr` is unset and this
    environment bakes nothing.

    The image itself is not here and must never be: an image Terraform owned
    could be destroyed while a node's instance template still named it.
  EOT

  value = length(module.image_bake) == 0 ? null : {
    payload_bucket  = module.image_bake[0].payload_bucket
    builder_account = module.image_bake[0].builder_service_account_email
    builder_subnet  = module.image_bake[0].builder_subnet
    builder_tag     = module.image_bake[0].builder_tag
  }
}

output "public_edge" {
  description = <<-EOT
    The customer-facing edge (§40.5, §40.14): whether it exists at all, the
    global address its hostnames must resolve to, the Cloud Armor policy its
    backends are attached to, and the bucket the static shell is published to.

    `exists` is false in every environment and the other fields are null,
    because no environment declares a hostname. Surfaced anyway, and named
    `exists` rather than left to be inferred from a null address: "there is no
    public edge here" is a fact an operator should be able to read, and an
    empty output reads identically to an apply that failed halfway.
  EOT

  value = {
    exists          = module.public_edge.enabled
    address         = module.public_edge.address
    hostnames       = module.public_edge.hostnames
    security_policy = module.public_edge.security_policy
    shell_bucket    = module.public_edge.static_shell_bucket
  }
}

output "kms_protection_level" {
  description = <<-EOT
    The protection level this environment's KMS keys are planned with —
    `SOFTWARE`, or `HSM` where Cloud HSM has been chosen.

    Taken from the key the secrets module plans rather than from the variable,
    so an operator reading it is reading what the configuration would build and
    not what it was asked for. One value covers all four keys by construction:
    the root passes `var.kms_protection_level` to every module that owns one,
    and a mixed posture has no way to be expressed.

    This does not say what an applied environment holds. `version_template` is
    immutable on a crypto key, so an environment applied at `SOFTWARE` stays
    `SOFTWARE` until its keys are replaced, and `prevent_destroy` stops that
    replacement rather than performing it. Compare this against the project
    before believing either.
  EOT

  value = module.secrets.key_protection_level
}

output "deploy_attribute_condition" {
  description = <<-EOT
    The condition deciding which GitHub repository may federate into this
    project, as the workload identity pool provider is planned with it.

    Published so the deployment trust boundary can be asserted on without a
    credential. `.claude/rules/domains/infrastructure.md` records why this
    value has a harness rather than only a validation: an identity derived from
    a repository variable once carried an apt-install advisory into the
    workload-identity audience, and every run afterwards failed on an audience
    nobody could explain. The identity is derived from committed tfvars, and
    `tests/deploy-identity.tftest.hcl` proves the gate on that value both
    refuses a malformed repository and admits a well-formed one.

    Reading it is not the same as checking the project. This is what the
    configuration would build; compare it against the deployed provider before
    believing either.
  EOT

  value = module.cicd.deploy_attribute_condition
}

output "console_front_door" {
  description = <<-EOT
    **Where the console actually is, and the command that admits one person to
    it.** ADR 0095.

    This is the output to read for the **grant**. `portal_front_door` below
    describes the custom-domain door, which no environment turns on; this one
    describes whichever door the environment actually has, and in every
    environment today that is the Google-issued one:

        url  = https://qip-dev-portal-<project-number>.us-east4.run.app

    **`url` is a derivation and in `algorik-dev` it is the wrong family of
    hostname. Read the address off the service instead** — `infra.yml`'s
    `diagnose` action prints `status.url` per service, applies nothing and
    costs a minute. Run 35636247990 (`diagnose`, dev, 2026-09-21) read the
    one service in that project with `RoutesReady=True` and got the legacy
    `<service>-<token>-<region-code>.a.run.app` form, not the
    `<service>-<project-number>.<region>.run.app` form this output builds.
    `modules/iap-run`'s `url` output argues why the derivation is kept anyway
    — a `data` source read fails the plan wherever the portal is not deployed,
    which is everywhere — and the argument only holds because the reading is
    cheap and named here.

    `grant` is unaffected by any of that: it names a project, a region and a
    service, never a hostname.

    No A record, no zone, no registrar, no nameserver delegation, and no
    certificate to watch leave PROVISIONING. The name exists as soon as Cloud
    Run has a service, on a certificate Google manages — whichever family it
    belongs to.

    `mode` says which door answered, because the two have different failure
    modes and an operator reading a URL cannot tell them apart:
    `cloud-run-iap` is IAP on the service itself, `load-balancer-iap` is
    ADR 0094's edge. Null only in an environment with no console at all —
    `console_egress_cidr` unset, so there is no identity for the portal to run
    as.

    **`grant` is the one step this repository deliberately does not take.**
    The access list is empty in every environment because an IAM member is an
    account identifier and `.claude/rules/00-enterprise-governance.md` refuses
    one in a committed file, so the door comes up admitting nobody. Run the
    printed command to admit yourself. `--resource-type=cloud-run` is the part
    that is easy to lose: the same subcommand without it edits the project's
    IAP policy, which is the wide grant ADR 0095 narrowed away from.
  EOT

  value = length(module.portal_iap_run) > 0 ? {
    mode  = "cloud-run-iap"
    url   = module.portal_iap_run[0].url
    grant = module.portal_iap_run[0].grant_command
    } : length(module.portal_edge) > 0 ? {
    mode  = "load-balancer-iap"
    url   = module.portal_edge[0].url
    grant = "gcloud projects add-iam-policy-binding ${var.project_id} --role=roles/iap.httpsResourceAccessor --member='user:YOU@example.com'"
  } : null
}

output "portal_front_door" {
  description = <<-EOT
    The portal's **custom-domain** IAP front door (ADR 0094, narrowed by ADR
    0095): the hostname, the URL, and the reserved global address its A record
    must point at. Null in an environment that sets no
    `gitops_portal_hostname` — which is now every environment, because the
    console is reached at its Google-issued `run.app` URL instead. Read
    `console_front_door` above for where the console actually is.

    **The A record is the one step this repository cannot perform.**
    `algorik.ai` answers from nameservers outside this project, so somebody
    creates the record at the registrar by hand, and Google's managed
    certificate stays in PROVISIONING until it resolves. That is the feedback
    that the step was done; there is no plan or apply that can tell you.

    Who may pass IAP is not here either. It is the project-level
    `roles/iap.httpsResourceAccessor` grant the tfvars describe — one list for
    this door and the GitOps Gateway both — and `gitops_iap_members` is empty
    on purpose, so the door comes up admitting nobody.
  EOT

  value = length(module.portal_edge) == 0 ? null : {
    hostname = module.portal_edge[0].hostname
    url      = module.portal_edge[0].url
    address  = module.portal_edge[0].address
  }
}

output "dns_zone" {
  description = <<-EOT
    The domain this environment is authoritative for, and **the four
    nameservers that are the one remaining manual step in its life**. Null in
    an environment that sets no `dns_zone_domain`, which is every environment
    but dev.

    Do this once, and never again per record: at the registrar that holds the
    domain, replace every nameserver with the four in `nameservers`. Nothing
    else there changes — not the ownership, not the contacts, not the renewal.
    From the moment it propagates, every name under the domain is answered out
    of this state file, and a new front door is an A record in a commit.

    `dig +short NS <domain>` returning these four is how you know it took. The
    Google-managed certificates on the front doors leave PROVISIONING within
    about fifteen minutes of that, and the doors serve.

    `ds_record` is **not** part of that step and must not be done alongside it.
    Replacing nameservers is reversible in minutes; publishing a DS makes the
    whole domain's resolution depend on this zone continuing to exist with
    these keys, and `infra.yml down` destroys this zone. The module's output
    description spells out what that costs.
  EOT

  value = length(module.dns_zone) == 0 ? null : {
    domain         = module.dns_zone[0].domain
    zone_name      = module.dns_zone[0].zone_name
    nameservers    = module.dns_zone[0].nameservers
    records        = module.dns_zone[0].records
    dnssec_enabled = module.dns_zone[0].dnssec_enabled
    ds_record      = module.dns_zone[0].ds_record
  }
}
