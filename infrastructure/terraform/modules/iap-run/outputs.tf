output "url" {
  description = <<-EOT
    The address Cloud Run's **documented** `run.app` naming convention would
    assign this service: the service name, the project number, the region.

    **Do not paste this into a browser without checking it against the
    service, because in `algorik-dev` it is wrong.** That is a measurement
    rather than a worry. `infra.yml` run 35636247990 (`diagnose`, dev,
    2026-09-21) read `status.url` off the one service in that project with a
    Ready route, and the project issues the **legacy** form — service name, a
    project-and-region token Terraform cannot compute, a two-letter region
    code, `.a.run.app`. The new form this output builds is what Google
    documents for services created since 2024; it is not what this project
    hands out, and a hostname of the wrong family does not resolve at all.

    So the sentence this description used to end on — "`gcloud run services
    describe` is the authority if the two ever disagree" — was right about
    which is the authority and wrong to treat the disagreement as
    hypothetical. They disagree today.

    It is still derived rather than read back, and deliberately: the service
    is Config Connector's (ADR 0036), this module owns no resource to read it
    from, and a `data` source on a service that does not exist yet fails the
    plan for every environment whose portal has not been deployed — which is
    all of them. A derivation that is checkable beats a plan that cannot run.
    What makes it safe is that the check is cheap and named everywhere this
    value is printed: `infra.yml`'s `diagnose` action prints each service's
    `status.url`, applies nothing, and takes under a minute.

    **Nobody types either form into a registrar and nobody waits for it.**
    There is no A record, no zone, no delegated nameserver and no
    PROVISIONING state to watch — the name exists the moment Cloud Run has a
    service, which is the entire difference between this door and
    `modules/iap-edge`'s. That much is unaffected by which family the
    hostname belongs to.
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
