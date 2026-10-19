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
  owner             = "platform"
  github_repository = "example/example"

  trust_zones = {
    "application-identity" = { region = "us-east4", subnet_cidr = "10.0.32.0/24" }
    "cognition"            = { region = "us-east4", subnet_cidr = "10.0.33.0/24" }
    "intelligence"         = { region = "us-east4", subnet_cidr = "10.0.34.0/24" }
    "management"           = { region = "us-east4", subnet_cidr = "10.0.35.0/24" }
    "control-fabric"       = { region = "us-east4", subnet_cidr = "10.0.36.0/24" }
    "ledger"               = { region = "us-east4", subnet_cidr = "10.0.38.0/24" }
  }

  # Both machines own their sockets and speak mutual TLS on CA Service
  # certificates (ADR 0112), with the pinned Google roots baked beside the
  # binary; set for every run so each refusal below fails on its own check,
  # and nulled by the run proving that one.
  certificate_authority = {
    location     = "us-east4"
    organization = "Algorik"
  }
  google_tls_roots_file = "infrastructure/terraform/tests/fixtures/harness-roots.pem"
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
# The brokers' zones are the root's default region's (europe-west2): the module
# refuses a broker zone outside var.region (FABRIC-008), and this harness
# leaves the region at its default because its trust bundle is keyed to it.
run "a_named_broker_and_ledger_are_provisioned_in_their_zones" {
  command = plan

  variables {
    event_fabric_broker = {
      zone             = "europe-west2-a"
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

  # Item 8: the broker admits the ledger by the one SAN the workload
  # template lets the ledger obtain, and nothing else — not itself, not any
  # certificate the pool issued.
  assert {
    condition     = local.fabric_admitted_peers == tolist(["qip-dev-ledgerd.fabric-plan-harness.iam.gserviceaccount.com"])
    error_message = "The broker admitted peers are not exactly the ledger SAN: ${jsonencode(local.fabric_admitted_peers)}"
  }

  # Each machine may request one certificate: its own, through the template.
  assert {
    condition = (
      toset(keys(google_privateca_ca_pool_iam_member.requesters)) == toset(["fabricd", "ledgerd"])
      && google_privateca_ca_pool_iam_member.requesters["fabricd"].member == "serviceAccount:qip-dev-fabricd@fabric-plan-harness.iam.gserviceaccount.com"
      && google_privateca_ca_pool_iam_member.requesters["ledgerd"].member == "serviceAccount:qip-dev-ledgerd@fabric-plan-harness.iam.gserviceaccount.com"
      && alltrue([
        for key, grant in google_privateca_ca_pool_iam_member.requesters :
        grant.role == "roles/privateca.certificateRequester"
        && strcontains(grant.condition[0].expression, "api.getAttribute('privateca.googleapis.com/template', '') == 'fabric-plan-harness/-/qip-dev-workload'")
        && strcontains(grant.condition[0].expression, "hasOnly(['dns:${replace(trimprefix(grant.member, "serviceAccount:"), "@", ".")}'])")
      ])
    )
    error_message = "The broker and the ledger are not each a requester conditioned on the workload template and their own SAN."
  }
}

# Refused: either machine where the environment has no CA. Fails if the
# preconditions in fabric.tf are deleted.
run "a_broker_and_ledger_with_no_certificate_authority_are_refused" {
  command = plan

  variables {
    certificate_authority = null
    event_fabric_broker = {
      zone             = "europe-west2-a"
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

  expect_failures = [terraform_data.event_fabric_broker_is_placed, terraform_data.ledgerd_is_placed]
}

# Refused: a broker with no client to name. qip-fabricd refuses to start
# admitting nobody, and admitting "any certificate the pool issued" is not a
# list. Fails if the admitted-peers precondition is deleted.
run "a_broker_with_no_client_to_admit_is_refused" {
  command = plan

  variables {
    event_fabric_broker = {
      zone             = "europe-west2-a"
      boot_image       = "projects/fabric-plan-harness/global/images/qip-fabricd-20261011"
      internal_address = "10.0.36.10"
      dns_domain       = "fabric.qip-dev.internal"
    }
  }

  expect_failures = [terraform_data.event_fabric_broker_is_placed]
}

# FABRIC-8096 at the root: ADR 0111's three instances in a region, behind the
# Spanner lease, plan to the end.
run "three_brokers_behind_the_spanner_lease_are_provisioned" {
  command = plan

  variables {
    event_fabric_broker = {
      zone                 = "europe-west2-a"
      boot_image           = "projects/fabric-plan-harness/global/images/qip-fabricd-20261011"
      internal_address     = "10.0.36.10"
      additional_addresses = ["10.0.36.11", "10.0.36.12"]
      additional_zones     = ["europe-west2-b", "europe-west2-c"]
      dns_domain           = "fabric.qip-dev.internal"
      managed_log = {
        spanner_database = "projects/fabric-plan-harness/instances/qip-dev/databases/fabric"
      }
    }
    ledgerd = {
      zone             = "us-east4-a"
      boot_image       = "projects/fabric-plan-harness/global/images/qip-ledgerd-20261011"
      internal_address = "10.0.38.10"
      account_id       = "paper-book"
    }
  }

  assert {
    condition     = length(module.event_fabric_broker) == 1
    error_message = "Three brokers behind the managed log were not planned."
  }

  # FABRIC-008: the root's zones reach the machines, one broker per zone.
  assert {
    condition     = length(distinct(values(module.event_fabric_broker[0].broker_zones))) == 3
    error_message = "The three brokers do not sit in three zones."
  }
}

# Three addresses and one additional zone: a broker with no zone of its own
# is refused rather than stacked into `zone`.
run "additional_brokers_without_a_zone_each_are_refused" {
  command = plan

  variables {
    event_fabric_broker = {
      zone                 = "europe-west2-a"
      boot_image           = "projects/fabric-plan-harness/global/images/qip-fabricd-20261011"
      internal_address     = "10.0.36.10"
      additional_addresses = ["10.0.36.11", "10.0.36.12"]
      additional_zones     = ["europe-west2-b"]
      dns_domain           = "fabric.qip-dev.internal"
      managed_log = {
        spanner_database = "projects/fabric-plan-harness/instances/qip-dev/databases/fabric"
      }
    }
    ledgerd = {
      zone             = "us-east4-a"
      boot_image       = "projects/fabric-plan-harness/global/images/qip-ledgerd-20261011"
      internal_address = "10.0.38.10"
      account_id       = "paper-book"
    }
  }

  expect_failures = [terraform_data.event_fabric_broker_is_placed]
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
      "ledger"               = { region = "us-east4", subnet_cidr = "10.0.38.0/24" }
    }
    event_fabric_broker = {
      zone             = "europe-west2-a"
      boot_image       = "projects/fabric-plan-harness/global/images/qip-fabricd-20261011"
      internal_address = "10.0.36.10"
      dns_domain       = "fabric.qip-dev.internal"
    }
    # A client to admit, in its own declared zone, so the zone is the only
    # thing wrong with the broker.
    ledgerd = {
      zone             = "us-east4-a"
      boot_image       = "projects/fabric-plan-harness/global/images/qip-ledgerd-20261011"
      internal_address = "10.0.38.10"
      account_id       = "paper-book"
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
      zone             = "europe-west2-a"
      boot_image       = "projects/fabric-plan-harness/global/images/qip-fabricd-20261011"
      internal_address = "10.0.36.10"
      dns_domain       = "fabric.algorik.ai"
    }
  }

  expect_failures = [var.event_fabric_broker]
}

# ADR 0112 §2 across the three binaries that own their sockets: with the
# broker, the ledger and a node all declared, the requesters are exactly
# their three identities, each bound to its own SAN, and the broker admits
# exactly the other two.
run "the_three_socket_owning_binaries_and_only_they_may_request_certificates" {
  command = plan

  variables {
    event_fabric_broker = {
      zone             = "europe-west2-a"
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
    artefact_signing = {
      bundle_not_after = "2099-01-01T00:00:00Z"
    }
    artefact_trust_bundle_file = "infrastructure/terraform/tests/fixtures/fabric-harness-bundle.json"
    execution_nodes_enabled    = true
    execution_nodes = {
      "newyork-1" = {
        region                   = "us-east4"
        zone                     = "us-east4-a"
        subnet_cidr              = "10.67.0.0/20"
        machine_type             = "c4-highcpu-8"
        boot_image               = "projects/fabric-plan-harness/global/images/qip-execution-node-c4-highcpu-8-0123456789ab-r7"
        venues                   = { "simulated-1" = { cidr = "192.0.2.0/24", port = 443 } }
        region_allocation        = "1000000"
        default_pricing          = ""
        strategy_plan_path       = ""
        cross_region_mirror_path = ""
      }
    }
  }

  assert {
    condition = {
      for key, grant in google_privateca_ca_pool_iam_member.requesters : key => grant.member
      } == {
      "edge-node-newyork-1" = "serviceAccount:qip-exec-newyork-1-dev@fabric-plan-harness.iam.gserviceaccount.com"
      "fabricd"             = "serviceAccount:qip-dev-fabricd@fabric-plan-harness.iam.gserviceaccount.com"
      "ledgerd"             = "serviceAccount:qip-dev-ledgerd@fabric-plan-harness.iam.gserviceaccount.com"
    }
    error_message = "The certificate requesters are not exactly qip-fabricd's, qip-ledgerd's and the node's identities."
  }

  assert {
    condition = alltrue([
      for key, grant in google_privateca_ca_pool_iam_member.requesters :
      strcontains(grant.condition[0].expression, "hasOnly(['dns:${replace(trimprefix(grant.member, "serviceAccount:"), "@", ".")}'])")
    ])
    error_message = "A requester may obtain a certificate for a SAN other than its own."
  }

  assert {
    condition = local.fabric_admitted_peers == tolist([
      "qip-dev-ledgerd.fabric-plan-harness.iam.gserviceaccount.com",
      "qip-exec-newyork-1-dev.fabric-plan-harness.iam.gserviceaccount.com",
    ])
    error_message = "The broker does not admit exactly the ledger and the node, by the SANs their certificates carry."
  }
}
