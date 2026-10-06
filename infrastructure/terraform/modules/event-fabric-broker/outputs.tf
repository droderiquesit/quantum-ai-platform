output "instance_template_self_link" {
  value       = try(google_compute_instance_template.event_fabric_broker[0].self_link, null)
  description = "Self link of the event fabric broker instance template"
}

output "instance_template_name" {
  value       = try(google_compute_instance_template.event_fabric_broker[0].name, null)
  description = "Name of the event fabric broker instance template"
}

output "broker_dns_name" {
  value       = try(google_dns_record_set.event_fabric_broker_srv[0].name, null)
  description = "Internal DNS A record name for broker discovery (FABRIC-087, FABRIC-089)"
}

output "broker_srv_record" {
  value       = try(google_dns_record_set.event_fabric_broker_srv_tcp[0].name, null)
  description = "SRV record name for broker service discovery (FABRIC-087)"
}

output "broker_srv_rdata" {
  value       = try(google_dns_record_set.event_fabric_broker_srv_tcp[0].rrdatas[0], null)
  description = "SRV record data pointing to broker A record"
}
