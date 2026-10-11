# W4.2: the event-fabric broker and qip-ledgerd, wired from the root.
#
# Both are off unless an environment names them, and each refuses to be
# placed outside its trust zone. Mocked providers and `command = plan`
# throughout: no credential, no project, nothing created. The three module
# overrides are the ones every root harness here carries, for the same
# reason — `for_each` inputs a mock cannot know — and none of them is on the
# fabric's path.

mock_provider "google" {}
mock_provider "google-beta" {}

override_module {
  target = module.ai
  outputs = {
    training_bucket         = "harness-training"
    metadata_store_id       = "projects/fabric-plan-harness/locations/us-east4/metadataStores/harness"
    serving_endpoint_id     = null
    reachable_by_this_build = false
  }
}

override_module {
  target = module.evidence
  outputs = {
    bucket_name       = "harness-evidence"
    bucket_url        = "gs://harness-evidence"
    encryption_key_id = "projects/fabric-plan-harness/locations/us-east4/keyRings/harness/cryptoKeys/evidence"
  }
}

override_module {
  target = module.registry
  outputs = {
    repository_id   = "projects/fabric-plan-harness/locations/us-east4/repositories/harness"
    repository_name = "harness"
    image_prefix    = "us-east4-docker.pkg.dev/fabric-plan-harness/harness"
  }
}

variables {
  project_id        = "fabric-plan-harness"
  project_number    = 123456789012
  environment       = "dev"
  github_repository = "example/example"

  trust_zones = {
    "application-identity" = { region = "us-east4", subnet_cidr = "10.0.32.0/24" }
    "cognition"            = { region = "us-east4", subnet_cidr = "10.0.33.0/24" }
    "intelligence"         = { region = "us-east4", subnet_cidr = "10.0.34.0/24" }
    "management"           = { region = "us-east4", subnet_cidr = "10.0.35.0/24" }
    "control-fabric"       = { region = "us-east4", subnet_cidr = "10.0.36.0/24" }
    "ledger"               = { region = "us-east4", subnet_cidr = "10.0.38.0/24" }
  }
}

# Off by default: no broker, no ledger, no identity, no zone.
run "neither_is_provisioned_unless_an_environment_names_it" {
  command = plan

  assert {
    condition     = length(module.event_fabric_broker) == 0 && length(module.ledgerd) == 0
    error_message = "A broker or a ledger was planned with neither named."
  }

  assert {
    condition     = length(google_service_account.fabricd) == 0 && length(google_dns_managed_zone.fabric) == 0
    error_message = "The broker's identity or private zone was planned with no broker."
  }
}

# The admitting half: named, both plan to the end.
run "a_named_broker_and_ledger_are_provisioned_in_their_zones" {
  command = plan

  variables {
    event_fabric_broker = {
      zone             = "us-east4-a"
      boot_image       = "projects/fabric-plan-harness/global/images/qip-fabricd-20261011"
      internal_address = "10.0.36.10"
      dns_domain       = "fabric.qip-dev.internal"
    }
    ledgerd = {
      zone             = "us-east4-a"
      boot_image       = "projects/fabric-plan-harness/global/images/qip-ledgerd-20261011"
      internal_address = "10.0.38.10"
      account_id       = "paper-book"
    }
  }

  assert {
    condition     = length(module.event_fabric_broker) == 1 && length(module.ledgerd) == 1
    error_message = "A named broker or ledger was not planned."
  }

  assert {
    condition     = google_dns_managed_zone.fabric[0].visibility == "private"
    error_message = "The broker's discovery zone is not private; an internal address would be published."
  }
}

# The refusing halves.
run "a_broker_outside_a_declared_control_fabric_zone_is_refused" {
  command = plan

  variables {
    trust_zones = {
      "application-identity" = { region = "us-east4", subnet_cidr = "10.0.32.0/24" }
      "cognition"            = { region = "us-east4", subnet_cidr = "10.0.33.0/24" }
      "intelligence"         = { region = "us-east4", subnet_cidr = "10.0.34.0/24" }
      "management"           = { region = "us-east4", subnet_cidr = "10.0.35.0/24" }
    }
    event_fabric_broker = {
      zone             = "us-east4-a"
      boot_image       = "projects/fabric-plan-harness/global/images/qip-fabricd-20261011"
      internal_address = "10.0.36.10"
      dns_domain       = "fabric.qip-dev.internal"
    }
  }

  expect_failures = [terraform_data.event_fabric_broker_is_placed]
}

run "a_ledger_outside_a_declared_ledger_zone_is_refused" {
  command = plan

  variables {
    trust_zones = {
      "application-identity" = { region = "us-east4", subnet_cidr = "10.0.32.0/24" }
      "cognition"            = { region = "us-east4", subnet_cidr = "10.0.33.0/24" }
      "intelligence"         = { region = "us-east4", subnet_cidr = "10.0.34.0/24" }
      "management"           = { region = "us-east4", subnet_cidr = "10.0.35.0/24" }
    }
    ledgerd = {
      zone             = "us-east4-a"
      boot_image       = "projects/fabric-plan-harness/global/images/qip-ledgerd-20261011"
      internal_address = "10.0.38.10"
      account_id       = "paper-book"
    }
  }

  expect_failures = [terraform_data.ledgerd_is_placed]
}

run "a_public_discovery_domain_is_refused" {
  command = plan

  variables {
    event_fabric_broker = {
      zone             = "us-east4-a"
      boot_image       = "projects/fabric-plan-harness/global/images/qip-fabricd-20261011"
      internal_address = "10.0.36.10"
      dns_domain       = "fabric.algorik.ai"
    }
  }

  expect_failures = [var.event_fabric_broker]
}
