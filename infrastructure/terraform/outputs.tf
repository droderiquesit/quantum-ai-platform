# Outputs.
#
# Nothing here is a secret. The service URLs and the service-account emails
# are needed to deploy; the credentials they authenticate with are in Secret
# Manager and never in state or output.

output "cloud_run_services" {
  description = <<-EOT
    Every workload in the catalogue: its Cloud Run URL, its identity, the
    trust zone it attaches through, whether it carries the egress proxy, and
    whether a metrics collector is declared beside it — declared, which is
    not scraped; `workload_metrics_exist` is the fact about ingestion.

    The URL is internal — every service is `INGRESS_TRAFFIC_INTERNAL_ONLY` —
    so a request arriving at it from the internet is refused before the
    container sees it. It is what `deploy.yml` moves and what the console is
    configured to call.
  EOT

  value = {
    for name, workload in module.cloud_run : name => {
      uri               = workload.uri
      service_account   = workload.service_account_email
      trust_zone        = workload.trust_zone
      has_egress_proxy  = workload.has_egress_proxy
      metrics_collected = workload.metrics_collected
      network_tags      = workload.network_tags
    }
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
