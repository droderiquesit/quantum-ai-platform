output "url" {
  description = <<-EOT
    Where the console is reachable: the Google-issued `run.app` URL, on a
    Google-managed certificate, with IAP in front of it.

    **Nobody types this into a registrar and nobody waits for it.** There is
    no A record, no zone, no delegated nameserver and no PROVISIONING state to
    watch — the name exists the moment Cloud Run has a service, which is the
    entire difference between this door and `modules/iap-edge`'s.

    Derived rather than read back, because the service is Config Connector's
    (ADR 0036) and this module owns no resource to read it from. Cloud Run has
    assigned this form deterministically since 2024 and `modules/cloudrun`
    computes the same one for the API. `gcloud run services describe` is the
    authority if the two ever disagree.
  EOT

  value = local.url
}

output "service_name" {
  description = "The Cloud Run service this door guards, for an operator to name in a `gcloud iap web add-iam-policy-binding` and for a test to hold the manifest to."
  value       = var.service_name
}

output "iap_members" {
  description = "Who may pass IAP and reach this service today. Empty means the door admits nobody, which is a posture rather than a failure — and, unlike ADR 0094's project-level list, an empty list here really is empty: nothing is inherited from the project onto a Cloud Run IAP resource."
  value       = var.iap_members
}

output "grant_command" {
  description = <<-EOT
    The exact command that admits one person, printed so that the one step
    this repository deliberately cannot take is not also a step somebody has
    to reconstruct. `--resource-type=cloud-run` is the part that is easy to
    get wrong: the same subcommand with no resource type edits the project's
    IAP policy instead, which is the wide grant ADR 0095 narrowed away from.
  EOT

  value = "gcloud iap web add-iam-policy-binding --project=${var.project_id} --resource-type=cloud-run --region=${var.region} --service=${var.service_name} --role=roles/iap.httpsResourceAccessor --member='user:YOU@example.com'"
}
