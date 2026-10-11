output "service_account_email" {
  description = "qip-ledgerd's machine identity."
  value       = google_service_account.ledgerd.email
}

output "read_api" {
  description = "The read-side API's internal address and port."
  value       = "${var.internal_address}:${var.listen_port}"
}

output "instance_name" {
  description = "The ledger machine's name."
  value       = google_compute_instance.ledgerd.name
}
