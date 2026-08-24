# What Terraform cannot order

This module creates the VPC half of a private circuit. The circuit itself is a
physical cross-connect in a colocation facility, arranged commercially with a
company, and no amount of Terraform brings one into existence. Applying this
with `enable_partner_interconnect = true` and none of the below arranged
produces attachments that sit in `PENDING_PARTNER` for ever.

The order matters, because two of these steps take weeks and one of them
produces a value the next step needs.

## 1. A partner

A service provider that offers Google Cloud Partner Interconnect *and* has a
presence in the facility where the cell's equipment lives. Both halves of that
sentence are constraints: the partner list is per-metro, and a partner who
reaches Chicago is not thereby a partner who reaches the specific cage.

This is a contract with a term and a monthly charge. It is not a cloud
resource and it has no Terraform representation.

## 2. A circuit

Physical connectivity from the cell's cage to the partner's equipment: a
cross-connect ordered from the facility, with a port on each end. Lead times
are measured in weeks, and the facility — not the partner, and not Google — is
who fulfils it.

For the three cells this exists for, that cage is the whole point. `chicago-1`
runs in `us-central1` (Council Bluffs, about 400km from the Chicago venues),
`newyork-1` in `us-east4` (Ashburn, about 300km from NY/NJ) and `dubai-1` in
`me-central1` (Doha, about 380km from Dubai). A circuit that terminates in the
same Google region the cell already runs in buys nothing. The arrangement worth
the money is equipment *in* the venue's metro, reaching the VPC over this
circuit — which is a different deployment from the one this repository
describes, and `docs/operations/deploying-an-edge-cell.md` says so.

## 3. A pairing key, handed over

Once the attachment exists, Google generates a pairing key for it. The
deployment reads it with:

    terraform output -json pairing_keys

and gives it to the partner through whatever channel the partner's ordering
process uses. It is a bearer token in everything but name — whoever holds it
can attach a circuit to this project's VLAN attachment — which is why the
output is marked sensitive and why it should not travel through a CI log or a
ticket anyone can read.

## 4. A VLAN attachment on the partner's side

The partner configures their half against that pairing key. Until they do, the
attachment's `state` is `PENDING_PARTNER`. When they finish it becomes
`ACTIVE`, and only then does anything route — and only if `admin_enabled` is
true, which is deliberately not the default here.

## 5. BGP on the far end

Google creates the Cloud Router interface and the BGP peer itself for a
`PARTNER` attachment, which is why this module creates neither. The equipment
in the cage still has to run its own side of that session, with its own private
ASN, different from `cloud_router_asn`. Two ends claiming the same ASN never
establish a session, and the symptom is an attachment that looks finished and
carries nothing.

## And for the Private Service Connect endpoint: DNS

The endpoint is an address that answers for Google APIs. Nothing reaches it
until something resolves a Google API hostname to that address, and for the
colocated end that resolver is the site's own — a zone for `googleapis.com`
pointing at the endpoint, configured on equipment this project does not manage.

**Do not repoint `*.googleapis.com` inside this VPC.** Every workload here
reaches Google APIs through the restricted range `199.36.153.8/30`, and every
NetworkPolicy in `infrastructure/kubernetes` names that range and only that
range as a permitted egress destination — `an_edge_cell_may_reach_its_venues_and_the_central_plane_and_nothing_else`
in the acceptance suite asserts exactly that. Making in-VPC DNS resolve Google
APIs to this endpoint's address instead would send every workload's traffic to
an address its egress policy denies, and the failure appears as every Google
API call timing out at once, in every pod, with no configuration having changed
in the cluster.

The endpoint exists for the far end of the interconnect, where
`199.36.153.8/30` is not routable without one. That is the arrangement it is
for.
