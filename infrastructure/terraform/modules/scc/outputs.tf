output "custom_modules" {
  description = <<-EOT
    The custom Security Health Analytics detectors this project defines, by
    name, or empty when Security Command Center is off here.
  EOT
  value = var.enable_security_command_center ? {
    binary_authorization_not_enforcing = google_scc_management_project_security_health_analytics_custom_module.binary_authorization[0].name
    cluster_control_plane_is_public    = google_scc_management_project_security_health_analytics_custom_module.private_endpoint[0].name
  } : {}
}

output "still_needs_an_organisation" {
  description = <<-EOT
    What Security Command Center cannot do from a project-scoped configuration,
    reported at plan time rather than discovered when somebody asks why the
    findings list is empty.

    The counterpart of the data module's `enabled_without_an_adapter` and the
    connectivity module's `still_needs_arranging_out_of_band`, and for the same
    reason: a gap an operator reads before applying beats one they infer from a
    console months later.
  EOT

  value = concat(
    [
      "Activation at the organisation, at Premium or Enterprise tier. Nothing in this module evaluates until that is done, and neither this module nor Terraform can check it from a project.",
      "The built-in detectors: Security Health Analytics' own library, Event Threat Detection, Container Threat Detection, VM Threat Detection. All organisation-level services.",
      "Getting findings out. Notification configs and BigQuery exports publish as a Security Command Center service agent created by the organisation's activation, so the export path is arranged with the organisation rather than here.",
      "Security posture management and org-level mute rules, which apply across projects by definition.",
    ],
    var.enable_security_command_center ? [] : [
      "Everything above, plus: enable_security_command_center is false, so this project defines no detectors at all.",
    ],
  )
}
