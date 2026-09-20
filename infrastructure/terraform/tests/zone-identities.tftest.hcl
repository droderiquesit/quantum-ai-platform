# The identities each trust zone holds, planned from the root.
#
# `modules/trust-zones` keys every ledger and control-fabric grant on the
# identities the root places in a zone, and the root's `trust_zones.identities`
# output is what a reader consults to learn who is inside a boundary. Until
# 2026-09-19 the management entry listed OpenObserve and nothing else, while
# the control plane's three controllers — Config Connector, Argo CD and Kargo —
# ran on the management subnet under the management tag and were named in no
# zone at all. A `permitted_paths` entry from `management` would have granted
# the dashboard's identity and not the deployer's, and the output would have
# read as complete. This file plans the root and reads the output, which is
# the only way to see what the catalogue actually places rather than what a
# comment beside it says.
#
# Both halves. The admitting runs assert the *count* of management
# identities rather than their emails, because a mocked plan does not know an
# account's email until it exists — but `concat` over lists of known length
# has a known length, which is exactly why `catalogue.tf` builds the entry
# with `concat` and not `sort`. The refusing run is the root's own guard on
# the same map: a digest that would place OpenObserve in a zone the
# environment never declared.
#
# Mocked providers, plan only, three modules overridden with fixed outputs —
# the same shape as the sibling harnesses and for the same reasons: nothing
# here can reach a project, and the three overrides replace `for_each` inputs
# a mock cannot compute. None of the three is on the identity map's path.

mock_provider "google" {}
mock_provider "google-beta" {}

override_module {
  target = module.ai
  outputs = {
    training_bucket         = "harness-training"
    metadata_store_id       = "projects/zone-identities-harness/locations/us-east4/metadataStores/harness"
    serving_endpoint_id     = null
    reachable_by_this_build = false
  }
}
override_module {
  target = module.evidence
  outputs = {
    bucket_name       = "harness-evidence"
    bucket_url        = "gs://harness-evidence"
    encryption_key_id = "projects/zone-identities-harness/locations/us-east4/keyRings/harness/cryptoKeys/evidence"
  }
}
override_module {
  target = module.registry
  outputs = {
    repository_id   = "projects/zone-identities-harness/locations/us-east4/repositories/harness"
    repository_name = "harness"
    image_prefix    = "us-east4-docker.pkg.dev/zone-identities-harness/harness"
  }
}

variables {
  project_id        = "zone-identities-harness"
  project_number    = 123456789012
  environment       = "dev"
  github_repository = "example/example"

  # The zones below are in us-east4, and the management zone's GitHub egress
  # needs a NAT in the zone's own region; the root's default region is not
  # this one, and the module refuses a zone NAT'd from elsewhere.
  region = "us-east4"

  trust_zones = {
    "application-identity" = { region = "us-east4", subnet_cidr = "10.0.32.0/24" }
    "cognition"            = { region = "us-east4", subnet_cidr = "10.0.33.0/24" }
    "intelligence"         = { region = "us-east4", subnet_cidr = "10.0.34.0/24" }
    "management"           = { region = "us-east4", subnet_cidr = "10.0.35.0/24" }
  }
}

# --- the admitting half ------------------------------------------------------

run "a_control_plane_places_its_three_controllers_in_the_management_zone" {
  command = plan

  variables {
    gitops_enabled                = true
    gitops_master_ipv4_cidr_block = "10.0.36.0/28"
  }

  # The control plane's outputs, fixed. A mock cannot know an account's email
  # before it exists, and `modules/cloudrun` counts its deployer binding on
  # whether that email is null — an unknown there is not a plan with an
  # unknown in it, it is a plan Terraform refuses to make. The three emails
  # are what this run counts, so fixing them is what makes the count a fact
  # of the catalogue rather than of the mock.
  override_module {
    target = module.gitops_control_plane
    outputs = {
      cluster_name                 = "qip-dev-control-plane"
      cluster_location             = "us-east4"
      kcc_service_account_email    = "qip-dev-kcc@zone-identities-harness.iam.gserviceaccount.com"
      argocd_service_account_email = "qip-dev-argocd@zone-identities-harness.iam.gserviceaccount.com"
      kargo_service_account_email  = "qip-dev-kargo@zone-identities-harness.iam.gserviceaccount.com"
      etcd_key_id                  = "projects/zone-identities-harness/locations/us-east4/keyRings/harness/cryptoKeys/etcd"
    }
  }

  # Three, not four: the OpenObserve digest is null here, so the dashboard's
  # identity does not exist and the zone holds the deployer, the reconciler
  # and the promoter. A count of one would be the state this file was
  # written to end; a count of zero would mean the control plane's identities
  # are in no zone and the module can never grant them anything.
  assert {
    condition     = length(output.trust_zones.identities["management"]) == 3
    error_message = "the management zone does not hold exactly the control plane's three controller identities; a path granted from management would reach the wrong accounts"
  }

  # The catalogue's own three zones are still placed. Their lists are sorted
  # and so unknown at plan, which is why only the keys are asserted here.
  assert {
    condition     = alltrue([for zone in ["application-identity", "cognition", "intelligence"] : contains(keys(output.trust_zones.identities), zone)])
    error_message = "a catalogue zone lost its identity entry; the ledger and fabric grants for that zone can no longer be made"
  }
}

run "an_environment_with_no_control_plane_and_no_dashboard_places_nobody_in_management" {
  command = plan

  # The shape of test, stage and prod: the key is present so that the
  # module sees the zone the moment something is placed there, and the list
  # is empty so that the module's own validation admits it whether or not
  # the environment declared the zone.
  assert {
    condition     = length(output.trust_zones.identities["management"]) == 0
    error_message = "an environment with neither a control plane nor OpenObserve lists an identity in management; something is being placed there that nothing created"
  }
}

# --- the refusing half -------------------------------------------------------

run "a_dashboard_digest_with_no_management_zone_stops_the_plan" {
  command = plan

  variables {
    trust_zones = {
      "application-identity" = { region = "us-east4", subnet_cidr = "10.0.32.0/24" }
      "cognition"            = { region = "us-east4", subnet_cidr = "10.0.33.0/24" }
      "intelligence"         = { region = "us-east4", subnet_cidr = "10.0.34.0/24" }
    }
    # A well-formed digest of nothing in particular: the variable's own
    # validation is on the shape, and this run is about the zone.
    vendored_openobserve_image_digest = "sha256:0000000000000000000000000000000000000000000000000000000000000000"
  }

  # Two refusals of one mistake, and both are expected on purpose. The
  # source-specific guard names the decision that is missing — declare the
  # zone beside the digest — and the source-agnostic one refuses the identity
  # that would otherwise have been narrowed out of the map the module is
  # handed. Listing only the first would report the second as an unexpected
  # error; listing only the second would let a root that stopped placing the
  # dashboard pass while the digest's own precondition went unevaluated.
  #
  # Neither is the module's validation on `zone_identities`, and that is not
  # an omission: a run can expect a failure only from a root object, which is
  # the reason the root narrows the map before handing it over and carries a
  # refusal of its own. The module's is planned from both sides in
  # modules/trust-zones/tests/zone-model.tftest.hcl.
  expect_failures = [
    terraform_data.openobserve_is_placed[0],
    terraform_data.identities_are_placed,
  ]
}
