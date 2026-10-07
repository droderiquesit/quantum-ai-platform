# The Data and Engineering VPCs, planned (GCP-051).
#
# GCP-051 requires the Data and Engineering planes to run on separate VPCs,
# isolated from the shared VPC that hosts the Service plane (trust zones) and
# Reflex (execution node). This test asserts the two separate networks exist
# and are configured identically to the shared VPC: deny-all ingress, private
# Google APIs zone. Neither hosts workload subnets or console egress — those
# remain the shared VPC's responsibility.
#
# Runs against a mocked provider, so it needs no credential and reaches no
# project; `command = plan` throughout, so nothing is applied even in the mock.

mock_provider "google" {}

variables {
  project_id  = "network-plan-harness"
  region      = "us-east4"
  environment = "dev"
  labels      = {}
}

run "the_data_vpc_exists_and_is_globally_scoped" {
  command = plan

  assert {
    condition     = google_compute_network.data_vpc.name != ""
    error_message = "the Data VPC must exist as a separate google_compute_network resource"
  }

  assert {
    condition     = google_compute_network.data_vpc.routing_mode == "REGIONAL"
    error_message = "the Data VPC must use REGIONAL routing like the shared VPC"
  }

  assert {
    condition     = google_compute_network.data_vpc.auto_create_subnetworks == false
    error_message = "the Data VPC must not auto-create subnetworks; subnets are declared, not automatic"
  }
}

run "the_engineering_vpc_exists_and_is_globally_scoped" {
  command = plan

  assert {
    condition     = google_compute_network.engineering_vpc.name != ""
    error_message = "the Engineering VPC must exist as a separate google_compute_network resource"
  }

  assert {
    condition     = google_compute_network.engineering_vpc.routing_mode == "REGIONAL"
    error_message = "the Engineering VPC must use REGIONAL routing like the shared VPC"
  }

  assert {
    condition     = google_compute_network.engineering_vpc.auto_create_subnetworks == false
    error_message = "the Engineering VPC must not auto-create subnetworks; subnets are declared, not automatic"
  }
}

run "each_vpc_has_a_deny_all_ingress_firewall" {
  command = plan

  assert {
    condition     = google_compute_firewall.data_vpc_deny_ingress.name != ""
    error_message = "the Data VPC must have a deny-all ingress firewall at priority 65534"
  }

  assert {
    condition     = google_compute_firewall.data_vpc_deny_ingress.direction == "INGRESS" && google_compute_firewall.data_vpc_deny_ingress.priority == 65534
    error_message = "the Data VPC's deny firewall must be INGRESS at priority 65534"
  }

  assert {
    condition     = google_compute_firewall.engineering_vpc_deny_ingress.name != ""
    error_message = "the Engineering VPC must have a deny-all ingress firewall at priority 65534"
  }

  assert {
    condition     = google_compute_firewall.engineering_vpc_deny_ingress.direction == "INGRESS" && google_compute_firewall.engineering_vpc_deny_ingress.priority == 65534
    error_message = "the Engineering VPC's deny firewall must be INGRESS at priority 65534"
  }
}

run "each_vpc_has_a_private_googleapis_zone" {
  command = plan

  assert {
    condition     = google_dns_managed_zone.data_googleapis.name != ""
    error_message = "the Data VPC must have a private googleapis.com DNS zone"
  }

  assert {
    condition     = google_dns_managed_zone.data_googleapis.visibility == "private"
    error_message = "the Data VPC's googleapis zone must be private, not public"
  }

  assert {
    condition     = google_dns_managed_zone.engineering_googleapis.name != ""
    error_message = "the Engineering VPC must have a private googleapis.com DNS zone"
  }

  assert {
    condition     = google_dns_managed_zone.engineering_googleapis.visibility == "private"
    error_message = "the Engineering VPC's googleapis zone must be private, not public"
  }
}

run "each_vpc_has_restricted_vip_records" {
  command = plan

  assert {
    condition     = google_dns_record_set.data_restricted_vip.name != ""
    error_message = "the Data VPC must have an A record for restricted.googleapis.com"
  }

  assert {
    condition     = google_dns_record_set.data_googleapis_wildcard.name != ""
    error_message = "the Data VPC must have a CNAME record for *.googleapis.com"
  }

  assert {
    condition     = google_dns_record_set.engineering_restricted_vip.name != ""
    error_message = "the Engineering VPC must have an A record for restricted.googleapis.com"
  }

  assert {
    condition     = google_dns_record_set.engineering_googleapis_wildcard.name != ""
    error_message = "the Engineering VPC must have a CNAME record for *.googleapis.com"
  }
}

run "the_data_and_engineering_vpcs_are_separate_from_the_shared_vpc" {
  command = plan

  assert {
    condition     = !contains([for v in google_compute_network.vpc : v.name], google_compute_network.data_vpc.name) && !contains([for v in google_compute_network.vpc : v.name], google_compute_network.engineering_vpc.name)
    error_message = "the Data and Engineering VPCs must be distinct resources, not aliases or subsets of the shared VPC"
  }

  assert {
    condition     = google_compute_network.data_vpc.name != google_compute_network.engineering_vpc.name
    error_message = "the Data and Engineering VPCs must be separate from each other"
  }
}

# That neither separate VPC hosts the console egress subnet is not asserted
# here. It compares network ids, which are unknown until a plan is applied,
# and this file used to settle that with a `command = apply` run against the
# mock — which `qip-acceptance`'s `terraform_plan` suite refuses, because
# every run in this repository's harnesses is a plan. The subnet's one
# `network` line is asserted on the configuration instead, and naming the
# Service network excludes both of these:
# `terraform_plan::the_console_egress_subnet_and_its_rules_attach_to_the_service_network`.

# The refusing half, which this file did not have. The module's only gates
# are on the console's range, and a value that is not a range is refused at
# plan rather than placed on any of the networks.
run "a_console_egress_value_that_is_not_a_range_is_refused" {
  command = plan

  variables {
    console_egress_cidr = "the data vpc"
  }

  expect_failures = [var.console_egress_cidr]
}
