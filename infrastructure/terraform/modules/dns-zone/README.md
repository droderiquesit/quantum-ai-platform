# `modules/dns-zone` — the domain, owned in code

One public `google_dns_managed_zone`, and one A record per front door that
exists, each pointing at the address of the module that reserved it.

## The failure it prevents

Three front doors were applied before this module existed and none of them
resolved. `argocd.algorik.ai` and `kargo.algorik.ai` are served by
`modules/gitops-gateway`'s reserved global address; `portal.algorik.ai` by
`modules/iap-edge`'s (ADR 0094). Each module's `address` output ends with a
sentence explaining that somebody has to create the A record by hand, because
`algorik.ai` answers from `dns1.registrar-servers.com`. The Google-managed
certificates sit in `PROVISIONING` until the names resolve, so all three doors
were applied, reserved, certificated, and serving nothing.

The point is not that typing is slow. A hand-made record is a **second claim
about an address this repository already knows**. Two claims about one fact
agree on the day they are written and disagree the first time an address is
released and re-reserved — and the disagreement is silent: the record still
resolves, the certificate still serves, and the name points at whatever Google
handed the next tenant. Here the record *is* the module output. There is no
second copy to drift, and `the_dns_records_take_their_addresses_from_the_modules_that_reserved_them`
in the infrastructure acceptance suite refuses a pasted literal in the root's
wiring.

## What is still manual, and it is once

Replacing the nameservers at the registrar with the four in the `nameservers`
output. That is a delegation, not a record: it happens once in the domain's
life and never again, and after it every new name is a commit.

Read the `nameservers` output's description before doing it; it says how to
tell it worked without waiting on a browser.

## DNSSEC

**Signing is on. The DS record at the registrar is deliberately not part of the
delegation step**, and `variables.tf` and the `ds_record` output carry the
argument. In short: signing is free, invisible to resolvers until a parent DS
exists, and awkward to turn on later against a live delegation. Publishing the
DS is the irreversible half — it makes the *whole domain's* resolution depend
on this zone continuing to exist with these keys, and `infra.yml down` destroys
this zone.

## The singleton problem

A domain has one authoritative zone. Two environments creating one each would
both apply cleanly, report success, and serve two different sets of records
from two different sets of nameservers — and only whichever set the registrar
happens to name would be the one anybody sees. The other would be a state file
full of records nobody resolves, which is the same shape of defect as a Cloud
Armor policy attached to no backend.

Terraform cannot catch this. Each environment has its own state and no plan can
see another's. So the guard is in two places that can:

- `var.dns_zone_domain` is `""` by default and the root's `count` is on it, so
  an environment that does not name the domain creates nothing at all — not an
  empty zone, not a placeholder.
- `a_single_environment_owns_the_dns_zone_for_the_domain` in the infrastructure
  acceptance suite reads all four `terraform.tfvars` and fails if more than one
  names a domain, or if two name the same one. The second declaration has to be
  committed to exist, and that is where it is caught.

## Inputs, briefly

| Variable | Why it is shaped that way |
|---|---|
| `domain` | Without a trailing dot; the module adds it. A supplied dot makes `algorik.ai..`, a zone nothing resolves. |
| `a_records` | `name → { address, ttl_seconds }`. No TTL default: a TTL is a promise about how long a mistake lasts, and the caller states it beside the record with the reason. |
| `dnssec_enabled` | Defaults true. See above. |

## Evidence

`tests/dns-zone.tftest.hcl`, `mock_provider`, `command = plan` throughout. Every
refusal is paired with an admission of the same shape, because a validation
proven only to refuse may refuse everything — and a zone module that refuses
everything is a domain that never gets delegated, which reads from a one-sided
harness exactly like a strict gate.
