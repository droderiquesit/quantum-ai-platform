# W4.3: ADR 0035's one dev node, declared in committed configuration and
# provisioned only when the owner flips `execution_nodes_enabled`.
#
# Mocked providers and `command = plan` throughout: no credential, no
# project, nothing created. The three module overrides are the ones every
# root harness here carries (see fabric.tftest.hcl), for `for_each` inputs a
# mock cannot know; none is on the node's path.
#
# The `dev_node` fixture is the entry `environments/dev/terraform.tfvars`
# commits, copied field for field — `terraform test` cannot read a tfvars
# file — with the two values nobody has chosen left null exactly as they are
# committed. Each refusing run says which mutation of `execution-nodes-
# enabled.tf` or `variables.tf` would make it pass when it should not.

mock_provider "google" {}
mock_provider "google-beta" {}

override_module {
  target = module.ai
  outputs = {
    training_bucket         = "harness-training"
    metadata_store_id       = "projects/node-plan-harness/locations/us-east4/metadataStores/harness"
    serving_endpoint_id     = null
    reachable_by_this_build = false
  }
}

override_module {
  target = module.evidence
  outputs = {
    bucket_name       = "harness-evidence"
    bucket_url        = "gs://harness-evidence"
    encryption_key_id = "projects/node-plan-harness/locations/us-east4/keyRings/harness/cryptoKeys/evidence"
  }
}

override_module {
  target = module.registry
  outputs = {
    repository_id   = "projects/node-plan-harness/locations/us-east4/repositories/harness"
    repository_name = "harness"
    image_prefix    = "us-east4-docker.pkg.dev/node-plan-harness/harness"
  }
}

variables {
  project_id        = "node-plan-harness"
  project_number    = 123456789012
  environment       = "dev"
  region            = "us-east4"
  github_repository = "example/example"

  # The zones dev declares, so the catalogue's placement preconditions pass
  # and a run fails only on what it is about. None overlaps the node's /20.
  trust_zones = {
    "application-identity" = { region = "us-east4", subnet_cidr = "10.0.32.0/24" }
    "cognition"            = { region = "us-east4", subnet_cidr = "10.0.33.0/24" }
    "intelligence"         = { region = "us-east4", subnet_cidr = "10.0.34.0/24" }
    "management"           = { region = "us-east4", subnet_cidr = "10.0.35.0/24" }
  }

  # As committed in dev: declared, not enabled, image and allocation null.
  execution_nodes = {
    "newyork-1" = {
      region       = "us-east4"
      zone         = "us-east4-a"
      subnet_cidr  = "10.67.0.0/20"
      machine_type = "c3-highcpu-8"
      boot_image   = null
      venues = {
        "simulated-1" = { cidr = "192.0.2.0/24", port = 443 }
      }
      create_egress_nat        = false
      region_allocation        = null
      default_pricing          = ""
      strategy_plan_path       = ""
      cross_region_mirror_path = ""
    }
  }
}

# The committed state plans, and plans nothing for the node. Fails if the
# root's `for_each` goes back to `var.execution_nodes` unconditionally — the
# declared entry would then reach the module with a null image.
run "a_declared_node_is_not_provisioned_while_the_flag_is_off" {
  command = plan

  assert {
    condition     = var.execution_nodes_enabled == false
    error_message = "The harness's premise is the committed default: the flag off."
  }

  assert {
    condition     = length(var.execution_nodes) == 1
    error_message = "The harness's premise is one declared node; with none, 'nothing planned' proves nothing."
  }

  assert {
    condition     = length(module.execution_node) == 0
    error_message = "A node was planned with execution_nodes_enabled false. Declaring a node must not provision it."
  }
}

# Refused: the flag on while the image is still null. Fails (planned
# instead of refused) if the boot_image validation in
# execution-nodes-enabled.tf is deleted.
run "enabling_a_node_with_no_baked_image_is_refused" {
  command = plan

  variables {
    execution_nodes_enabled = true
    execution_nodes = {
      "newyork-1" = {
        region                   = "us-east4"
        zone                     = "us-east4-a"
        subnet_cidr              = "10.67.0.0/20"
        machine_type             = "c3-highcpu-8"
        boot_image               = null
        venues                   = { "simulated-1" = { cidr = "192.0.2.0/24", port = 443 } }
        region_allocation        = "1000000"
        default_pricing          = ""
        strategy_plan_path       = ""
        cross_region_mirror_path = ""
      }
    }
  }

  expect_failures = [var.execution_nodes_enabled]
}

# Refused: the flag on while nobody has chosen the allocation. Fails if the
# region_allocation validation is deleted, or if it is changed to admit null.
run "enabling_a_node_with_no_chosen_allocation_is_refused" {
  command = plan

  variables {
    execution_nodes_enabled = true
    execution_nodes = {
      "newyork-1" = {
        region                   = "us-east4"
        zone                     = "us-east4-a"
        subnet_cidr              = "10.67.0.0/20"
        machine_type             = "c3-highcpu-8"
        boot_image               = "projects/node-plan-harness/global/images/qip-execution-node-c3-highcpu-8-0123456789ab-r7"
        venues                   = { "simulated-1" = { cidr = "192.0.2.0/24", port = 443 } }
        region_allocation        = null
        default_pricing          = ""
        strategy_plan_path       = ""
        cross_region_mirror_path = ""
      }
    }
  }

  expect_failures = [var.execution_nodes_enabled]
}

# Refused: a zero allocation is not a choice, it is a node that can reserve
# nothing. Fails if the zero arm of the validation is deleted.
run "enabling_a_node_with_a_zero_allocation_is_refused" {
  command = plan

  variables {
    execution_nodes_enabled = true
    execution_nodes = {
      "newyork-1" = {
        region                   = "us-east4"
        zone                     = "us-east4-a"
        subnet_cidr              = "10.67.0.0/20"
        machine_type             = "c3-highcpu-8"
        boot_image               = "projects/node-plan-harness/global/images/qip-execution-node-c3-highcpu-8-0123456789ab-r7"
        venues                   = { "simulated-1" = { cidr = "192.0.2.0/24", port = 443 } }
        region_allocation        = "0.00"
        default_pricing          = ""
        strategy_plan_path       = ""
        cross_region_mirror_path = ""
      }
    }
  }

  expect_failures = [var.execution_nodes_enabled]
}

# Refused: the flag on with nothing declared. Fails if that validation is
# deleted.
run "enabling_with_no_declared_node_is_refused" {
  command = plan

  variables {
    execution_nodes_enabled = true
    execution_nodes         = {}
  }

  expect_failures = [var.execution_nodes_enabled]
}

# Refused, whether or not the flag is on: an image baked for c3-highcpu-22
# named on a c3-highcpu-8 node. Fails if the shape validation on
# `execution_nodes` in variables.tf is deleted or stops interpolating the
# node's own machine_type.
run "an_image_baked_for_another_machine_shape_is_refused" {
  command = plan

  variables {
    execution_nodes = {
      "newyork-1" = {
        region                   = "us-east4"
        zone                     = "us-east4-a"
        subnet_cidr              = "10.67.0.0/20"
        machine_type             = "c3-highcpu-8"
        boot_image               = "projects/node-plan-harness/global/images/qip-execution-node-c3-highcpu-22-0123456789ab-r7"
        venues                   = { "simulated-1" = { cidr = "192.0.2.0/24", port = 443 } }
        region_allocation        = "1000000"
        default_pricing          = ""
        strategy_plan_path       = ""
        cross_region_mirror_path = ""
      }
    }
  }

  expect_failures = [var.execution_nodes]
}

# The admitting half: the owner's flip, with an image baked for this shape
# and an allocation a person chose (the harness's number is a fixture, not a
# proposal). Plans exactly one node, in shadow mode, with the simulated feed.
# Fails if any validation above refuses everything.
run "a_complete_node_is_provisioned_once_the_owner_enables_it" {
  command = plan

  variables {
    execution_nodes_enabled = true
    execution_nodes = {
      "newyork-1" = {
        region                   = "us-east4"
        zone                     = "us-east4-a"
        subnet_cidr              = "10.67.0.0/20"
        machine_type             = "c3-highcpu-8"
        boot_image               = "projects/node-plan-harness/global/images/qip-execution-node-c3-highcpu-8-0123456789ab-r7"
        venues                   = { "simulated-1" = { cidr = "192.0.2.0/24", port = 443 } }
        region_allocation        = "1000000"
        default_pricing          = ""
        strategy_plan_path       = ""
        cross_region_mirror_path = ""
      }
    }
  }

  assert {
    condition     = length(module.execution_node) == 1
    error_message = "A complete, enabled node was not planned: the gate refuses everything."
  }

  assert {
    condition     = module.execution_node["newyork-1"].shadow_mode == true
    error_message = "The enabled node is not in shadow mode."
  }
}
