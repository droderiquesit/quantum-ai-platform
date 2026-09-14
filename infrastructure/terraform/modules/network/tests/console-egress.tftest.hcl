# The console's egress range, planned.
#
# This file exists because the validation it exercises was broken in a way no
# amount of reading could show and no test in the Rust acceptance suite could
# see. `console_egress_cidr == null || tonumber(split("/", …)[1]) <= 26` looks
# like a guarded check and is not one: Terraform evaluates both operands of
# `||`, `split` refuses a null argument outright, and the failure is a
# provider-level error the validation block can neither catch nor report. The
# variable is null by default and null in test, stage and prod, so the shape
# above made `terraform plan` impossible in three of the four environments —
# a gate that refused every good value while never once reporting the bad one
# it was written for.
#
# The acceptance suite had a test asserting the /26 floor "fires and admits",
# which passed throughout, because it proved the rule by re-implementing the
# arithmetic in Rust. A mirror of an expression is not the expression. Only a
# plan sees what Terraform does with a null.
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

run "a_null_range_is_admitted_and_creates_no_subnet" {
  command = plan

  variables {
    console_egress_cidr = null
  }

  # The case that killed the plan. Null is not a bad value here — it is the
  # default, and it means an environment whose console has no route to the
  # platform. It must reach the end of the plan and create nothing.
  assert {
    condition     = length(google_compute_subnetwork.console_egress) == 0
    error_message = "a null console_egress_cidr created a subnet; a range nobody decided on is not a range"
  }
}

run "the_documented_floor_is_admitted" {
  command = plan

  variables {
    # The value dev actually carries, so this run also says the committed
    # environment still plans.
    console_egress_cidr = "10.0.16.0/26"
  }

  assert {
    condition     = length(google_compute_subnetwork.console_egress) == 1
    error_message = "the /26 floor no longer produces the console's egress subnet"
  }
}

run "a_range_wider_than_the_floor_is_admitted" {
  command = plan

  variables {
    console_egress_cidr = "10.0.16.0/24"
  }

  assert {
    condition     = length(google_compute_subnetwork.console_egress) == 1
    error_message = "a /24 is wider than the /26 floor and must be admitted; a gate that refuses it refuses everything above the floor too"
  }
}

run "a_range_smaller_than_the_floor_is_refused" {
  command = plan

  variables {
    # Google refuses direct VPC egress on anything below a /26, and does so at
    # apply — after the subnet and the instance template already exist. This
    # is the refusal the validation was written for and had never performed.
    console_egress_cidr = "10.0.16.0/28"
  }

  expect_failures = [var.console_egress_cidr]
}

run "a_value_that_is_not_a_range_at_all_is_refused" {
  command = plan

  variables {
    console_egress_cidr = "the console subnet"
  }

  expect_failures = [var.console_egress_cidr]
}
