output "nameservers" {
  description = <<-EOT
    **The one manual step left, and it is performed once rather than once per
    record.**

    Sign in to the registrar that holds the domain — Namecheap, which is why it
    answers from dns1.registrar-servers.com today — find the custom-DNS setting
    for it, and replace every nameserver there with the four names in this
    list. Nothing else at the registrar changes: not the ownership, not the
    contacts, not the renewal.

    From the moment that propagates, every name under the domain is answered
    out of this Terraform state, and a new front door becomes an A record in a
    commit rather than a form somebody fills in. Until it propagates, this zone
    is authoritative for a domain nobody is asking it about — a harmless state,
    and an easy one to mistake for a broken one, because `terraform apply` will
    have reported complete success either way.

    How to tell it worked, without waiting on a browser: `dig +short NS
    <domain>` returns these four names, and `dig +short A <a front door>
    @<the first of them>` answers with the address the record names. The
    Google-managed certificates leave PROVISIONING within about fifteen minutes
    of that, and the doors serve.
  EOT

  value = google_dns_managed_zone.zone.name_servers
}

output "domain" {
  description = "The domain this zone is authoritative for, without a trailing dot, for an operator to read back and for a test to hold the wiring to."
  value       = var.domain
}

output "zone_name" {
  description = "The managed zone's resource name, for `gcloud dns record-sets list --zone` and for a reviewer matching a console page to a state file."
  value       = google_dns_managed_zone.zone.name
}

output "records" {
  description = <<-EOT
    Every A record this zone serves, as name to address, so that `terraform
    output` answers "what does this domain resolve to" without a console page
    and without a `dig` against a resolver that may be caching.

    The addresses are unknown until apply, because each is a reserved global
    address the plan has not created yet. That is the honest state of this
    output at plan time and not a defect in it.
  EOT

  value = { for name, record in google_dns_record_set.a : name => one(record.rrdatas) }
}

output "dnssec_enabled" {
  description = "Whether Cloud DNS signs this zone. True, and the argument is in `variables.tf`: a signed zone with no DS at the registrar resolves exactly as an unsigned one does, so this costs nothing until the owner chooses to finish it."
  value       = var.dnssec_enabled
}

output "ds_record" {
  description = <<-EOT
    The DS record to publish at the registrar **if and when the owner decides
    to turn DNSSEC validation on**. Null while `dnssec_enabled` is false.

    This is not part of the delegation step above and must not be done at the
    same time. Replacing the nameservers is reversible in minutes; publishing a
    DS is not. Once it is at the registrar, every validating resolver refuses
    to answer for the domain at all — mail, the landing site, everything, not
    just the names in this zone — unless this exact zone is serving with these
    exact keys. `infra.yml down` destroys this zone, so tearing a dev
    environment down would take the whole domain dark, and the only place to
    fix that is the registrar, which is the one place nothing in this
    repository can act.

    Publish it when the environment has stopped being something that gets torn
    down, and remove it at the registrar *before* any teardown after that.
  EOT

  value = var.dnssec_enabled ? try(data.google_dns_keys.zone[0].key_signing_keys[0].ds_record, null) : null
}
