output "enabled" {
  description = <<-EOT
    Each API this configuration manages, mapped to the resource that needs it.

    Surfaced so `terraform output` answers "why is this API on in our project"
    without anyone reading the module, which is the question a security review
    asks and the one an undocumented enablement cannot answer.
  EOT
  value       = local.services
}

output "ids" {
  description = <<-EOT
    The `google_project_service` resource ids.

    Not interesting in itself. It exists so a module can take a dependency on
    the enablement having happened rather than on this module having been
    planned — see the root's `depends_on`.
  EOT
  value       = [for service in google_project_service.platform : service.id]
}
