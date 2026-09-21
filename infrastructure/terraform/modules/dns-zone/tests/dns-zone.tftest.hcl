# The domain's zone, planned.
#
# Written with the module, for the reason `modules/iap-edge`'s harness gives: a
# module that has never been planned is a module whose preconditions are
# assertions about assertions. Every safety refusal in this tree lives in a
# `validation` block or a `lifecycle.precondition`, and `terraform validate`
# evaluates neither.
#
# Mocked provider, `command = plan` throughout: no credential, no project,
# nothing created and no zone delegated. Every refusal below is paired with the
# admission of the same shape, because the half that matters is the second one.
# A domain validation proven only to refuse may refuse every domain, and a zone
# module that refuses everything is a delegation that never happens — which
# reads, from a one-sided harness, exactly like a strict gate. `qip-acceptance`'s
# `terraform_plan` suite fails a harness that proves only one half.

mock_provider "google" {}

variables {
  project_id  = "dns-zone-plan-harness"
  environment = "dev"
  labels      = {}
  domain      = "algorik.ai"

  # The shape the root builds out of the front-door modules' addresses. Here
  # they are literals because a mocked provider leaves a real
  # `google_compute_global_address.address` unknown at plan, and a validation
  # handed an unknown is skipped rather than run — so the refusing runs below
  # would pass vacuously against the real wiring. Literals here are the only
  # way to make the gate actually fire; a literal in the *root* is the defect
  # this module exists to prevent, and the infrastructure suite is what refuses
  # that.
  a_records = {
    "argocd.algorik.ai" = { address = "203.0.113.10", ttl_seconds = 300 }
    "kargo.algorik.ai"  = { address = "203.0.113.10", ttl_seconds = 300 }
    "portal.algorik.ai" = { address = "203.0.113.11", ttl_seconds = 300 }
  }
}

# --- the admitting half ------------------------------------------------------

run "a_reviewed_domain_and_three_front_doors_bring_up_a_public_signed_zone" {
  command = plan

  # The run that tells a strict gate from a broken one. The same expressions
  # that reject every shape below let the values dev's tfvars actually carry
  # through, and produce the zone and all three records.
  assert {
    condition     = google_dns_managed_zone.zone.dns_name == "algorik.ai."
    error_message = "the zone's dns_name is not the domain with the trailing dot the module is supposed to add; a zone named anything else is one nothing is ever delegated to"
  }

  # Public, and asserted because the wrong value here is invisible. A private
  # zone applies cleanly, shows in `terraform show` as a managed zone for the
  # domain, and leaves every name on the internet exactly as dark as before.
  # `modules/network` creates the private zone this project also has, so both
  # kinds exist here and a default would blur them.
  assert {
    condition     = google_dns_managed_zone.zone.visibility == "public"
    error_message = "the zone is not public; a private zone answers only inside the VPC and reads in a plan exactly like a working one"
  }

  # Signed. The DS at the registrar is the owner's separate decision — see the
  # `ds_record` output — but the signing is on from the first apply, because
  # turning it on later means a key rollover against a live delegation.
  assert {
    condition     = one(google_dns_managed_zone.zone.dnssec_config).state == "on"
    error_message = "DNSSEC signing is off; it is free, invisible to resolvers until a DS exists at the registrar, and awkward to enable later on a zone the domain is already delegated to"
  }

  # Every front door that exists gets a record, and it is an A record with the
  # trailing dot Cloud DNS wants. Three, not "at least one": a merge that
  # dropped the gateway's pair would leave this passing on the portal alone.
  assert {
    condition     = length(keys(google_dns_record_set.a)) == 3
    error_message = "the zone does not serve one record per front door; a door whose record was dropped is an address, a certificate and nothing anybody can type"
  }

  assert {
    condition     = google_dns_record_set.a["portal.algorik.ai"].name == "portal.algorik.ai." && google_dns_record_set.a["portal.algorik.ai"].type == "A"
    error_message = "the portal's record is not an A record named for the portal with a trailing dot"
  }

  # The address reaches the record. Without this the module could create three
  # correctly named records pointing at nothing in particular.
  assert {
    condition     = one(google_dns_record_set.a["portal.algorik.ai"].rrdatas) == "203.0.113.11"
    error_message = "the address given for the portal did not reach its record's rrdatas"
  }

  # And the argocd and kargo names share the gateway's one address, which is
  # the fact `modules/gitops-gateway` documents and the reason it has a single
  # `address` output rather than two.
  assert {
    condition     = one(google_dns_record_set.a["argocd.algorik.ai"].rrdatas) == one(google_dns_record_set.a["kargo.algorik.ai"].rrdatas)
    error_message = "argocd and kargo resolve to different addresses; they are served by one reserved global address and one forwarding rule"
  }

  # The TTL is the one the caller chose, not a provider default. A number
  # nobody chose is a promise nobody made about how long a mistake lasts.
  assert {
    condition     = google_dns_record_set.a["portal.algorik.ai"].ttl == 300
    error_message = "the record's TTL is not the value the caller passed; a TTL nobody chose is a promise nobody made"
  }
}

run "a_zone_with_no_front_doors_yet_is_still_a_zone" {
  command = plan

  variables {
    # The state on the day the domain is delegated and before any door is
    # switched on. It has to plan: if an empty map were refused, the zone could
    # only ever be created at the same moment as a front door, and the
    # delegation — which takes propagation time — could never be done first.
    a_records = {}
  }

  assert {
    condition     = google_dns_managed_zone.zone.dns_name == "algorik.ai." && length(keys(google_dns_record_set.a)) == 0
    error_message = "a zone with no records was refused; the delegation could then never be made before the first front door exists, and it is the delegation that takes propagation time"
  }
}

run "signing_can_be_turned_off_and_the_zone_still_comes_up" {
  command = plan

  variables {
    dnssec_enabled = false
  }

  # The other side of the `dnssec_config` assertion above. Without this the
  # module could be one that hard-codes `on` and ignores the variable, and the
  # first run would not know the difference.
  assert {
    condition     = one(google_dns_managed_zone.zone.dnssec_config).state == "off"
    error_message = "dnssec_enabled = false did not reach the zone; the variable would be a switch wired to nothing"
  }
}

# --- the refusing half: the domain -------------------------------------------

run "a_domain_carrying_a_trailing_dot_is_refused" {
  command = plan

  variables {
    # The shape somebody copies out of a zone file or out of Cloud DNS itself.
    # The module appends the dot, so this becomes `algorik.ai..` — which Cloud
    # DNS accepts and which nothing on the internet is ever delegated to. An
    # apply that reports success and cannot work.
    domain    = "algorik.ai."
    a_records = {}
  }

  expect_failures = [var.domain]
}

run "a_domain_carrying_a_scheme_is_refused" {
  command = plan

  variables {
    domain    = "https://algorik.ai"
    a_records = {}
  }

  expect_failures = [var.domain]
}

run "an_upper_case_domain_is_refused" {
  command = plan

  variables {
    # DNS is case-insensitive; a Cloud DNS zone name is not, and neither is the
    # suffix check every record in this module is held to.
    domain    = "Algorik.ai"
    a_records = {}
  }

  expect_failures = [var.domain]
}

run "a_wildcard_domain_is_refused" {
  command = plan

  variables {
    domain    = "*.algorik.ai"
    a_records = {}
  }

  expect_failures = [var.domain]
}

run "a_bare_name_with_no_dot_is_refused" {
  command = plan

  variables {
    # `algorik` is not a domain anybody can be delegated. It is what a hurried
    # edit leaves behind, and the zone it creates answers for a name that does
    # not exist.
    domain    = "algorik"
    a_records = {}
  }

  expect_failures = [var.domain]
}

# --- the refusing half: the records ------------------------------------------

run "a_record_name_that_is_not_inside_the_zone_is_refused" {
  command = plan

  variables {
    # Well formed, real address, and wrong in the only way that matters. Cloud
    # DNS refuses this when the record is created — after the zone exists, and
    # in a delegated environment while the domain is live. It is the shape a
    # hostname takes when a front door is renamed in one variable and not the
    # other.
    a_records = {
      "portal.example.com" = { address = "203.0.113.11", ttl_seconds = 300 }
    }
  }

  expect_failures = [google_dns_record_set.a]
}

run "a_record_for_the_apex_itself_is_admitted" {
  command = plan

  variables {
    # The other side of the suffix check, and not a detail: `algorik.ai` does
    # not end with `.algorik.ai`, so a precondition written as `endswith` alone
    # would refuse the apex — the one name a domain is most likely to want.
    a_records = {
      "algorik.ai" = { address = "203.0.113.12", ttl_seconds = 300 }
    }
  }

  assert {
    condition     = google_dns_record_set.a["algorik.ai"].name == "algorik.ai."
    error_message = "the apex record was refused; a suffix check that rejects the domain itself refuses the one name a zone most obviously serves"
  }
}

run "a_record_name_carrying_a_wildcard_is_refused" {
  command = plan

  variables {
    # This one applies cleanly if it is let through, which is why it is here
    # rather than left to Cloud DNS. A wildcard answers for every name nobody
    # enumerated, including the ones a future front door will want to own.
    a_records = {
      "*.algorik.ai" = { address = "203.0.113.11", ttl_seconds = 300 }
    }
  }

  expect_failures = [var.a_records]
}

run "an_address_that_is_not_an_ipv4_address_is_refused" {
  command = plan

  variables {
    # A hostname where an address belongs — what somebody writes when they mean
    # a CNAME. Cloud DNS refuses it at apply, after the zone exists.
    a_records = {
      "portal.algorik.ai" = { address = "ghs.googlehosted.com", ttl_seconds = 300 }
    }
  }

  expect_failures = [var.a_records]
}

run "an_address_with_an_octet_out_of_range_is_refused" {
  command = plan

  variables {
    # What a truncated copy-paste produces, and what the usual four-group regex
    # lets straight through.
    a_records = {
      "portal.algorik.ai" = { address = "203.0.113.999", ttl_seconds = 300 }
    }
  }

  expect_failures = [var.a_records]
}

run "an_ipv6_address_in_an_a_record_is_refused" {
  command = plan

  variables {
    # `cidrhost` parses an IPv6 prefix quite happily, so the address check
    # would admit this on its own. An AAAA value in an A record is refused by
    # Cloud DNS at exactly the apply-time moment these validations exist to
    # move earlier.
    a_records = {
      "portal.algorik.ai" = { address = "2001:db8::1", ttl_seconds = 300 }
    }
  }

  expect_failures = [var.a_records]
}

run "a_ttl_below_the_floor_is_refused" {
  command = plan

  variables {
    # Most resolvers floor a TTL this low anyway, so the number stops
    # describing what actually happens — and a record nobody may cache is a
    # query bill for a name three people type.
    a_records = {
      "portal.algorik.ai" = { address = "203.0.113.11", ttl_seconds = 5 }
    }
  }

  expect_failures = [var.a_records]
}

run "a_ttl_above_a_day_is_refused" {
  command = plan

  variables {
    # A week. A wrong record would then be wrong for a week, and the
    # Google-managed certificate waiting on it would stay in PROVISIONING for a
    # week with no way to tell whether the fix took.
    a_records = {
      "portal.algorik.ai" = { address = "203.0.113.11", ttl_seconds = 604800 }
    }
  }

  expect_failures = [var.a_records]
}

run "the_ttls_at_both_ends_of_the_window_are_admitted" {
  command = plan

  variables {
    # Both boundaries, because a window proven only by what it rejects may be
    # a window that rejects everything, and an off-by-one at either end is a
    # refusal nobody can explain from the error message.
    a_records = {
      "argocd.algorik.ai" = { address = "203.0.113.10", ttl_seconds = 60 }
      "portal.algorik.ai" = { address = "203.0.113.11", ttl_seconds = 86400 }
    }
  }

  assert {
    condition     = google_dns_record_set.a["argocd.algorik.ai"].ttl == 60 && google_dns_record_set.a["portal.algorik.ai"].ttl == 86400
    error_message = "a TTL at one end of the admitted window was refused; the window would be narrower than its own error message claims"
  }
}
