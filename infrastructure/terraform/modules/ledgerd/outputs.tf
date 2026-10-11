output "service_account_email" {
  description = "qip-ledgerd's machine identity."
  value       = google_service_account.ledgerd.email
}

output "service_account_id" {
  description = "The ledger identity's account id: configuration, so known at plan where the email is not. The root writes the CA Service grant and the SAN from it."
  value       = google_service_account.ledgerd.account_id
}

output "read_api" {
  description = "The read-side API's internal address and port."
  value       = "${var.internal_address}:${var.listen_port}"
}

output "instance_name" {
  description = "The ledger machine's name."
  value       = google_compute_instance.ledgerd.name
}
