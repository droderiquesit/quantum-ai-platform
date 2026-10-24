output "test_ids" {
  description = "Each declared connectivity test's id, by name."
  value       = { for name, test in google_network_management_connectivity_test.this : name => test.id }
}
