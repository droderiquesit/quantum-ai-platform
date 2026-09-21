# The domain's zone as the root wires it, planned.
#
# `modules/dns-zone/tests/dns-zone.tftest.hcl` proves the module: that the
# domain validation fires and admits, that a record outside the zone is
# refused, that the TTL window has both ends. This file proves the thing that
# module cannot see — **that the records are derived from the front doors that
# actually exist**, and that an environment naming no domain creates no zone at
# all.
#
# The failure it prevents is the one this whole module was built for, in its
# subtler form. A record whose address is a literal and a record whose address
# is a module output plan identically when the literal is right. They diverge
# the first time an address is released and re-reserved, in silence, and by
# then the plan that would have shown it has long since been applied. So the
# assertion here is on the *shape* — a door that exists has a record, a door
# that does not have none — and the infrastructure acceptance suite refuses the
# literal in the wiring by reading it.
#
# Mocked providers, plan only, the same three `override_module` blocks the
# sibling harnesses carry and for the same reason: they replace `for_each`
# inputs a mock cannot compute, and none of the three is on this variable's
# path.

mock_provider "google" {}
mock_provider "google-beta" {}

variables {
  project_id        = "dns-zone-root-plan-harness"
  project_number    = 123456789012
  environment       = "dev"
  github_repository = "example/example"

  trust_zones = {
    "application-identity" = { region = "us-east4", subnet_cidr = "10.0.32.0/24" }
    "cognition"            = { region = "us-east4", subnet_cidr = "10.0.33.0/24" }
    "intelligence"         = { region = "us-east4", subnet_cidr = "10.0.34.0/24" }
    "management"           = { region = "us-east4", subnet_cidr = "10.0.35.0/24" }
  }
}

# --- the closed state --------------------------------------------------------

run "an_environment_naming_no_domain_creates_no_zone_at_all" {
  command = plan

  override_module {
    target = module.ai
    outputs = {
      training_bucket         = "harness-training"
      metadata_store_id       = "projects/dns-zone-root-plan-harness/locations/us-east4/metadataStores/harness"
      serving_endpoint_id     = null
      reachable_by_this_build = false
    }
  }
  override_module {
    target = module.evidence
    outputs = {
      bucket_name       = "harness-evidence"
      bucket_url        = "gs://harness-evidence"
      encryption_key_id = "projects/dns-zone-root-plan-harness/locations/us-east4/keyRings/harness/cryptoKeys/evidence"
    }
  }
  override_module {
    target = module.registry
    outputs = {
      repository_id   = "projects/dns-zone-root-plan-harness/locations/us-east4/repositories/harness"
      repository_name = "harness"
      image_prefix    = "us-east4-docker.pkg.dev/dns-zone-root-plan-harness/harness"
    }
  }

  # The state of test, stage and prod. A domain has one authoritative zone, no
  # plan can see another environment's state, and so the only structural guard
  # available is that an environment which does not name the domain creates
  # nothing — not an empty zone, not a placeholder. Asserted on the output
  # because the output is null exactly when the module's `count` is zero.
  assert {
    condition     = output.dns_zone == null
    error_message = "a DNS zone was planned for an environment that named no domain; a second authoritative zone for one domain applies cleanly and then serves records nobody resolves"
  }
}

# --- the admitting half ------------------------------------------------------

run "a_domain_and_a_front_door_produce_the_zone_and_that_doors_record" {
  command = plan

  override_module {
    target = module.ai
    outputs = {
      training_bucket         = "harness-training"
      metadata_store_id       = "projects/dns-zone-root-plan-harness/locations/us-east4/metadataStores/harness"
      serving_endpoint_id     = null
      reachable_by_this_build = false
    }
  }
  override_module {
    target = module.evidence
    outputs = {
      bucket_name       = "harness-evidence"
      bucket_url        = "gs://harness-evidence"
      encryption_key_id = "projects/dns-zone-root-plan-harness/locations/us-east4/keyRings/harness/cryptoKeys/evidence"
    }
  }
  override_module {
    target = module.registry
    outputs = {
      repository_id   = "projects/dns-zone-root-plan-harness/locations/us-east4/repositories/harness"
      repository_name = "harness"
      image_prefix    = "us-east4-docker.pkg.dev/dns-zone-root-plan-harness/harness"
    }
  }

  variables {
    dns_zone_domain = "algorik.ai"
    # One door on: the portal's. The GitOps Gateway stays off, which is what
    # makes the record count below mean something — see the next assertion.
    gitops_portal_hostname = "portal.algorik.ai"
    console_egress_cidr    = "10.0.16.0/26"
  }

  assert {
    condition     = output.dns_zone.domain == "algorik.ai"
    error_message = "the reviewed domain did not reach the zone; the value in the tfvars would be a name nothing is authoritative for"
  }

  # **The shape that matters.** Exactly one record, for the one door that
  # exists. A wiring that hard-coded the three names would plan three here and
  # two of them would resolve to addresses no forwarding rule answers on —
  # which is a connection that hangs, and reads to whoever typed the name as a
  # network problem rather than as a door nobody opened.
  assert {
    condition     = keys(output.dns_zone.records) == ["portal.algorik.ai"]
    error_message = "the zone's records are not exactly the one front door switched on in this run; a record for a door that does not exist resolves to an address nothing answers on, and a hanging connection reads as a network fault rather than as a door nobody opened"
  }

  # The nameservers are the one manual step and the output has to carry them.
  # Their *values* are unknown at plan — a mocked provider leaves every
  # computed attribute unknown, which `portal-front-door.tftest.hcl` records
  # about its address for the same reason. What is assertable, and what
  # actually breaks when somebody trims the output, is that the key is
  # published at all: an owner who cannot read it from `terraform output` reads
  # it out of the console, and then the delegation and the zone are two facts
  # nobody reconciles.
  assert {
    condition     = contains(keys(output.dns_zone), "nameservers")
    error_message = "the dns_zone output publishes no nameservers; the one remaining manual step would have nothing to point the registrar at"
  }

  # Signing on by default, and the DS published for the day the owner decides
  # to finish it. `ds_record` present-but-null at plan is the honest state;
  # its absence would mean the owner has to go and find it in the console.
  assert {
    condition     = output.dns_zone.dnssec_enabled == true && contains(keys(output.dns_zone), "ds_record")
    error_message = "the zone is unsigned, or the DS record an owner would need to publish is not offered; signing is free until a DS exists and awkward to enable later against a live delegation"
  }
}

# --- the refusing half -------------------------------------------------------
#
# One run per shape. Each of these would reach Cloud DNS as a zone name: the
# trailing dot produces `algorik.ai..`, which is *accepted* and then delegated
# to by nothing — an apply reporting success on a zone that cannot ever work.

run "a_domain_carrying_a_trailing_dot_stops_the_plan" {
  command = plan

  variables {
    dns_zone_domain = "algorik.ai."
  }

  expect_failures = [var.dns_zone_domain]
}

run "a_domain_carrying_a_scheme_stops_the_plan" {
  command = plan

  variables {
    dns_zone_domain = "https://algorik.ai"
  }

  expect_failures = [var.dns_zone_domain]
}

run "an_upper_case_domain_stops_the_plan" {
  command = plan

  variables {
    dns_zone_domain = "Algorik.ai"
  }

  expect_failures = [var.dns_zone_domain]
}

run "a_wildcard_domain_stops_the_plan" {
  command = plan

  variables {
    dns_zone_domain = "*.algorik.ai"
  }

  expect_failures = [var.dns_zone_domain]
}

run "a_ttl_nobody_would_choose_stops_the_plan" {
  command = plan

  variables {
    dns_zone_domain        = "algorik.ai"
    dns_record_ttl_seconds = 5
  }

  expect_failures = [var.dns_record_ttl_seconds]
}
