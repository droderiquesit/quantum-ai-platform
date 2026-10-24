output "project_ids" {
  description = "Each plane's project id, keyed environment/plane or environment/plane-region."
  value       = { for key, p in local.projects : key => p.project_id }
}
