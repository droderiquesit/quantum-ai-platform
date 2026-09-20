# The zone model, planned.
#
# NOT-ENFORCED-HERE.md said, for as long as this module has existed, that
# `terraform fmt`, `validate` and `plan` had never been run against it because
# no Terraform binary was available where it was written — so every
# `lifecycle.precondition` in main.tf was an assertion about an assertion, and
# the two-sided proof the infrastructure rules demand had never been produced
# for any of them. This file is that proof, and it is a real plan: Terraform
# walks the configuration, evaluates every variable validation and every
# precondition, and reports what it refused.
#
# It needs no credential and it reaches no project. `mock_provider` replaces
# the google provider entirely, so nothing here can create, read or delete a
# cloud resource; every run is `command = plan`, so nothing is applied even in
# the mock. That matters more than convenience: the dev environment has been
# torn down, there is no project to plan against, and a gate that can only be
# exercised against live infrastructure is a gate nobody exercises.
#
# Every refusal below is paired with an admission of the same shape. A gate
# that refuses everything passes a one-sided test and stops a deployment for
# reasons nobody can find; the infrastructure rules name that pairing as the
# thing that distinguishes a working gate from a broken one, and the pairs are
# written adjacent so a reader can see the pairing rather than take it on
# trust.

mock_provider "google" {}

variables {
  project_id  = "tz-plan-harness"
  environment = "dev"
  region      = "us-east4"
  network_id  = "projects/tz-plan-harness/global/networks/harness"
}

# --- the zone vocabulary -----------------------------------------------------

run "the_thirteen_blueprint_zone_names_are_admitted" {
  command = plan

  variables {
    zones = {
      "public-edge"          = { region = "us-east4", subnet_cidr = "10.90.0.0/24" }
      "application-identity" = { region = "us-east4", subnet_cidr = "10.90.1.0/24" }
      "ingestion-discovery"  = { region = "us-east4", subnet_cidr = "10.90.2.0/24" }
      "cognition"            = { region = "us-east4", subnet_cidr = "10.90.3.0/24" }
      "valuation"            = { region = "us-east4", subnet_cidr = "10.90.4.0/24" }
      "intelligence"         = { region = "us-east4", subnet_cidr = "10.90.5.0/24" }
      "optimisation"         = { region = "us-east4", subnet_cidr = "10.90.6.0/24" }
      "control-fabric"       = { region = "us-east4", subnet_cidr = "10.90.7.0/24" }
      "execution"            = { region = "us-east4", subnet_cidr = "10.90.8.0/24" }
      "ledger"               = { region = "us-east4", subnet_cidr = "10.90.9.0/24" }
      "wallet-read"          = { region = "us-east4", subnet_cidr = "10.90.10.0/24" }
      "treasury-write"       = { region = "us-east4", subnet_cidr = "10.90.11.0/24" }
      "management"           = { region = "us-east4", subnet_cidr = "10.90.12.0/24" }
    }
  }

  # Thirteen zones means twenty-six deny rules, one pair each. Asserted rather
  # than assumed: `var.zones` could be admitted and the deny rules generated
  # from some other collection, and a zone with no deny rule is a zone that
  # reaches everything while reading as governed.
  assert {
    condition     = length(google_compute_firewall.deny_egress) == 13 && length(google_compute_firewall.deny_ingress) == 13
    error_message = "every declared zone gets a deny rule in each direction; one that does not is open by default"
  }
}

run "a_fourteenth_zone_name_is_refused_rather_than_created" {
  command = plan

  variables {
    zones = {
      # Plausible, adjacent to a real one, and not in the blueprint. A name
      # this module does not know is a zone with no sanctioned paths and no
      # sanctioned egress — governed-looking and ungoverned.
      "observability" = { region = "us-east4", subnet_cidr = "10.90.20.0/24" }
    }
  }

  expect_failures = [var.zones]
}

run "two_zones_sharing_one_range_are_refused" {
  command = plan

  variables {
    zones = {
      # The path rules name ranges. Two zones on one range is one zone with a
      # boundary drawn on paper between its halves.
      "cognition"    = { region = "us-east4", subnet_cidr = "10.90.30.0/24" }
      "optimisation" = { region = "us-east4", subnet_cidr = "10.90.30.0/24" }
    }
  }

  expect_failures = [var.zones]
}

# --- IBM Quantum egress, the sharpest rule in the model ----------------------

run "an_ibm_destination_on_optimisation_is_admitted" {
  command = plan

  variables {
    zones = {
      "optimisation" = { region = "us-east4", subnet_cidr = "10.90.6.0/24" }
    }
    external_egress = {
      "ibm-quantum-runtime" = {
        zone    = "optimisation"
        cidr    = "104.16.0.0/24"
        port    = 443
        purpose = "ibm-quantum"
        note    = "IBM Quantum Runtime, from the one zone §46.1 permits to reach it."
      }
    }
  }

  # The admitting half. Without it, the refusal below proves only that the
  # module says no to something, not that it says yes to the one thing the
  # blueprint permits — and the sanctioned-purpose table could have been
  # emptied entirely without this run noticing.
  assert {
    condition     = length(google_compute_firewall.external_egress) == 1
    error_message = "the one sanctioned IBM destination did not produce an egress rule; the optimisation zone can no longer reach IBM at all"
  }
}

run "an_ibm_destination_on_cognition_is_refused" {
  command = plan

  variables {
    zones = {
      "cognition" = { region = "us-east4", subnet_cidr = "10.90.3.0/24" }
    }
    external_egress = {
      # The real temptation: qip-deepbrain links qip-optimization-engine and
      # sits in `cognition`, so this is the entry somebody writes when an IBM
      # call fails. It has to be refused at plan time, because the fix is to
      # split the workload rather than to widen the zone.
      "ibm-quantum-runtime" = {
        zone    = "cognition"
        cidr    = "104.16.0.0/24"
        port    = 443
        purpose = "ibm-quantum"
        note    = "the deep brain links the optimisation engine and wants the endpoint"
      }
    }
  }

  expect_failures = [google_compute_firewall.external_egress["ibm-quantum-runtime"]]
}

run "an_egress_purpose_no_zone_holds_is_refused" {
  command = plan

  variables {
    zones = {
      # Ingestion has the widest external surface on the platform and may
      # reach nothing that moves money. A withdrawal API is money.
      "ingestion-discovery" = { region = "us-east4", subnet_cidr = "10.90.2.0/24" }
    }
    external_egress = {
      "withdrawal-endpoint" = {
        zone    = "ingestion-discovery"
        cidr    = "203.0.113.0/24"
        port    = 443
        purpose = "withdrawal-api"
        note    = "a source adapter that also settles would be one compromise from a transfer"
      }
    }
  }

  expect_failures = [google_compute_firewall.external_egress["withdrawal-endpoint"]]
}

run "an_information_source_for_ingestion_is_admitted" {
  command = plan

  variables {
    zones = {
      "ingestion-discovery" = { region = "us-east4", subnet_cidr = "10.90.2.0/24" }
    }
    external_egress = {
      "reference-data-feed" = {
        zone    = "ingestion-discovery"
        cidr    = "203.0.113.0/24"
        port    = 443
        purpose = "information-source"
        note    = "the purpose ingestion does hold, so the refusal above is about the purpose and not about the zone"
      }
    }
  }

  assert {
    condition     = length(google_compute_firewall.external_egress) == 1
    error_message = "ingestion can no longer reach an information source; the sanctioned-purpose table refuses everything"
  }
}

run "an_allowlist_entry_wider_than_a_24_is_refused" {
  command = plan

  variables {
    zones = {
      "ingestion-discovery" = { region = "us-east4", subnet_cidr = "10.90.2.0/24" }
    }
    external_egress = {
      "a-whole-provider" = {
        zone    = "ingestion-discovery"
        cidr    = "203.0.0.0/8"
        port    = 443
        purpose = "information-source"
        note    = "a range chosen for convenience is the range that turns out to contain something else"
      }
    }
  }

  expect_failures = [var.external_egress]
}

# --- the internal adjacency --------------------------------------------------

run "a_sanctioned_path_is_admitted_and_written_in_both_directions" {
  command = plan

  variables {
    zones = {
      "application-identity" = { region = "us-east4", subnet_cidr = "10.90.1.0/24" }
      "ledger"               = { region = "us-east4", subnet_cidr = "10.90.9.0/24" }
    }
    permitted_paths = {
      "application-reads-ledger" = {
        from  = "application-identity"
        to    = "ledger"
        mode  = "read"
        ports = [9010]
        note  = "the portal reads positions; §46.1 gives the application zone a read path and nothing wider."
      }
    }
  }

  # Both halves, because the deny is in both directions: a path written only
  # outbound is traffic that leaves and is dropped on arrival, and it is
  # diagnosed for a day before anyone looks at the ingress rule.
  assert {
    condition     = length(google_compute_firewall.path_egress) == 1 && length(google_compute_firewall.path_ingress) == 1
    error_message = "a permitted path no longer produces a rule in each direction; half a path reads as a path"
  }
}

run "the_wallet_read_path_may_not_reach_the_treasury_write_path" {
  command = plan

  variables {
    zones = {
      "wallet-read"    = { region = "us-east4", subnet_cidr = "10.90.10.0/24" }
      "treasury-write" = { region = "us-east4", subnet_cidr = "10.90.11.0/24" }
    }
    permitted_paths = {
      # There is no key joining these two in either direction in
      # local.sanctioned_paths, and the absence is the control. A route here
      # is the one that turns a read surface into a path to a transfer.
      "wallet-tells-treasury" = {
        from  = "wallet-read"
        to    = "treasury-write"
        mode  = "read"
        ports = [9020]
        note  = "convenient, and the reason the two are separate zones at all"
      }
    }
  }

  expect_failures = [google_compute_firewall.path_egress["wallet-tells-treasury"]]
}

run "a_sanctioned_pair_declared_with_an_unsanctioned_mode_is_refused" {
  command = plan

  variables {
    zones = {
      "application-identity" = { region = "us-east4", subnet_cidr = "10.90.1.0/24" }
      "ledger"               = { region = "us-east4", subnet_cidr = "10.90.9.0/24" }
    }
    permitted_paths = {
      # The pair is sanctioned; only `read` is. `append` would earn this zone
      # roles/spanner.databaseUser, which updates and deletes — the mode is
      # not decoration, it decides an IAM grant.
      "application-writes-ledger" = {
        from  = "application-identity"
        to    = "ledger"
        mode  = "append"
        ports = [9010]
        note  = "the pair is permitted, so only the mode check can refuse this"
      }
    }
  }

  expect_failures = [google_compute_firewall.path_egress["application-writes-ledger"]]
}

run "a_path_to_a_zone_that_was_never_declared_is_refused" {
  command = plan

  variables {
    zones = {
      "application-identity" = { region = "us-east4", subnet_cidr = "10.90.1.0/24" }
    }
    permitted_paths = {
      "application-reads-ledger" = {
        from  = "application-identity"
        to    = "ledger"
        mode  = "read"
        ports = [9010]
        note  = "a rule targeting a zone with no subnet is a rule targeting nothing, and it reads as a boundary"
      }
    }
  }

  expect_failures = [google_compute_firewall.path_egress["application-reads-ledger"]]
}

# --- where a client may arrive -----------------------------------------------

run "the_public_edge_may_be_given_a_load_balancer" {
  command = plan

  variables {
    zones = {
      "public-edge" = { region = "us-east4", subnet_cidr = "10.90.0.0/24" }
    }
    public_ingress = {
      "static-shell" = {
        zone = "public-edge"
        port = 443
        note = "the static shell §40.5 puts behind Cloud CDN; the one door on the platform."
      }
    }
  }

  assert {
    condition     = length(google_compute_firewall.public_ingress) == 1
    error_message = "the public edge can no longer be reached from Google's load-balancer ranges, so the refusal below proves nothing"
  }
}

run "a_load_balancer_in_front_of_the_execution_zone_is_refused" {
  command = plan

  variables {
    zones = {
      "execution" = { region = "us-east4", subnet_cidr = "10.90.8.0/24" }
    }
    public_ingress = {
      # §40.5: customer traffic and trading traffic never share a load
      # balancer, an identity, a credential or a route. This is the single
      # declaration that would break all four at once.
      "node-health" = {
        zone = "execution"
        port = 443
        note = "somebody wants to see a node's health page from a browser"
      }
    }
  }

  expect_failures = [google_compute_firewall.public_ingress["node-health"]]
}

# --- the regional NAT --------------------------------------------------------

run "a_zone_egressing_from_this_region_is_given_a_nat" {
  command = plan

  variables {
    region = "us-east4"
    zones = {
      "ingestion-discovery" = { region = "us-east4", subnet_cidr = "10.90.2.0/24" }
    }
    external_egress = {
      "reference-data-feed" = {
        zone    = "ingestion-discovery"
        cidr    = "203.0.113.0/24"
        port    = 443
        purpose = "information-source"
        note    = "the in-region case, so that the refusal below is about the region and not about declaring egress at all"
      }
    }
  }

  assert {
    condition     = length(google_compute_router_nat.egress) == 1 && length(google_compute_router.egress) == 1
    error_message = "a zone with a sanctioned destination in this region got no NAT; it would have firewall rules permitting a route it cannot take"
  }
}

run "a_zone_declaring_no_egress_is_given_no_nat" {
  command = plan

  variables {
    region = "us-east4"
    zones = {
      "cognition" = { region = "us-east4", subnet_cidr = "10.90.3.0/24" }
    }
  }

  # The third state, and the reason the count cannot simply be one. A platform
  # that reaches nothing outside the VPC creates no router and no NAT at all;
  # counting them into existence would give every zone a translation waiting
  # for a firewall rule somebody widens later.
  assert {
    condition     = length(google_compute_router_nat.egress) == 0 && length(google_compute_router.egress) == 0
    error_message = "a deployment with an empty allowlist created a NAT; egress capability should not exist before a destination is argued for"
  }
}

run "a_zone_egressing_from_another_region_is_refused" {
  command = plan

  variables {
    region = "us-east4"
    zones = {
      "ingestion-discovery" = { region = "europe-west4", subnet_cidr = "10.90.2.0/24" }
    }
    external_egress = {
      "reference-data-feed" = {
        zone    = "ingestion-discovery"
        cidr    = "203.0.113.0/24"
        port    = 443
        purpose = "information-source"
        note    = "a Cloud NAT is regional; a zone NAT'd through another region leaves from an address no counterparty allowlisted"
      }
    }
  }

  expect_failures = [google_compute_router_nat.egress[0]]
}

# --- the identities placed in a zone ----------------------------------------

# `zone_identities`' description promised, from the day the variable was
# written, that "a zone named here that is not in `zones` is refused". Until
# 2026-09-19 no validation implemented it: the one block checked the thirteen
# names, so an identity under a blueprint zone this environment never declared
# was admitted, earned nothing (the grant lists iterate `permitted_paths`
# filtered to declared zones), sat under no rule, and was listed in the root's
# output under a zone as if it were governed. A promise in a description is
# the exact shape of control that reads as protection and cannot fire. The
# three runs below are the refusal, the admission that proves an identity in
# a declared zone actually becomes a grant, and the one shape the root sends
# in three of four environments — a zone named with nobody in it.

run "an_identity_placed_in_a_zone_this_deployment_never_declared_is_refused" {
  command = plan

  variables {
    zones = {
      "cognition" = { region = "us-east4", subnet_cidr = "10.90.3.0/24" }
    }
    zone_identities = {
      # A node's identity, in an environment that declared no execution
      # subnet. The name is one of the thirteen, so the first validation
      # admits it; only the second can see that the zone is not here.
      "execution" = ["qip-dev-newyork-1-node@tz-plan-harness.iam.gserviceaccount.com"]
    }
  }

  expect_failures = [var.zone_identities]
}

run "an_identity_in_a_declared_zone_with_a_read_path_earns_the_ledger_grant" {
  command = plan

  variables {
    zones = {
      "application-identity" = { region = "us-east4", subnet_cidr = "10.90.1.0/24" }
      "ledger"               = { region = "us-east4", subnet_cidr = "10.90.9.0/24" }
    }
    permitted_paths = {
      "application-reads-ledger" = {
        from  = "application-identity"
        to    = "ledger"
        mode  = "read"
        ports = [9010]
        note  = "§46.1: application and identity may read the ledger"
      }
    }
    zone_identities = {
      "application-identity" = [
        "qip-dev-api@tz-plan-harness.iam.gserviceaccount.com",
        "qip-dev-web@tz-plan-harness.iam.gserviceaccount.com",
      ]
    }
    ledger_database = {
      instance = "qip-dev-ledger"
      database = "ledger"
    }
  }

  # The admitting half, and it asserts on the grant rather than on the plan
  # succeeding: two identities placed in a zone with a `read` path are two
  # `databaseReader` bindings and no `databaseUser`. A validation that
  # refused every non-empty list would fail here, which is what makes the
  # refusal above evidence of a gate and not of a module that says no.
  assert {
    condition     = length(google_spanner_database_iam_member.ledger_read) == 2 && length(google_spanner_database_iam_member.ledger_append) == 0
    error_message = "two identities in a declared zone with a read path did not become exactly two ledger read grants; the identity list no longer reaches the grant, or the mode no longer decides the role"
  }
}

run "a_zone_named_with_no_identity_in_it_is_admitted_even_if_undeclared" {
  command = plan

  variables {
    zones = {
      "cognition" = { region = "us-east4", subnet_cidr = "10.90.3.0/24" }
    }
    zone_identities = {
      # The root's real shape wherever OpenObserve is off: `management` is
      # merged into the map unconditionally and arrives empty. An empty list
      # places nobody, so there is nothing outside a boundary to refuse, and a
      # gate that refused it would refuse every plan in test, stage and prod.
      "management" = []
    }
  }

  assert {
    condition     = length(google_compute_firewall.deny_egress) == 1
    error_message = "a zone named with an empty identity list stopped the plan; the root sends exactly this shape in every environment without OpenObserve"
  }
}
