output "service_account_email" {
  value = google_service_account.scout.email
}

output "records_bucket" {
  value = google_storage_bucket.records.name
}
