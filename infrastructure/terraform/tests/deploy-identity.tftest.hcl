# The deployment identity, planned: the repository gate both refuses a bad
# value and admits a good one.
#
# `var.github_repository` is the whole of what decides who may federate into
# this project. It reaches two seams — the pool provider's CEL condition and
# the principal set permitted to impersonate the pipeline account — and a wrong
# value at either is not a broken build but somebody else's pipeline holding
# this project's deploy account.
#
# The variable has had a validation for as long as it has existed. It had no
# harness, which is the gap this file closes, and `.claude/rules/domains/
# infrastructure.md` records why the gap mattered: an identity set from a
# repository variable once carried an apt-install advisory into the
# workload-identity audience, and every run afterwards failed on an audience
# nobody could explain. That is the reason the identity is derived from
# committed tfvars and a `${{ vars.* }}` is refused — and a gate nobody has
# exercised is a gate nobody knows the shape of.
#
# Both halves are here, per ADR 0069 and the domain rule, because a gate proven
# only to refuse may refuse everything. The admitting runs assert on
# `output.deploy_attribute_condition`, which is read off
# `google_iam_workload_identity_pool_provider.github` rather than rebuilt from
# the variable, so severing the variable from the provider fails the run. An
# assertion rebuilt from `var.github_repository` would agree with itself
# whatever the provider was configured with.
#
# What this file does not prove, stated because a mocked plan invites being
# read as more than it is:
#
#   - Nothing about Google, and nothing about GitHub. Every provider here is
#     mocked. This says the configuration is coherent and that its refusal
#     fires on the values given; it has never spoken to IAM and has never seen
#     a token. Whether Google's CEL evaluator parses this condition the way it
#     reads here is Google's answer to give, and no plan asks for it.
#   - Nothing about a project already applied. A pool provider that exists with
#     a different condition is not compared against; a variable validation is
#     handed the value and never the prior state.
#   - Nothing about the repository being the right one. It proves the committed
#     value survives the gate and reaches the provider, not that somebody chose
#     the correct repository — that is the tfvars review, and it is why there
#     is no default on the variable.
#   - Nothing about the second seam, and that is a real gap rather than a
#     scoping choice. `var.github_repository` is interpolated twice in
#     `modules/cicd`: into the condition asserted below, and into the
#     `principalSet://` member of `google_service_account_iam_member
#     .github_impersonation`. Severing one leaves the other reading correctly.
#     The member embeds `google_iam_workload_identity_pool.github.name`, which
#     the provider generates, so under `command = plan` the whole string is
#     unknown and an assertion on it cannot be evaluated — Terraform refuses it
#     rather than passing vacuously. Every harness in this repository is
#     `command = plan` on purpose, so closing this needs either an applying run
#     under mocks, which would be the first in the tree and is a decision
#     rather than a lane's edit, or a seam that carries the repository without
#     the generated name. It is written down here rather than closed with an
#     assertion that would read as coverage.
#
# It needs no credential and reaches no project. `mock_provider` replaces both
# providers and every run is `command = plan`.

mock_provider "google" {}
mock_provider "google-beta" {}

override_module {
  target = module.ai
  outputs = {
    training_bucket         = "harness-training"
    metadata_store_id       = "projects/deploy-identity-harness/locations/us-east4/metadataStores/harness"
    serving_endpoint_id     = null
    reachable_by_this_build = false
  }
}
override_module {
  target = module.evidence
  outputs = {
    bucket_name       = "harness-evidence"
    bucket_url        = "gs://harness-evidence"
    encryption_key_id = "projects/deploy-identity-harness/locations/us-east4/keyRings/harness/cryptoKeys/evidence"
  }
}
override_module {
  target = module.registry
  outputs = {
    repository_id   = "projects/deploy-identity-harness/locations/us-east4/repositories/harness"
    repository_name = "harness"
    image_prefix    = "us-east4-docker.pkg.dev/deploy-identity-harness/harness"
  }
}

variables {
  project_id     = "deploy-identity-harness"
  project_number = 123456789012
  environment    = "dev"

  # The four zones dev declares. The catalogue refuses a workload whose zone
  # this environment did not declare, so a harness with no zones would fail on
  # that precondition long before it reached the pool provider.
  trust_zones = {
    "application-identity" = { region = "us-east4", subnet_cidr = "10.0.32.0/24" }
    "cognition"            = { region = "us-east4", subnet_cidr = "10.0.33.0/24" }
    "intelligence"         = { region = "us-east4", subnet_cidr = "10.0.34.0/24" }
    "management"           = { region = "us-east4", subnet_cidr = "10.0.35.0/24" }
  }
}

# --- the admitting half ------------------------------------------------------

run "the_committed_repository_plans_to_the_end_and_reaches_the_pool_provider" {
  command = plan

  variables {
    # The value all four environments commit. A harness that only ever named an
    # invented repository would not notice the shipped one being refused, and
    # the shipped one is the only value that has to work.
    github_repository = "droderiquesit/quantum-ai-platform"
  }

  # Asserted whole rather than by `contains`. The condition *is* the boundary:
  # every character of it decides who gets in, so there is no part of it a
  # change may make silently. `contains` would also walk into the substring
  # trap this repository has already been bitten by — "droderiquesit/quantum-ai
  # -platform" is a substring of "droderiquesit/quantum-ai-platform-staging",
  # and a condition admitting the second would satisfy a containment check
  # written for the first.
  #
  # The single quotes are load-bearing and are why the refusing half below
  # includes a value carrying one. They delimit the repository inside CEL, so a
  # repository able to contain a quote could close the literal and append its
  # own disjunct.
  assert {
    condition     = output.deploy_attribute_condition == "attribute.repository == 'droderiquesit/quantum-ai-platform' && attribute.ref.startsWith('refs/heads/')"
    error_message = "the committed repository did not reach the pool provider's condition intact, so the pool would admit a repository other than the one the tfvars name"
  }
}

run "a_repository_using_every_permitted_character_is_admitted" {
  command = plan

  variables {
    # Upper case, digits, a dot, an underscore and a hyphen on both sides —
    # every class GitHub permits in an owner or a repository name. A gate that
    # admits only the simplest shape is nearly as broken as one that refuses
    # everything: the platform would move to a new organisation and the failure
    # would read as an audience error rather than as a rejected name.
    github_repository = "Example-Org.io/qip_platform-v2.1"
  }

  assert {
    condition     = output.deploy_attribute_condition == "attribute.repository == 'Example-Org.io/qip_platform-v2.1' && attribute.ref.startsWith('refs/heads/')"
    error_message = "a repository using characters GitHub permits was admitted by the validation but did not reach the provider unchanged"
  }
}

# --- the refusing half -------------------------------------------------------
#
# One run per mistake rather than one naming the worst, for the reason the
# ceiling harness gives: a single case can pass while its neighbours are
# admitted.

run "a_repository_carrying_a_quote_stops_the_plan" {
  command = plan

  variables {
    # The one that is not a typo. `attribute_condition` interpolates this value
    # inside a single-quoted CEL literal, so a value able to carry a quote
    # closes the literal and appends its own disjunct — the condition below
    # would become `attribute.repository == 'x' || true || ...`, which is true
    # for every repository on GitHub. Refused by the character class rather
    # than by escaping, because an allowlist that cannot express a quote cannot
    # be escaped wrongly.
    github_repository = "x' || true || attribute.repository == 'y"
  }

  expect_failures = [var.github_repository]
}

run "a_shell_advisory_appended_to_the_repository_stops_the_plan" {
  command = plan

  variables {
    # The failure the domain rule records, in the shape it actually took: a
    # value captured from a shell that also printed an advisory on stdout. It
    # reached the workload-identity audience and every run afterwards failed
    # naming an audience nobody could explain. The gate refuses it on the
    # space, before it can become an audience.
    github_repository = "droderiquesit/quantum-ai-platform WARNING: apt does not have a stable CLI interface"
  }

  expect_failures = [var.github_repository]
}

run "a_repository_given_as_a_url_stops_the_plan" {
  command = plan

  variables {
    # What somebody pastes from a browser. Admitted, it would produce a
    # condition matching an `assertion.repository` GitHub never sends, so the
    # pipeline would be locked out rather than over-admitted — a failure that
    # costs a day and names nothing useful.
    github_repository = "https://github.com/droderiquesit/quantum-ai-platform"
  }

  expect_failures = [var.github_repository]
}

run "a_repository_with_a_trailing_path_stops_the_plan" {
  command = plan

  variables {
    # The other browser paste: a deep link. Named separately from the URL
    # because the two fail the regex on different halves — the scheme on the
    # colon, this one on the extra separator — and a check written against only
    # one of them admits the other.
    github_repository = "droderiquesit/quantum-ai-platform/tree/main"
  }

  expect_failures = [var.github_repository]
}

run "an_owner_with_no_repository_stops_the_plan" {
  command = plan

  variables {
    # Half the fact. The principal set would end at the owner, which in a
    # `principalSet://` membership is a prefix and not a name, so every
    # repository in the organisation would be inside the boundary.
    github_repository = "droderiquesit"
  }

  expect_failures = [var.github_repository]
}

run "an_empty_repository_stops_the_plan" {
  command = plan

  variables {
    # A tfvars line somebody cleared. Without the gate it reaches the condition
    # as `attribute.repository == ''`, which is syntactically fine, matches
    # nothing, and reports as an audience failure rather than as an empty
    # variable.
    github_repository = ""
  }

  expect_failures = [var.github_repository]
}
