# The domain, owned in code.
#
# Three front doors were built before this module existed and none of them
# resolved. `argocd.algorik.ai` and `kargo.algorik.ai` answer on
# `modules/gitops-gateway`'s reserved global address, `portal.algorik.ai` on
# `modules/iap-edge`'s (ADR 0094), and every one of them waited on an A record
# somebody typed into a registrar's web form. Each module's `address` output
# says so in its own description. The Google-managed certificates stay in
# `PROVISIONING` until the names resolve, so the practical state of all three
# doors was: applied, reserved, certificated, and serving nothing.
#
# The failure this module prevents is not "typing is slow". It is that a
# hand-made record is a second, unversioned claim about an address this
# repository already knows. The two agree on the day they are written and
# disagree the first time an address is released — at which point the record
# keeps resolving, the certificate keeps serving, and the name points at
# whatever Google handed the next tenant. Here the record *is* the module
# output; there is no second copy to drift.
#
# What is left over is one act, once, and it is a delegation rather than a
# record: replacing the nameservers at the registrar with the four in
# `nameservers`. See that output.

# --- The zone ----------------------------------------------------------------
#
# `visibility = "public"` is written rather than left to the provider default,
# and the reason is that the wrong value here is invisible. A private zone
# applies cleanly, shows in `terraform show` as a managed zone for the domain,
# answers correctly from inside the VPC, and leaves every name on the internet
# exactly as dark as it was before — which reads, to anyone reviewing a plan,
# like success. `modules/network` creates the private one this platform also
# has; two zones in one project with opposite visibility is precisely the pair
# a default would blur.
#
# `force_destroy` is left at its default of false on purpose. Terraform
# destroys the records it manages before the zone, so the teardown this
# repository performs works without it; what it refuses is destroying a zone
# somebody added a record to by hand. That refusal is the right one — an
# unmanaged record in here is a fact nobody wrote down, and losing it silently
# during a `down` is how a domain ends up missing a name nobody can name.
resource "google_dns_managed_zone" "zone" {
  project     = var.project_id
  name        = "qip-${var.environment}-${replace(var.domain, ".", "-")}"
  dns_name    = "${var.domain}."
  description = "Authoritative public zone for ${var.domain}, owned by this repository. Every A record under it is derived from the address of the module that reserved it; nothing here is hand-made."
  visibility  = "public"

  labels = var.labels

  # Signed, always, and the DS record deliberately left to the owner. The
  # argument is in `variables.tf` under `dnssec_enabled` and the operational
  # half is in the `ds_record` output: signing is free and reversible,
  # publishing the DS is neither.
  dnssec_config {
    state = var.dnssec_enabled ? "on" : "off"
  }
}

# --- The records -------------------------------------------------------------
#
# One per front door that exists, keyed by the name. The address is whatever
# the module that reserved it returned; the caller cannot pass a literal
# without the infrastructure suite noticing.
resource "google_dns_record_set" "a" {
  for_each = var.a_records

  project      = var.project_id
  managed_zone = google_dns_managed_zone.zone.name
  name         = "${each.key}."
  type         = "A"
  ttl          = each.value.ttl_seconds
  rrdatas      = [each.value.address]

  lifecycle {
    # A record must be inside the zone that serves it.
    #
    # Cloud DNS refuses `portal.example.com` in the `algorik.ai` zone, and it
    # refuses it at apply — after the zone has been created and, in an
    # environment where the delegation has already been made, while the domain
    # is live. The plan that produced it looked entirely reasonable: the name
    # is well formed, the address is real, and nothing but the suffix is
    # wrong. That is the shape a hostname takes when a front door is renamed
    # in one variable and not the other.
    #
    # A precondition rather than a variable validation because the fact is a
    # relationship between two variables, and this repository's convention —
    # see `terraform_data.portal_edge_has_a_console` in the root — is that
    # such a fact is a precondition.
    precondition {
      condition     = each.key == var.domain || endswith(each.key, ".${var.domain}")
      error_message = "The A record ${each.key} is not inside ${var.domain}, so this zone is not authoritative for it and Cloud DNS refuses it when the record is created — after the zone exists. Either the record belongs in another zone or the front door's hostname and this zone's domain have come apart."
    }
  }
}

# --- What the registrar needs for DNSSEC -------------------------------------
#
# Read rather than constructed: the DS record depends on the key-signing key
# Cloud DNS generates when the zone is created, and there is no way to know it
# before the apply. Guarded on `dnssec_enabled` so that turning signing off
# does not leave a data source reading keys that do not exist.
data "google_dns_keys" "zone" {
  count = var.dnssec_enabled ? 1 : 0

  project      = var.project_id
  managed_zone = google_dns_managed_zone.zone.id
}
