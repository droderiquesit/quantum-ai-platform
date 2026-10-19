# BILLED plan, 2026-10-19

Read at head `ebf08fa8fae653690bcd11447219616781f027b7`, branch
`ccr-d1bc1e54-kf6mr0`. Input: `retriage-2026-10-18b.json`, category BILLED (21
rows), plus the HSM, Cloud NAT, R12-FABRIC-8101 and R12-DATA-9054 items the
parent named. Planning only: no tfvars, Terraform, workflow or code changed, no
plan run, nothing created. `infrastructure/environments/dev/terraform.tfvars`
is a protected path; the owner applies it.

## Findings that change the shape of the plan

1. **No BILLED row closes from a switch alone.** Each also needs an unbuilt
   sink or adapter, a boot image, a new resource block, a protected workflow
   edit, or a decision; several (CICD-10046, TICK-8110, TICK-9036, MODEL-1085,
   GCP-8036) have no tfvars switch at all. Turning a switch on early buys a "healthy,
   empty, billable instance" (the warning in `variables.tf` above
   `enable_bigquery`). Ranking by rows per dollar therefore favours the
   switches that cost almost nothing.
2. **`kms_protection_level = "HSM"` cannot be applied to applied dev.**
   `version_template` is immutable on a crypto key and every key carries
   `prevent_destroy`, so the plan is a replacement and the apply stops. The
   variable's own comment says "choose the level before an environment's first
   apply"; Terraform cannot gate it. Not proposed.
3. **`enable_cloud_storage` creates an irreversible lock.** The event-archive
   bucket in `modules/data/main.tf` sets `retention_policy.is_locked = true`
   with `archive_retention_days` defaulting to 2557 (seven years), and the
   root does not pass an override. Any object written in dev is undeletable
   for seven years. Turn it on in the change that lands the first writer, not
   before.
4. **Spanner's config string `nam-eur-asea` (modules/data/main.tf, `config`)
   does not appear on Google's Spanner pricing page**, which lists
   `nam-eur-asia1` and `nam-eur-asia3`. It may be a typo that fails at apply.
   Verify before anyone switches Spanner on. ADR 0113 and the traceability
   matrix also use the string.

## Pricing method

List prices fetched from Google's public pages on 2026-10-19. Monthly = hourly
x 730. Storage in GiB-month = hourly GiB rate x 730. Rates are the page's
default region column; where the page selects by region from a dropdown the
text carried one rate, and dev's region (`us-east4`) may differ slightly. Those
are marked "(default column)". Egress, support, taxes, committed-use discounts
and free tiers beyond those named are not modelled.

| Page | URL |
|---|---|
| Spanner | https://cloud.google.com/spanner/pricing |
| Bigtable | https://cloud.google.com/bigtable/pricing |
| Certificate Authority Service | https://cloud.google.com/certificate-authority-service/pricing |
| Cloud NAT | https://cloud.google.com/nat/pricing |
| BigQuery | https://cloud.google.com/bigquery/pricing |
| Cloud KMS | https://cloud.google.com/kms/pricing |
| Cloud Storage | https://cloud.google.com/storage/pricing |
| Artifact Registry | https://cloud.google.com/artifact-registry/pricing |
| Cloud Run | https://cloud.google.com/run/pricing |

Compute Engine pricing (`/compute/all-pricing`) redirected and was not read, so
the fabric broker and ledgerd machines are not priced.

## Summary

| # | Item | Rows | Dev monthly, low to high | Recommendation |
|---|---|---|---|---|
| 1 | `enable_bigquery` | DATA-9054, DATA-10081, DATA-8127; part of GCP-8037, MODEL-10083 | $0.10 to $1.50 | **Turn on now** |
| 2 | `enable_cloud_storage` (incl. R12-FABRIC-8101 archive bucket) | FABRIC-8101; part of DATA-8127, TICK-8110, TICK-9036, MODEL-10083 | $0.30 to $6 | Defer until a writer lands (locked 7-year retention) |
| 3 | `artefact_signing` (HSM signing keys) | SEC-7014, SEC-9127, SEC-9137 | $10 to $11 | Defer (new external-identity trust; needs owner's `bundle_not_after` and trust-bundle commit) |
| 4 | `kms_protection_level = "HSM"` (global) | SEC-10053 | $7 to $14 | **Not proposed** (wedges apply on applied dev) |
| 5 | Cloud NAT | none found | $5 to $6 (exec-node NAT); trust-zones NAT already follows `gitops_enabled` | **Do not turn on** |
| 6 | `enable_bigtable` | DATA-8126, DATA-9184, DATA-10035, DATA-8122, DATA-9053; part of MODEL-10083 | $950 to $1,300 at the autoscaler floor; $9,830 at ceiling | Defer (no Rust adapter; high minimum) |
| 7 | `enable_spanner` | DATA-9185, LEDGER-8007, CAPITAL-9047; part of GCP-8037 | $2,705 to $8,140 | Do not turn on (highest minimum; config string suspect; ledgerd undeployed) |
| 8 | `certificate_authority` (CAS Enterprise) | SEC-10031, SEC-8100, SEC-9128 | $200 to $250 | Defer (no consumer, no leaf-delivery code) |
| 9 | Registry replica | CICD-10046 | $0.50 to $3.10 | Defer (no switch exists; owner decision plus protected `deploy.yml`) |
| 10 | Sharded replay Cloud Run Job | TICK-8110, TICK-9036 | $0 to $15 | Defer (no Terraform for a Job exists) |
| 11 | Fabric brokers / ledgerd machines | FABRIC-10161 | not priced | Not proposed (no boot image; new dependency needs an ADR) |
| 12 | Hosted language model | MODEL-1085 | vendor-billed, unquantified | Not proposed (third-party terms review first) |
| 13 | Per-region instantiation | GCP-8036 | not priced | Not proposed (second region; ADR 0113) |

All 21 BILLED rows appear above: items 1 to 13 name each at least once.

## 1. `enable_bigquery` (R12-DATA-9054 and the warehouse sink rows)

**Rows.** R12-DATA-10081 and R12-DATA-8127 (BILLED): the `qip-mesh`
`BigQueryWarehouse` sink has no caller and runs only against a billed
warehouse. R12-DATA-9054 (`code_small`, not BILLED): resolved outcomes streamed
to the warehouse; its closure text says "enable_bigquery must also be set".
Partial for R12-GCP-8037 and R12-MODEL-10083.

**Switch.** `enable_bigquery`, root `variables.tf`, default `false`. Dev
currently `false` (tfvars line 334). Set `true`. Diff in the combined hunk
below.

**Resources added** (all `count = var.enable_bigquery ? 1 : 0`, in
`module.data`):
- `module.data.google_kms_crypto_key.warehouse[0]` (software key, 90-day
  rotation, `prevent_destroy`)
- `module.data.google_bigquery_dataset.research[0]` (location `var.region`,
  CMEK, no default table expiry)
- `module.data.google_bigquery_dataset_iam_member.writers[...]` for the deep
  brain's service account and `...readers[...]` for the API's and deep brain's
- `module.services` enables `bigquery.googleapis.com` (free)

**Monthly estimate.** KMS software key version $0.000082192/h x 730 = $0.06,
plus one more active version per 90-day rotation ($0.06 each, old versions stay
active). Storage: active logical $0.000031507/GiB-h x 730 = $0.023/GiB-month
(default column), first 10 GiB free; assumed 1 to 50 GiB, so $0 to $0.92.
Queries on demand $6.25/TiB with the first TiB/month free; Storage Write API
gRPC $0.025/GiB with the first 2 TiB/month free. At dev volumes both are in the
free tiers. Range **$0.10 to $1.50**.

**Still needs code.** Yes, R12-DATA-9054: construct `BigQueryWarehouse` in
`qip-deepbrain/src/main.rs` and insert resolved outcomes from
`calibrate_resolved`, with a test. No new dependency is implied by the row text.
R12-DATA-10081 needs the Fabric-driven sink in `qip-fabricd`, which is not
deployed.

**Recommendation.** **Turn on now.** It is the cheapest item, it is reversible
in practice (dataset deletable; the key is not, and costs $0.06/month if left),
and it removes the "no dataset" half of three rows. It closes none alone.

## 2. `enable_cloud_storage` and the R12-FABRIC-8101 archive bucket

**Rows.** R12-FABRIC-8101 (`code_small`): `ArchiveTarget::CloudStorage` exists
behind `QIP_EVENT_FABRIC_ARCHIVE_BUCKET`; `fabric.tf` and
`modules/event-fabric-broker` create no bucket. The data module's buckets are
the destination. Also R12-DATA-8127 (batch loader), R12-TICK-8110 and
R12-TICK-9036 (archived segments to shard over), part of R12-MODEL-10083 and
R12-FABRIC-10161.

**Switch.** `enable_cloud_storage`, default `false`; dev `false` (line 335).
Set `true`.

**Resources added** (`module.data`):
- `google_kms_crypto_key.archive[0]` (`prevent_destroy`)
- `google_storage_bucket.archive[0]` (`qip-dev-event-archive`, `bucket_location`
  default `US`, CMEK, **locked retention, 2557 days**, lifecycle to Coldline
  and Nearline)
- `google_storage_bucket.artifacts[0]`, `google_storage_bucket.segments[0]`
  (regional, 7-day retention, 30-day expiry), `google_storage_bucket.simulation[0]`
  (30-day retention, 90-day expiry)
- `google_storage_bucket_iam_member.archive_writers[...]` and `archive_readers[...]`
  for the deep brain and API service accounts

**Monthly estimate.** KMS $0.06. Standard storage on the page's rows works out
to $0.020 to $0.026/GiB-month (0.000027397 to 0.000035616/GiB-h x 730).
Assumed 10 to 200 GiB across the four buckets: $0.26 to $5.20. Operations
negligible at dev volume. Range **$0.30 to $6**.

**Irreversibility.** The archive bucket's retention lock cannot be shortened or
removed. Dev test objects in it are undeletable for seven years. The cost is
small; the lock is the reason to wait. Passing a short
`archive_retention_days` for dev would need a root change in protected
`main.tf`, which this plan does not make.

**Still needs code.** Yes. FABRIC-8101 wants a landing bucket and transfer inside
`modules/event-fabric-broker` plus a `startup.sh.tftpl` line and a tftest. The
broker is not deployed (`event_fabric_broker = null`).

**Recommendation.** **Defer** until a writer exists and the owner has chosen the
dev retention. Not in the combined diff.

## 3. `artefact_signing` (HSM signing keys, the cheap HSM route)

**Rows.** R12-SEC-7014, R12-SEC-9127, R12-SEC-9137 (OWNER_SWITCH): per-class
`ASYMMETRIC_SIGN` keys render in no committed plan.

**Switch.** `artefact_signing` (object, `artefact-signing.tf`), `null`
everywhere (not present in dev tfvars). Value to set, as an owner edit:
`{ bundle_not_after = "2027-10-19T00:00:00Z" }`. `protection_level` defaults to
and may only be `"HSM"`. `bundle_not_after` is documented as a review horizon a
person chooses, with no default; the date here is a placeholder twelve months out.

**Resources added** (by address): `google_kms_key_ring.artefacts[0]`;
`google_kms_crypto_key.artefact["policy"|"halt"|"envelope"|"model-pack"]`
(EC_SIGN_P256_SHA256, HSM, `prevent_destroy`);
`google_kms_crypto_key_iam_binding.artefact_signer[...]` x4;
`google_service_account.model_pack_publisher[0]`;
`google_iam_workload_identity_pool.model_pack_publish[0]` and
`..._provider.model_pack_publish[0]`;
`google_service_account_iam_member.model_pack_publisher_impersonation[0]`.

**Monthly estimate.** HSM elliptic-curve key version, first 2,000: $0.003424658/h
x 730 = $2.50 each, x4 = $10.00. Signing operations $0.03 to $0.15 per 10,000
(the page's operations row has four columns and I did not resolve which is HSM
asymmetric); at under 10,000 a month, under $0.15. Range **$10 to $11**.

**Still needs.** The committed trust bundle (`artefact_trust_bundle` output,
written to `infrastructure/trust/`) and the owner's D12 for the attestor and
CMEK keys.

**Recommendation.** **Defer, and exclude from the diff.** The cost is trivial
but the items above add a GitHub workload-identity trust and four signer
grants. That is new IAM surface, and I am not proposing IAM changes without the
owner reading `artefact-signing.tf` lines 311 to 400. If the owner wants it, it
is one tfvars line and the table above is the review list.

## 4. `kms_protection_level = "HSM"` (global)

**Rows.** R12-SEC-10053 (OWNER_SWITCH, awaiting D12, which splits the variable
into a fixed-HSM signing level and a per-environment CMEK level).

**Switch.** `kms_protection_level`, default `"SOFTWARE"`, not set in dev tfvars.
It is threaded to seven sites (`main.tf` x5, `audit-logs.tf`, `build-pool.tf`):
secrets, registry, attestor, backups, evidence, asset feed and the build-pool
staging key.

**Resources.** None added. Every existing key is **replaced**
(`version_template` is immutable). Nine keys already exist outside Terraform
state after the 2026-09-13 teardown and cannot be deleted; all carry
`prevent_destroy`.

**Monthly estimate (if it could apply).** HSM symmetric version $0.001369863/h
x 730 = $1.00 against $0.06 software, so about +$0.94 per key, x7 = +$6.6, plus
one extra version per key per 90-day rotation. Range **$7 to $14**.

**Recommendation.** **Not proposed.** The apply would stop on `prevent_destroy`
and leave dev wedged, or, if the guard were removed, would orphan data under the
old key. It is a design decision (D12), not a switch. HSM for signing keys is
covered by item 3, which creates new keys instead of replacing old ones.

## 5. Cloud NAT

**Rows.** None. Searching the retriage JSON for NAT found no row that asks for
one.

**Two NATs exist in the code.**
- `module.trust_zones`: `google_compute_router.egress[0]`,
  `google_compute_address.nat[0]`, `google_compute_router_nat.egress[0]`, created
  when any zone declares `external_egress`. The root generates a GitHub entry
  from `gitops_enabled`, which dev sets `true` (line 168). So it follows an
  existing flag and is **not a billed switch to add**; check the owner's next
  plan for it.
- `module.execution_node[...]`: same three resources behind `create_egress_nat`,
  in the `newyork-1` entry (line 247, currently `false`). The module is empty
  while `execution_nodes_enabled = false`.

**Monthly estimate (execution-node NAT).** Gateway $0.0014/VM-h x 730 x 1 VM =
$1.02; static IP in use $0.005/h x 730 = $3.65; data $0.045/GiB at 0 to 20 GiB
= $0 to $0.90. Range **$5 to $6**.

**Recommendation.** **Do not turn on.** The tfvars comment says the node needs
none: it reaches Google APIs by private access and the central plane over the
VPC, and a NAT gives it an IP-layer internet route nothing asked for. It closes
no row, and the node is not deployable anyway.

## 6. `enable_bigtable`

**Rows.** R12-DATA-8126, R12-DATA-9184 (BILLED), R12-DATA-10035, R12-DATA-8122,
R12-DATA-9053 (adapter rows), part of R12-MODEL-10083.

**Switch.** `enable_bigtable`; dev `false` (line 337). Set `true`.

**Resources added.** `module.data.google_bigtable_instance.timeseries[0]` (two
SSD clusters, `primary` in `us-east4-a` and `secondary` in `us-east4-b`, each
autoscaling 1 to 10 nodes, CPU target 60%); `google_bigtable_app_profile.timeseries[...]`
(three profiles); `bigtableadmin.googleapis.com` enabled.

**Monthly estimate.** Enterprise node $0.65/h (default column) x 730 = $474.50.
Floor is 2 clusters x 1 node = **$949**. SSD storage $0.000232877/GiB-h x 730 =
$0.17/GiB-month, stored in both clusters: 0 to 1,000 GiB = $0 to $340. Range at
the floor **$950 to $1,300**; if both clusters scale to 10 nodes, $9,490 plus
storage, about $9,830. Billed per provisioned node regardless of use, with a one
hour minimum.

**Still needs code.** Yes, all of it: `qip-storage/src/gcp/bigtable.rs` is a
refusing port (no client), and a client crate needs its own ADR under ADR 0109.
The fabric sink R12-DATA-10035 does not exist.

**Recommendation.** **Defer.** A $950 minimum buys an empty instance until the
adapter and ADR exist. A single-cluster dev profile would halve it, but the
module hardcodes two clusters, so that is a code change, not a switch.

## 7. `enable_spanner`

**Rows.** R12-DATA-9185, R12-LEDGER-8007, R12-CAPITAL-9047; part of R12-GCP-8037.

**Switch.** `enable_spanner`; dev `false` (line 339). Set `true`. Also reads
`spanner_min_processing_units` (default 1000) and `..._max_...` (default 3000).

**Resources added.** `module.data.google_kms_crypto_key.records[0]` (30-day
rotation, `prevent_destroy`), `google_spanner_instance.global[0]` (ENTERPRISE_PLUS,
config `nam-eur-asea`, autoscaling 1,000 to 3,000 processing units),
`google_spanner_database.positions[0]` (CMEK), `spanner.googleapis.com`.

**Monthly estimate.** The page's multi-region Enterprise Plus rate (default
column, the first listed config) is $3.705/node-h. Floor 1 node x 730 =
**$2,705**; ceiling 3 nodes = $8,114. Storage, all replicas, SSD
$0.000712329/GiB-h x 730 = $0.52/GiB-month; assume 10 to 50 GiB = $5 to $26. A
dual-region price on the same page is $4.617/node-h; a single-region
Enterprise Plus node (Northern Virginia column) shows $1.71/h = $1,248. Range
**$2,705 to $8,140**. A one-hour minimum applies to any capacity change.

**Still needs.** A deployed `qip-ledgerd` (no boot image exists), the
positions/obligations schema, and a verified config name (see Finding 4).

**Recommendation.** **Do not turn on.** It is the highest minimum on the page,
the row cannot close without an undeployed binary, and its own variable
description says it is "the last to enable, not the first".

## 8. `certificate_authority` (CAS Enterprise)

**Rows.** R12-SEC-10031, R12-SEC-8100, R12-SEC-9128.

**Switch.** `certificate_authority` object, `null` in dev (absent from tfvars).
Value: `{ location = "us-east4", organization = "<owner-chosen>" }`; the
organisation string is the owner's.

**Resources added.** `google_privateca_ca_pool.internal[0]` (tier ENTERPRISE),
`google_privateca_certificate_authority.internal[0]` (self-signed root, EC
P-256, 10-year life), `google_privateca_certificate_template.workload[0]`, and
`google_privateca_ca_pool_iam_member.requesters[...]` for each of fabricd,
ledgerd and execution nodes. With none deployed, the requester set is empty.

**Monthly estimate.** Enterprise $200 per CA per month. Certificates $0.50 each
for the first 50,000. Assumed 3 binaries x 30 daily certificates = 90 = $45.
Range **$200 to $250** (no issuance to a restart-heavy month). The module fixes
the tier to ENTERPRISE; the $20 DevOps tier is not selectable by a switch.

**Still needs.** Leaf delivery in `startup.sh.tftpl` and in fabricd and ledgerd
`main.rs`, and the API-8113 IAM evidence. All three consumers are undeployed.

**Recommendation.** **Defer.** $200 a month for a root CA that nothing can
request from.

## 9. Registry replica (R12-CICD-10046)

**Rows.** R12-CICD-10046.

**Switch.** None. `modules/registry` has no replica or second-region variable.

**Resources.** Would be a second `google_artifact_registry_repository` in
another region, its CMEK key and a copy step in protected `deploy.yml`.

**Monthly estimate.** Storage $0.000136986/GiB-h x 730 = $0.10/GiB-month, first
0.5 GiB free; assumed 5 to 30 GiB = $0.45 to $2.95; cross-region copy
$0.01/GiB for 1 to 15 GiB = $0.01 to $0.15. Range **$0.50 to $3.10**.

**Recommendation.** **Defer.** Cheap, but it needs module code, a protected
workflow edit and the owner's replica-versus-multi-region decision (multi-region
replaces the signed-image repository).

## 10. Sharded replay Cloud Run Job (R12-TICK-8110, R12-TICK-9036)

**Rows.** R12-TICK-8110, R12-TICK-9036.

**Switch.** None; `grep` finds no `google_cloud_run_v2_job` in the tree.

**Monthly estimate.** CPU $0.000018/vCPU-s and memory $0.000002/GiB-s (default
column) with 240,000 vCPU-s and 450,000 GiB-s free. At 8 shards x 2 vCPU x 4 GiB
x 1 h x 4 runs a month that is 230,400 vCPU-s and 460,800 GiB-s, so about
$0.02. At 16 runs: about $12.30 plus $2.80 = $15. Range **$0 to $15**. Needs the
segments bucket from item 2.

**Recommendation.** **Defer.** A Job module and `session_replay.rs` sharding
come first, and archived segments need item 2.

## 11 to 13. Not proposed

- **Fabric brokers and ledgerd (R12-FABRIC-10161).** No boot image exists, the
  OTel exporter needs a dependency ADR, and the machine prices were not read.
  Three brokers at RF3 is the largest unquantified item.
- **Hosted language model (R12-MODEL-1085).** Spend is set by
  `QIP_LANGUAGE_MODEL_*` variables and billed by a third-party provider per
  token, not by a Terraform switch. `variables.tf` requires the provider's
  terms to be read before an environment turns it on. No cap exists to bound it.
- **Per-region instantiation (R12-GCP-8036).** A second region is the point at
  which ADR 0113 splits the organisation into projects. Beyond a tfvars edit
  and out of scope here.

## Totals

| Set | Items | Monthly low | Monthly high |
|---|---|---|---|
| **Recommended** | 1 | **$0.10** | **$1.50** |
| Full priced set | 1 to 10 (item 4 and item 3 not netted) | about $3,880 | about $18,280 |

The full set is the sum of items 1 to 10 at the stated ranges, using autoscaler
ceilings for Bigtable and Spanner for the high end. It cannot be applied as
written (item 4 wedges apply; item 7's config string is suspect) and is shown
only as the upper bound of what "turn everything on" would cost.

## Combined diff for dev (recommended set only)

Apply from the repository root. Checked with `git apply --check` against head
`ebf08fa8fae653690bcd11447219616781f027b7`: exit 0, no output.

```diff
--- a/infrastructure/environments/dev/terraform.tfvars
+++ b/infrastructure/environments/dev/terraform.tfvars
@@ -331,7 +331,7 @@
 
 # Every managed service off. Development runs on memory, which is what the
 # implemented storage targets are for on an instance with no volume.
-enable_bigquery      = false
+enable_bigquery      = true
 enable_cloud_storage = false
 enable_alloydb       = false
 enable_bigtable      = false
```

The comment above the hunk ("Every managed service off") becomes untrue once it
applies. The owner may want it reworded in the same edit.

## Boundary statement

Nothing here touches live trading, real capital, prod, test, stage or any IAM
grant. The only IAM an applied hunk creates is the module's existing
dataset-scoped `writers` and `readers` for the deep brain and API service
accounts. The paper-trading layers (Terraform ceiling refusal, composition-root
refusal, type system) are untouched.
