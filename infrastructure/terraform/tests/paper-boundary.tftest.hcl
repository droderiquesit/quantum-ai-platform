# The first of the three paper-trading layers, planned.
#
# `.claude/rules/01-security-and-safety.md` names Terraform as layer one: the
# root refuses `supervised_live`, `limited_autonomous_live` and
# `autonomous_live` at plan time, so a live ceiling never reaches the
# environment every catalogue workload reads it from. The refusal has been
# asserted by the Rust acceptance suite since it was written — but by reading
# `variables.tf` as text. Text is not a plan. Until this file there was no
# evidence that Terraform, given `autonomous_live`, stops; only evidence that
# a file contains a block that says it should.
#
# The distinction is not academic in this repository. The sibling validation
# on `modules/network`'s `console_egress_cidr` read correctly, had a Rust test
# asserting it both fires and admits, and could not run at all: it handed a
# null to `split` and killed the plan before any message was reached. That
# went unnoticed for as long as nothing planned. So this file plans.
#
# It needs no credential and reaches no project. `mock_provider` replaces both
# providers, so nothing here can create, read or delete anything, and every
# run is `command = plan`. It is also the only way to plan this root without
# credentials at all: `terraform plan` insists on initialising the GCS backend
# and would reach the state bucket; `terraform test` keeps its own state in
# memory.
#
# Three modules are overridden with fixed outputs. Not to hide anything: a
# mocked provider leaves every computed attribute unknown at plan time, and
# these three drive `for_each` from service-account emails and bucket names
# the mock cannot know. Overriding them replaces three modules' internals, and
# nothing in the ceiling's path goes through any of them.

mock_provider "google" {}
mock_provider "google-beta" {}

variables {
  project_id        = "ceiling-plan-harness"
  project_number    = 123456789012
  environment       = "dev"
  github_repository = "example/example"

  # The four zones dev declares. The catalogue refuses a workload whose zone
  # this environment did not declare, so a harness with no zones would fail on
  # that precondition and prove nothing about the ceiling.
  trust_zones = {
    "application-identity" = { region = "us-east4", subnet_cidr = "10.0.32.0/24" }
    "cognition"            = { region = "us-east4", subnet_cidr = "10.0.33.0/24" }
    "intelligence"         = { region = "us-east4", subnet_cidr = "10.0.34.0/24" }
    "management"           = { region = "us-east4", subnet_cidr = "10.0.35.0/24" }
  }
}

# --- the admitting half ------------------------------------------------------

run "the_paper_ceiling_plans_to_the_end_and_reaches_no_venue" {
  command = plan

  override_module {
    target = module.ai
    outputs = {
      training_bucket         = "harness-training"
      metadata_store_id       = "projects/ceiling-plan-harness/locations/us-east4/metadataStores/harness"
      serving_endpoint_id     = null
      reachable_by_this_build = false
    }
  }
  override_module {
    target = module.evidence
    outputs = {
      bucket_name       = "harness-evidence"
      bucket_url        = "gs://harness-evidence"
      encryption_key_id = "projects/ceiling-plan-harness/locations/us-east4/keyRings/harness/cryptoKeys/evidence"
    }
  }
  override_module {
    target = module.registry
    outputs = {
      repository_id   = "projects/ceiling-plan-harness/locations/us-east4/repositories/harness"
      repository_name = "harness"
      image_prefix    = "us-east4-docker.pkg.dev/ceiling-plan-harness/harness"
    }
  }

  variables {
    autonomy_ceiling = "paper_trading"
  }

  # The half the infrastructure rules say distinguishes a working gate from
  # one that refuses everything. A configuration that refused every ceiling
  # would pass all four refusals below and stop every deployment for a reason
  # nobody could find.
  assert {
    condition     = output.autonomy_ceiling == "paper_trading"
    error_message = "the ceiling a plan carries is not the one it was given"
  }

  # And the derived answer, which was once this sentence backwards: it read
  # `!= "paper_trading"` and so reported the two rungs *below* paper trading
  # as live-capable. Asserted on the output rather than on the local, because
  # the output is what an operator reads.
  assert {
    condition     = output.live_capable == false
    error_message = "a paper-trading environment reports itself live-capable; the venue credential is granted on this predicate"
  }
}

run "the_two_rungs_below_paper_trading_are_admitted_and_still_reach_no_venue" {
  command = plan

  override_module {
    target = module.ai
    outputs = {
      training_bucket         = "harness-training"
      metadata_store_id       = "projects/ceiling-plan-harness/locations/us-east4/metadataStores/harness"
      serving_endpoint_id     = null
      reachable_by_this_build = false
    }
  }
  override_module {
    target = module.evidence
    outputs = {
      bucket_name       = "harness-evidence"
      bucket_url        = "gs://harness-evidence"
      encryption_key_id = "projects/ceiling-plan-harness/locations/us-east4/keyRings/harness/cryptoKeys/evidence"
    }
  }
  override_module {
    target = module.registry
    outputs = {
      repository_id   = "projects/ceiling-plan-harness/locations/us-east4/repositories/harness"
      repository_name = "harness"
      image_prefix    = "us-east4-docker.pkg.dev/ceiling-plan-harness/harness"
    }
  }

  variables {
    # `observation` is the rung an operator reaches for when hardening an
    # environment. A gate that refused it would push them back up to
    # paper_trading to get a plan, which is the opposite of what it is for.
    autonomy_ceiling = "observation"
  }

  assert {
    condition     = output.live_capable == false
    error_message = "observation is below paper trading and must not report as live-capable"
  }
}

# --- the refusing half -------------------------------------------------------
#
# One run per live rung, rather than one run naming the widest. The acceptance
# suite already records why: `contains("autonomous_live")` is true of
# `"limited_autonomous_live"`, so a single case can pass with two of the three
# rungs admitted. These are three separate plans.

run "a_supervised_live_ceiling_stops_the_plan" {
  command = plan

  variables {
    autonomy_ceiling = "supervised_live"
  }

  expect_failures = [var.autonomy_ceiling]
}

run "a_limited_autonomous_live_ceiling_stops_the_plan" {
  command = plan

  variables {
    autonomy_ceiling = "limited_autonomous_live"
  }

  expect_failures = [var.autonomy_ceiling]
}

run "an_autonomous_live_ceiling_stops_the_plan" {
  command = plan

  variables {
    autonomy_ceiling = "autonomous_live"
  }

  expect_failures = [var.autonomy_ceiling]
}

run "a_ceiling_outside_the_six_rungs_stops_the_plan" {
  command = plan

  variables {
    # The other validation on the same variable, and the reason there are two:
    # a misspelling and a forbidden level fail differently, so the operator
    # reading the message is told which mistake they made.
    autonomy_ceiling = "paper-trading"
  }

  expect_failures = [var.autonomy_ceiling]
}
