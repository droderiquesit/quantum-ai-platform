# Outputs.
#
# Nothing here is a secret. The endpoint and the service-account emails are
# needed to deploy; the credentials they authenticate with are in Secret
# Manager and never in state or output.

output "cluster_name" {
  description = "The cluster's name."
  value       = module.cluster.name
}

output "cluster_endpoint" {
  description = "The private control-plane endpoint."
  value       = module.cluster.endpoint
  # Not a secret, but not something to print in a CI log either.
  sensitive = true
}

output "service_account_emails" {
  description = "The workload identity service accounts, one per deployable."
  value       = module.secrets.service_account_emails
}

output "autonomy_ceiling" {
  description = <<-EOT
    The highest autonomy level this environment's platform may reach.

    Surfaced as an output so an operator can answer "could this cluster trade
    live" from the infrastructure rather than by reading a config map.
  EOT
  value       = var.autonomy_ceiling
}

output "live_capable" {
  description = "Whether this environment is permitted to reach a real venue at all."
  value       = var.autonomy_ceiling != "paper_trading"
}

output "image_prefix" {
  description = <<-EOT
    The prefix every image reference starts with.

    Needed by the pipeline to tag what it pushes and by the manifests to name
    what they pull, so it comes from the infrastructure rather than being
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
    Set this as the GitHub repository variable GCP_WORKLOAD_IDENTITY_PROVIDER.

    Until it is set, the deploy workflow cannot authenticate and says so. That
    is the intended failure: the alternative is a service-account key in a
    repository secret, which is a credential that never expires and leaves no
    record of which run used it.
  EOT

  value = module.cicd.workload_identity_provider
}

output "deploy_service_account" {
  description = "Set this as the GitHub repository variable GCP_DEPLOY_SERVICE_ACCOUNT."
  value       = module.cicd.service_account_email
}

output "edge_cells" {
  description = <<-EOT
    Each cell's identity, subnet, node tag and permitted venues.

    The node tag matters: every firewall rule constraining a cell targets it,
    and a rule targeting a tag nothing carries does nothing silently.
  EOT

  value = {
    for id, cell in module.edge_cell : id => {
      service_account            = cell.service_account_email
      kubernetes_service_account = cell.kubernetes_service_account
      subnet_id                  = cell.subnet_id
      node_tag                   = cell.node_tag
      venues                     = cell.venues
    }
  }
}

output "binary_authorization_attestor" {
  description = <<-EOT
    Set this as the GitHub repository variable `GCP_BINAUTHZ_ATTESTOR`.

    Until it and `GCP_BINAUTHZ_KEY_VERSION` are set, `deploy.yml` refuses to
    build. That is the intended failure: the alternative is a pipeline that
    builds and pushes four images it cannot sign, and a cluster that refuses
    every one of them at admission with no indication of why.
  EOT

  value = module.binary_authorization.attestor_name
}

output "binary_authorization_key_version" {
  description = "Set this as the GitHub repository variable `GCP_BINAUTHZ_KEY_VERSION`. The fully qualified KMS key version the pipeline signs with; the private half never leaves KMS."
  value       = module.binary_authorization.attestor_key_version
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

    The counterpart of the data module's `enabled_without_an_adapter`, and for
    the same reason: a gap an operator reads at plan time rather than
    discovering at cutover.
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

output "node_pool_bounds" {
  description = <<-EOT
    What the node pool may grow and shrink to, per zone and regionally.

    Both forms, because they are three apart and confusing them is how a pool
    ends up a third of the intended size. A HorizontalPodAutoscaler's ceiling is
    only a policy if the regional maximum can hold it: `qip-api` asks for six
    replicas at 250m, which is why nothing being able to add a node made that
    ceiling a capacity limit instead.
  EOT
  value       = module.cluster.node_pool_bounds
}

output "confidential_nodes" {
  description = <<-EOT
    Whether the node pool's memory is encrypted by an AMD SEV key.

    Surfaced so the answer comes from the infrastructure rather than from a
    crate's name. `crates/libs/qip-confidential` is statistical disclosure
    control with no enclave and no attestation, and this being true does not
    change that; see the variable and modules/data/NOT-PROVISIONED.md.
  EOT
  value       = module.cluster.confidential_nodes
}

output "journal_backup" {
  description = <<-EOT
    What the edge cell journal backups cover, and where that stops.

    `survives_region_loss` is the field to read: it is false whenever backups
    are stored in the cluster's own region, which is the default.
    `protected_pod_count` is the other one — a plan whose namespace selector
    matches nothing succeeds, reports healthy and protects zero pods, which is
    indistinguishable from a working backup until somebody needs one.

    modules/backup/NOT-COVERED.md says what is deliberately excluded, including
    the positions and open orders that the disaster-recovery runbook insists are
    reconciled from the venue and never restored.
  EOT
  value = merge(
    module.backup.coverage,
    {
      plan                = module.backup.plan_name
      protected_pod_count = module.backup.protected_pod_count
      snapshot_schedule   = module.backup.snapshot_schedule_name
    },
  )
}

output "journal_snapshot_attachment_command" {
  description = <<-EOT
    The command that attaches the journal snapshot schedule to the journal
    disks, and the reason it is an output instead of a resource.

    A Compute Engine resource policy attaches to a disk. The journal disks are
    named `pvc-<uuid>` and are created by the CSI driver when a cell's pod is
    first scheduled — after any apply, with a name nothing could have
    predicted. `infrastructure/kubernetes/base/journal-storage.yaml` labels them
    `qip-journal=true` for exactly this reason; this is the other end of that
    arrangement.

    Until it has been run for a given disk, that disk is covered by the GKE
    backup plan and by nothing else — which is enough until somebody deletes
    the claim, at which point it is covered by nothing at all.

    Run it after a cell's first pod is running, and again after adding a cell.
    `docs/operations/disaster-recovery.md` carries it as a numbered step.
  EOT
  value       = module.backup.snapshot_attachment_command
}

output "security_command_center_still_needs_an_organisation" {
  description = <<-EOT
    What Security Command Center cannot do from a project-scoped configuration.

    The counterpart of the data module's `enabled_without_an_adapter` and the
    connectivity module's `still_needs_arranging_out_of_band`: a gap read at
    plan time beats one inferred from an empty findings list months later. The
    entry that matters most is the first — nothing this project defines
    evaluates at all until SCC is activated at the organisation, and a project
    cannot tell whether it has been.
  EOT
  value       = module.scc.still_needs_an_organisation
}

output "infra_service_account" {
  description = <<-EOT
    Set this as the GitHub repository variable GCP_INFRA_SERVICE_ACCOUNT.

    It is what infra.yml — the manually dispatched workflow that plans,
    applies and tears down the stack — authenticates as, so an operator or an
    agent can iterate the infrastructure from the repository with no key in
    existence. See modules/cicd for what bounds it.
  EOT
  value       = module.cicd.infra_service_account
}
