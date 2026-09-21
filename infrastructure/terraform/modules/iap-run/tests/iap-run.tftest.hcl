# The Cloud Run IAP door, planned.
#
# Written with the module, because a module that has never been planned is a
# module whose preconditions are assertions about assertions — the state
# `modules/trust-zones` was in until the day its harness was added, and the
# state in which two of its controls turned out not to fire.
#
# Mocked provider, `command = plan` throughout: no credential, no project,
# nothing created. Every refusal is paired with the admission of the same
# shape, because the half that matters is the second one — a gate proven only
# to refuse is a gate that may refuse everything, and `qip-acceptance`'s
# `terraform_plan` suite fails a harness that proves only one of the two.

mock_provider "google" {}

# `google-beta` too, for the one resource here that is beta-only:
# `google_project_service_identity`, which asks Google to create IAP's
# service agent. Without this block every run in this file fails at
# provider configuration rather than on its own assertion, and a harness
# that cannot reach its assertions proves nothing while still being run.
mock_provider "google-beta" {}

variables {
  project_id     = "iap-run-plan-harness"
  project_number = "95200532413"
  environment    = "dev"
  region         = "us-east4"
  service_name   = "qip-dev-portal"
  trust_zone     = "application-identity"
}

# --- the door with nobody on the list ------------------------------------------

run "an_empty_access_list_plans_cleanly_and_grants_nobody" {
  command = plan

  # The state every environment is committed in, and the one that has to work:
  # an IAM member is an account identifier, this repository carries none, and
  # a door that could not be planned without one would be a door nobody could
  # commit. Zero bindings is the posture — not a placeholder, and not a
  # principal somebody forgot to remove.
  variables {
    iap_members = []
  }

  assert {
    condition     = length(google_iap_web_cloud_run_service_iam_member.console) == 0
    error_message = "an empty iap_members produced an IAM binding; the committed state of every environment grants somebody something"
  }

  # The URL is asserted as the exact string rather than as a pattern. A
  # pattern would admit `https://qip-dev-portal-.us-east4.run.app` — the
  # shape a null project number produces — which resolves to nothing and
  # reads as a DNS fault.
  #
  # **What this pins is the documented convention, not this project's actual
  # hostname, and the two are not the same thing today.** `infra.yml` run
  # 35636247990 (`diagnose`, dev, 2026-09-21) read `status.url` off
  # `qip-dev-openobserve` — the one service in `algorik-dev` with
  # `RoutesReady=True` — and got the legacy
  # `<service>-<token>-<region-code>.a.run.app` form. The token is not
  # derivable from anything Terraform holds, so no assertion here can pin the
  # real address, and this one is deliberately not rewritten to pretend
  # otherwise: it guards the derivation against drift, and
  # `modules/iap-run/outputs.tf` and `README.md` both say in their first
  # paragraph that the derivation is not the address to open. Believe the
  # `diagnose` reading over this string.
  assert {
    condition     = output.url == "https://qip-dev-portal-95200532413.us-east4.run.app"
    error_message = "the derived run.app URL is not the deterministic form Cloud Run assigns; a console nobody can find is a console that is not deployed"
  }

  # No registrar, no zone, no record, no certificate to wait on. Stated as an
  # assertion rather than as prose because the absence of a resource is
  # exactly what prose stops describing correctly: this run plans the entire
  # module, and if a future edit adds an address or a certificate here, this
  # fails.
  assert {
    condition     = output.grant_command != "" && !strcontains(output.url, "algorik.ai")
    error_message = "the door's URL names a domain this repository would have to own; the point of ADR 0095 is a hostname Google issues"
  }

  # The grant command, as the exact string, for the same reason the URL is.
  #
  # `!= ""` above guards almost nothing: every wrong command is also
  # non-empty, and the two ways this one goes wrong are both silent. Drop
  # `--resource-type=cloud-run` and the same subcommand edits the **project's**
  # IAP policy — it succeeds, it prints nothing alarming, and it makes the
  # wide grant ADR 0095 narrowed away from. Drop or mistype `--region` and the
  # binding is made in a region the service is not in, which also applies
  # cleanly and admits nobody to the console anybody is trying to reach.
  #
  # This is the one step the repository deliberately cannot take on the
  # owner's behalf, so it is the one string a person will paste without
  # reading. Asserting it whole is what makes it safe to paste.
  assert {
    condition = output.grant_command == join(" ", [
      "gcloud iap web add-iam-policy-binding",
      "--project=iap-run-plan-harness",
      "--resource-type=cloud-run",
      "--region=us-east4",
      "--service=qip-dev-portal",
      "--role=roles/iap.httpsResourceAccessor",
      "--member='user:YOU@example.com'",
    ])
    error_message = "the printed grant command is not the invocation that admits a person to this one Cloud Run service; without --resource-type=cloud-run it edits the project's IAP policy instead, and with the wrong --region it binds in a region the service is not in — both succeed and neither opens the console"
  }
}

# --- the access list, when there is one ----------------------------------------

run "a_named_operator_is_bound_per_service_rather_than_per_project" {
  command = plan

  # The admitting half. The refusals below reject `allUsers` and
  # `allAuthenticatedUsers`; without this run they would also be satisfied by
  # a validation that rejected every member, which is a console nobody can
  # ever be granted.
  variables {
    iap_members = ["group:qip-operators@example.com"]
  }

  assert {
    condition     = length(google_iap_web_cloud_run_service_iam_member.console) == 1
    error_message = "a named member produced no binding; the access list admits nobody it was given"
  }

  assert {
    condition = alltrue([
      for binding in google_iap_web_cloud_run_service_iam_member.console :
      binding.role == "roles/iap.httpsResourceAccessor"
    ])
    error_message = "the binding grants a role other than roles/iap.httpsResourceAccessor; any other role either admits nothing or admits more than passing the door"
  }

  # The narrowing ADR 0095 is for. The binding names one service in one
  # region: a person admitted to the console is not thereby admitted to Argo
  # CD or Kargo, which is what the project-level grant ADR 0094 decision 3 had
  # to use could never manage.
  assert {
    condition = alltrue([
      for binding in google_iap_web_cloud_run_service_iam_member.console :
      binding.cloud_run_service_name == "qip-dev-portal" && binding.location == "us-east4"
    ])
    error_message = "the IAP binding does not name the one service and region it guards; a binding in the wrong location applies cleanly and admits nobody to the service anybody is trying to reach"
  }
}

# --- what the list may never contain -------------------------------------------

run "the_public_internet_is_refused_on_the_access_list" {
  command = plan

  variables {
    iap_members = ["allUsers"]
  }

  expect_failures = [var.iap_members]
}

run "every_google_account_in_existence_is_refused_on_the_access_list" {
  command = plan

  variables {
    iap_members = ["allAuthenticatedUsers"]
  }

  expect_failures = [var.iap_members]
}

# Refused even when it is buried in a list whose other entries are fine, which
# is the shape a widening arrives in: nobody commits `iap_members =
# ["allUsers"]`, and somebody appends it to a list that already works.
run "the_public_internet_is_refused_even_beside_a_legitimate_member" {
  command = plan

  variables {
    iap_members = ["group:qip-operators@example.com", "allUsers"]
  }

  expect_failures = [var.iap_members]
}

# --- the trust zone ------------------------------------------------------------

run "a_door_in_front_of_a_trading_zone_is_refused" {
  command = plan

  variables {
    trust_zone = "execution"
  }

  expect_failures = [terraform_data.client_reachable_zone]
}

run "the_public_edge_zone_is_admitted_beside_application_identity" {
  command = plan

  # The second admitting half. §46.1 marks two zones client-reachable and the
  # precondition names both; a test that only ever passed `application-identity`
  # would pass identically against a precondition hard-coded to that one value.
  variables {
    trust_zone = "public-edge"
  }

  assert {
    condition     = terraform_data.client_reachable_zone.input == "public-edge"
    error_message = "the client-reachable precondition refused a zone §46.1 marks client-reachable"
  }
}

# --- the project number --------------------------------------------------------

run "a_project_id_in_the_project_number_is_refused" {
  command = plan

  # The failure this prevents has a shape: `algorik-dev` in the numeric slot
  # produces `https://qip-dev-portal-algorik-dev.us-east4.run.app`, which is
  # a perfectly well-formed hostname that resolves to nothing, and the
  # operator reads it as a Cloud Run outage.
  variables {
    project_number = "algorik-dev"
  }

  expect_failures = [var.project_number]
}
