# Owner decisions, 2026-10-11 — the R12 rows that waited on a decision rather than code

The owner delegated these decisions in the session: "You make the
recommended decision and have full permission". Each decision below is the
recommended call for a platform that is strictly paper trading today and is
meant to become real-money and global-scale. Every one is written into an ADR
(new or amended) so it can be found later; this file is the index from the
register to those records, and the coordinator's input for re-scoring.

**Hard limits held throughout.** No decision creates, enables or eases a
live-order or real-money path. The three paper-trading layers (Terraform's
refusal of the live rungs, `AutonomyLevel::deployable`, `Cell::new` with
`Determinism::Required`) and the ledger's refusal of `simulated=false` (ADR
0106) are unchanged. ADR 0108 stays *proposed and unaccepted*, and every row
that needs live capital stays BLOCKED by it. External-action surfaces refused
by ADR 0114 D8 stay refused; only drafting for a person is approved.

## Scope: which rows were examined

- **(a)** Every row with `"status": "BLOCKED"` in
  `docs/blueprint/assessment/R12-*.json` — **55** rows (G1 13, G2 19, G3 4,
  M1 1, M4 2, M5 3, M6 7, M7 6).
- **(b)** PARTIAL/MISSING rows whose `gap`, `blocked_by` or `notes` matched a
  case-insensitive search for `owner|human|approval|person|decid`, kept only
  where the match is a decision the row waits on — **21** rows. Matches that
  were incidental (a field named "owner", "decision record", PAM for human
  grants, a row that only needs a deployment such as R12-GCP-7005, and
  R12-RISK-4104, whose "approval" is part of the requirement and whose gap is
  a test) were excluded.

**Result: 76 rows. 45 become buildable (24 of the 55 BLOCKED, all 21 of the
PARTIAL/MISSING); 31 stay BLOCKED, each now by a named, current decision.**

## Decisions

| # | Decision | Recommendation | ADR | Rows | What each row becomes |
|---|---|---|---|---|---|
| D1 | Fabric consensus, metadata quorum and broker shape (C2 for the fabric, the `openraft` fallback) | Keep Spanner as the consensus; refuse the "no cloud database" clause; put stream definitions, placement, broker epochs and ACLs in the same regional database as the lease; rendezvous picks who contends first, the lease decides; three C4/C4D brokers per production region, one per zone; deploy fabricd in `dev` in ADR 0035's region once billing is on | 0111 Amendment 1 | FABRIC-7037, 8085, 8096, 7035, 9118, 9182; RES-9168, 9169; OBS-9146 | UNBLOCKED-TO-BUILD (all nine) |
| D2 | Cross-region mirror (C8's mirror half) | In-tree asynchronous replicating consumer in the target region; region-qualified streams; idempotent by source position; lag series and stale state; replay from checkpoint or archive; no write waits on another region; build and game-day on two local brokers now, deploy at the second region | **0119** (new) | FABRIC-7038, RES-7114, OBS-9148 | UNBLOCKED-TO-BUILD (deploy waits on a second region) |
| D3 | Transport identity: internal LB + mTLS vs IAM invocation; QUIC; prost | IAM-authenticated invocation with internal-only ingress satisfies API-8113 and the warm-service half of SEC-10031; the internal ALB and a service mesh are refused; QUIC only on a committed measurement; prost stays unadmitted | 0112 Amendment 1 | API-8113, SEC-10031, FABRIC-7046, FABRIC-8083; FABRIC-7048 | 8113/10031/7046/8083 UNBLOCKED-TO-BUILD; 7048 STAYS-BLOCKED (ADR 0109: no external protobuf wire) |
| D4 | GKE-placed rows (ADR 0024 / ADR 0114 D4) | Mechanism rows stay refused; capability rows are built on Cloud Run, Cloud Run Jobs or Compute Engine and scored against that equivalent | 0114 Amendment 1 (D4) | Build: GCP-8104, 8105, 8106, EVID-8108, FABRIC-8109, TICK-8110, REASON-8111, GCP-8116, CICD-9116. Stay: GCP-8005, 8115, 9183, 10030, 7006, 9015, CICD-9113, MODEL-10065 | 9 UNBLOCKED-TO-BUILD; 8 STAYS-BLOCKED (ADR 0114 D4; 10065 also on no measured accelerator need) |
| D5 | External-action surfaces (ADR 0114 D8) | Acting externally stays refused; one drafting adapter whose only effect is an internal outbox is approved; the guard's quality gaps are buildable now | 0114 Amendment 1 (D8) | Build: AGENCY-1006, 1025, 1056, 1057, 1059, 1060, GOV-6124. Stay: AGENCY-6092, 6093, 6095, 6096, 9062 | 7 UNBLOCKED-TO-BUILD; 5 STAYS-BLOCKED (ADR 0114 D8) |
| D6 | Live capital, venue sessions, money movement | No change. ADR 0108 stays proposed; ADR 0003 and ADR 0021 hold | 0108 (unchanged), 0003, 0021 | EXEC-8068, GOV-10149, CAPITAL-4076, 4081, LEDGER-5023, 5024, 5027, AGENCY-6097, 6098, GOV-6119 | STAYS-BLOCKED (all ten) |
| D7 | Per-person operator presence and the two-person rule (ADR 0075 Gate A, ADR 0076's open questions) | Enrolled WebAuthn P-256 public keys baked into the attested image; `ring`-verified assertion over a request-bound, single-use, 120-second challenge yields `Presence::Attested`; console stays `viewer` and relays; two-person rule kept; no order seam, no live level | **0120** (new); 0076 amendment | RISK-4111, 4112, 4114, 4115, GOV-6125 | UNBLOCKED-TO-BUILD |
| D8 | Seven repositories (C6) | Declined again: one reviewed commit across contract, code, Terraform and boundary tests; ownership by directory and CODEOWNERS | 0016 amendment | CICD-9088, 9089, 9090, 9091, 9092, 9093, 9094 | STAYS-BLOCKED (ADR 0016) |
| D9 | Cloud Run metric collection while the GMP sidecar fails the scan gate | Stop waiting upstream; push the registry snapshot to Cloud Monitoring over the REST adapter and `rustls`; re-point the nine policies; flip `workload_metrics_exist` only after a read-back | **0121** (new) | OBS-9143 | UNBLOCKED-TO-BUILD |
| D10 | Non-Rust research (C5) | Offline Python lane on Vertex AI custom jobs, no route to production, output only as a signed model pack the in-tree provider serves; accelerators only here | **0122** (new) | MODEL-1082 (and placement for MODEL-10065, 9015's burst half) | UNBLOCKED-TO-BUILD |
| D11 | Legacy unbounded capital envelopes | The cell admits only through `admit_bounded`; an envelope without bounds is refused | 0008 amendment | GOV-6120 | UNBLOCKED-TO-BUILD |
| D12 | HSM scope and KMS step 3 | Every signing key HSM in every environment; CMEK HSM in stage/prod; KMS keys and publish workflow written and applied to `dev` with the plan shown | 0110 Amendment 1 | SEC-10053, SEC-9109 | UNBLOCKED-TO-BUILD |
| D13 | Tick-lake codec default | `zstd` by default, set explicitly in the node template, with the measurement committed in the same change; void if spool pressure rises | 0113 Amendment 1 | TICK-2057 | UNBLOCKED-TO-BUILD |
| D14 | Build private pool apply; portal reproducibility exemption | Apply approved for `dev`, then the tfvars flip; close the portal exemption by a commit-derived build id | 0115 Amendment 1 | CICD-9097 | UNBLOCKED-TO-BUILD |
| D15 | Agent sandbox placement | Fleet stays in `dev` until ADR 0113's split; egress to Google APIs only; read-only telemetry; synthetic data enforced | 0102 amendment | SEC-9077 | UNBLOCKED-TO-BUILD |

**Owner inputs that remain and are not architecture decisions.** A cost
ceiling in money (ADR 0101 C11) and enabling billing on `dev` are facts about
the owner's budget and account; no figure was invented here. Every apply named
above still shows its plan first, under the infrastructure rules. The
`.claude/rules/10-product-direction.md` sentence naming the browser as "the
only exception" to Rust needs the owner's edit to name ADR 0122's research
lane, and `CLAUDE.md`'s infrastructure row is still the edit ADR 0114 named.

## Top work items the unblocked rows now need

1. **ADR 0121 operator presence** — challenge endpoint, WebAuthn verifier
   with `ring`, `Presence::Attested`, enrolment files in the image, portal
   `navigator.credentials` flow; then ADR 0075 Gates B and C, which unblock
   RISK-4111/4112/4114/4115 and GOV-6125. Highest leverage: it is the only
   path by which any grant is issuable in a deployed binary.
2. **Fabric on Spanner, for real** — Terraform for the regional fabric
   database, emulator-proven adapter, metadata tables under lease epochs,
   rendezvous-preferred contention, the broker module opened to three zones
   on C4/C4D, Spanner commit-latency series (FABRIC-7035/7037/8085/8096/9118/
   9182, RES-9168/9169, OBS-9146).
3. **Transport evidence** — invoker manifests applied and an anonymous call
   refused (API-8113); CA Service workload certificates on every Compute
   Engine hop (SEC-10031); the TCP+TLS vs QUIC measurement on the peer mesh
   (FABRIC-7046/8083).
4. **Metrics push** (OBS-9143) — the exporter, re-pointed alert policies, a
   read-back in `dev`, then `workload_metrics_exist`.
5. **Cloud Run equivalents** — Cloud Run Jobs for replay shards (TICK-8110),
   a lag-scaled sink MIG (FABRIC-8109 after FABRIC-8080), traffic-split
   canary with SLO gate and digest rollback (CICD-9116), per-service
   instance/concurrency budgets (GCP-8116).
6. **Fabric mirror** (FABRIC-7038, RES-7114, OBS-9148) — policy arm, mirror
   role, lag series, stale state, two-broker game day.
7. **Capital and keys** — `admit_bounded` in the cell (GOV-6120); HSM by
   purpose and the KMS publish workflow in `dev` (SEC-10053, SEC-9109).
8. **Agency guard quality and the drafting adapter** (AGENCY-1006/1025/1056/
   1057/1059/1060, GOV-6124).

## Rows outside this scope with a stale `blocked_by` (for the coordinator)

Not re-scored here; listed because the decisions above make their
`blocked_by` text false. R12-MESH-8003 and R12-REFLEX-8065 cite ADR 0002/0009
(superseded by ADR 0109); R12-GCP-8055, R12-FABRIC-8081 cite "mTLS blocked"
(ADR 0112 decided it); R12-FABRIC-8075, R12-FABRIC-8077 cite a Raft crate and
prost under ADR 0002/0009 (ADR 0111 Amendment 1 and ADR 0109 decide them).
R12-AGENCY-1058, 1061 and 1062 are COMPLETE but still carry "External action
surfaces need governance approval"; ADR 0114 Amendment 1 answers it and their
`blocked_by` should be emptied.

## Re-scoring input for the coordinator

`status` uses the register's vocabulary. A row that becomes buildable leaves
BLOCKED for PARTIAL where something it needs already exists, MISSING where
nothing does, and its `blocked_by` is emptied; a row that stays blocked keeps
BLOCKED and its `blocked_by` names the current decision.

```json
{
  "R12-GCP-8005": {"status": "BLOCKED", "gap": "The requirement is the GKE Standard service plane itself, which ADR 0114 D4 refuses. Its capabilities are built on Cloud Run per ADR 0114 Amendment 1: evidence (EVID-8108), replay (TICK-8110), sinks (FABRIC-8109).", "blocked_by": "ADR 0114 D4 (no workload on Kubernetes), confirmed by ADR 0114 Amendment 1, 2026-10-11"},
  "R12-EXEC-8068": {"status": "BLOCKED", "gap": "A provider-sandbox venue adapter is ADR 0108 Stage 1, and ADR 0108 is not accepted. Safe pre-work that opens nothing: ADR 0108 P2's endpoint-derived venue class (any host off a committed sandbox allow-list is Live and refused).", "blocked_by": "ADR 0108 (proposed, not accepted; Stage 1 not authorised); ADR 0003"},
  "R12-FABRIC-8083": {"status": "PARTIAL", "gap": "TLS behind the FabricTransport seam exists (mTLS, FABRIC-7046). QUIC is admitted only on a committed TCP+TLS vs QUIC measurement on the cross-region peer-mesh hop (ADR 0112 §4, Amendment 1); commit that measurement and an integration test refusing plaintext.", "blocked_by": ""},
  "R12-GCP-8104": {"status": "PARTIAL", "gap": "Runs as a Cloud Run warm service (ADR 0114 Amendment 1). Needs the ledger on Spanner (ADR 0113 §1, LEDGER-8007) and declared Cloud Run autoscaling bounds; the HPA/custom-metric clause is refused by ADR 0114 D4.", "blocked_by": ""},
  "R12-GCP-8105": {"status": "PARTIAL", "gap": "Placement on Cloud Run satisfies the requirement (ADR 0114 Amendment 1). Needs the authoritative store per LEDGER-8007 (ADR 0113 §1) and an inspection test that authoritative capital state is written only through qip-ledgerd.", "blocked_by": ""},
  "R12-GCP-8106": {"status": "PARTIAL", "gap": "Placement on the Cloud Run brains satisfies the requirement (ADR 0114 Amendment 1). Needs a test that stress scenarios and envelopes are produced by the deployed composition, not only in kernel tests.", "blocked_by": ""},
  "R12-EVID-8108": {"status": "PARTIAL", "gap": "Continuous scoring as a scheduled stage of qip-deepbrain on Cloud Run; derived state reproducible from the event log and snapshotted to the archive, no Spanner Graph/BigQuery (ADR 0113, ADR 0114 Amendment 1).", "blocked_by": ""},
  "R12-FABRIC-8109": {"status": "MISSING", "gap": "Sinks (FABRIC-8080) first; then a dedicated Compute Engine managed instance group per destination, autoscaled on the consumer-lag series, with per-destination failure isolation (ADR 0114 Amendment 1).", "blocked_by": ""},
  "R12-TICK-8110": {"status": "PARTIAL", "gap": "Replay and counterfactual scoring exist in-process. Needs a Cloud Run Job launching replay shards over archived segments and comparing live vs counterfactual models, declared in Terraform (ADR 0114 Amendment 1).", "blocked_by": ""},
  "R12-REASON-8111": {"status": "PARTIAL", "gap": "Custom Rust solvers on the deep brain's Cloud Run capacity satisfy the requirement's 'custom Rust' arm (ADR 0114 Amendment 1); no-hot-path is structural. Needs the deep brain's CPU allocation for solver work declared and an inspection test. Z3/OR-Tools are not admitted.", "blocked_by": ""},
  "R12-API-8113": {"status": "PARTIAL", "gap": "IAM-authenticated invocation with internal-only ingress is the accepted mechanism (ADR 0112 Amendment 1; internal ALB refused). Needs the invoker manifests applied and evidence that a VPC call with no ID token, and one from an ungranted principal, are refused.", "blocked_by": ""},
  "R12-GCP-8115": {"status": "BLOCKED", "gap": "GKE Standard regional clusters are the requirement; refused. Cloud Run is regional for warm services.", "blocked_by": "ADR 0114 D4, confirmed by ADR 0114 Amendment 1, 2026-10-11"},
  "R12-GCP-8116": {"status": "MISSING", "gap": "Built as the Cloud Run equivalent (ADR 0114 Amendment 1): every catalogue service declares maximum instances and concurrency, research runs in separately budgeted Cloud Run Jobs, and a test refuses a service without a maximum. Node-pool taints are refused.", "blocked_by": ""},
  "R12-GCP-9015": {"status": "BLOCKED", "gap": "Persistent cognitive services on GKE are refused; they run on Cloud Run. The burst half is ADR 0122's research lane on Vertex AI custom jobs.", "blocked_by": "ADR 0114 D4, confirmed by ADR 0114 Amendment 1, 2026-10-11"},
  "R12-AGENCY-9062": {"status": "BLOCKED", "gap": "Adapter services per external action class are refused; the only approved adapter is the internal drafting adapter (ADR 0114 Amendment 1).", "blocked_by": "ADR 0114 D8 (external action refused), confirmed 2026-10-11"},
  "R12-CICD-9088": {"status": "BLOCKED", "gap": "Declined: one repository, ownership by directory and CODEOWNERS.", "blocked_by": "ADR 0016 (amendment 2026-10-11) and ADR 0101 C6, declined"},
  "R12-CICD-9089": {"status": "BLOCKED", "gap": "Declined: one repository, ownership by directory and CODEOWNERS.", "blocked_by": "ADR 0016 (amendment 2026-10-11) and ADR 0101 C6, declined"},
  "R12-CICD-9090": {"status": "BLOCKED", "gap": "Declined: one repository, ownership by directory and CODEOWNERS.", "blocked_by": "ADR 0016 (amendment 2026-10-11) and ADR 0101 C6, declined"},
  "R12-CICD-9091": {"status": "BLOCKED", "gap": "Declined: one repository, ownership by directory and CODEOWNERS.", "blocked_by": "ADR 0016 (amendment 2026-10-11) and ADR 0101 C6, declined"},
  "R12-CICD-9092": {"status": "BLOCKED", "gap": "Declined: one repository, ownership by directory and CODEOWNERS.", "blocked_by": "ADR 0016 (amendment 2026-10-11) and ADR 0101 C6, declined"},
  "R12-CICD-9093": {"status": "BLOCKED", "gap": "Declined: one repository, ownership by directory and CODEOWNERS.", "blocked_by": "ADR 0016 (amendment 2026-10-11) and ADR 0101 C6, declined"},
  "R12-CICD-9094": {"status": "BLOCKED", "gap": "Declined: one repository, ownership by directory and CODEOWNERS.", "blocked_by": "ADR 0016 (amendment 2026-10-11) and ADR 0101 C6, declined"},
  "R12-CICD-9113": {"status": "BLOCKED", "gap": "Argo Rollouts for GKE services is the requirement; there are no GKE services. The canary capability is CICD-9116 on Cloud Run.", "blocked_by": "ADR 0114 D4, confirmed by ADR 0114 Amendment 1, 2026-10-11"},
  "R12-CICD-9116": {"status": "MISSING", "gap": "Cloud Run traffic-split canary, an SLO gate read from the pushed series (ADR 0122), and rollback to the previous promoted digest (ADR 0115), driven through the Kargo promotion (ADR 0114 Amendment 1).", "blocked_by": ""},
  "R12-FABRIC-9118": {"status": "PARTIAL", "gap": "ADR 0111 supersedes ADR 0100 §3 for P0/P1: the managed log and followers exist (FABRIC-7035). Needs the Spanner database provisioned, the adapter proven against the emulator, and a multi-instance run; P2-P4 stay RF1 (ADR 0111 §2).", "blocked_by": ""},
  "R12-OBS-9143": {"status": "PARTIAL", "gap": "Push the registry snapshot from the central binaries to Cloud Monitoring over the REST adapter (ADR 0122), re-point the nine alert policies to the pushed types, read a series back in dev, then flip workload_metrics_exist.", "blocked_by": ""},
  "R12-OBS-9146": {"status": "MISSING", "gap": "Record the Spanner commit latency on the P0/P1 produce path per class as the quorum-acknowledgement latency (ADR 0111 Amendment 1), with a test that the series moves.", "blocked_by": ""},
  "R12-OBS-9148": {"status": "MISSING", "gap": "qip_fabric_mirror_lag_records and qip_fabric_mirror_lag_seconds from the fabric mirror (ADR 0120), labels bounded by region list and committed stream set, with a test.", "blocked_by": ""},
  "R12-RES-9168": {"status": "PARTIAL", "gap": "Replication and lease promotion exist for P0/P1 (ADR 0111). Needs the game day on a deployed multi-instance fabric and the Reflex standby promotion drill.", "blocked_by": ""},
  "R12-RES-9169": {"status": "PARTIAL", "gap": "Followers catch up from the managed log (ADR 0111). Needs followers in other zones in Terraform (ADR 0111 Amendment 1, three per region) and a follower-loss game day.", "blocked_by": ""},
  "R12-FABRIC-9182": {"status": "PARTIAL", "gap": "Multi-instance fabric on a Spanner lease (ADR 0111): open the broker module to three C4/C4D instances, one per zone (ADR 0111 Amendment 1), and plan it.", "blocked_by": ""},
  "R12-GCP-9183": {"status": "BLOCKED", "gap": "HPA/VPA and Cluster Autoscaler are the requirement; refused. Lag-scaled consumers are FABRIC-8109 on a Compute Engine MIG.", "blocked_by": "ADR 0114 D4, confirmed by ADR 0114 Amendment 1, 2026-10-11"},
  "R12-GCP-10030": {"status": "BLOCKED", "gap": "Regional GKE Standard for warm services, solvers, agents and sinks is refused; each capability is placed per ADR 0114 Amendment 1.", "blocked_by": "ADR 0114 D4, confirmed by ADR 0114 Amendment 1, 2026-10-11"},
  "R12-SEC-10031": {"status": "PARTIAL", "gap": "mTLS for warm services is met by ADR 0112's hop table (Amendment 1): rustls mTLS with CA Service workload certificates on every Compute Engine hop, IAM invocation into Cloud Run. Needs CA Service issuance in Terraform and the composition roots, and the IAM evidence of API-8113. The mesh itself is refused.", "blocked_by": ""},
  "R12-MODEL-10065": {"status": "BLOCKED", "gap": "GPU node pools are refused (ADR 0114 D4); accelerators are placed as Vertex AI custom jobs (ADR 0122) and only for an experiment that measures a need.", "blocked_by": "ADR 0114 D4; ADR 0122 §4 and ADR 0069 (no measured accelerator consumer)"},
  "R12-GOV-10149": {"status": "BLOCKED", "gap": "Live execution in prod.", "blocked_by": "ADR 0108 (proposed, not accepted); ADR 0003 paper trading only"},
  "R12-MODEL-1082": {"status": "MISSING", "gap": "Build the offline Python research lane (ADR 0122): own directory and hash-pinned lockfile, Vertex AI custom jobs with no route to production, model packs in a provider-served form with data digest, commit and classical baseline, admitted only through ADR 0110's signed publish; flip enable_vertex_ai in dev with the plan shown.", "blocked_by": ""},
  "R12-CAPITAL-4076": {"status": "BLOCKED", "gap": "Real movement of money.", "blocked_by": "ADR 0021 (capital movement refused) and ADR 0003; ADR 0108 not accepted and does not cover capital movement"},
  "R12-CAPITAL-4081": {"status": "BLOCKED", "gap": "Financing adapters with counterparties.", "blocked_by": "ADR 0021 and ADR 0003; ADR 0108 not accepted and does not cover financing"},
  "R12-LEDGER-5023": {"status": "BLOCKED", "gap": "Bank statement reconciliation needs bank accounts.", "blocked_by": "ADR 0021 and ADR 0003 (no real bank accounts); ADR 0108 not accepted"},
  "R12-LEDGER-5024": {"status": "BLOCKED", "gap": "Custodian reconciliation needs a custodian.", "blocked_by": "ADR 0021 and ADR 0003 (no custody); ADR 0108 not accepted"},
  "R12-LEDGER-5027": {"status": "BLOCKED", "gap": "Settlement-agent reconciliation implies live settlement.", "blocked_by": "ADR 0003 and ADR 0021; ADR 0108 not accepted"},
  "R12-AGENCY-6092": {"status": "BLOCKED", "gap": "Publishing externally is refused; agents may draft for a person through the drafting adapter (ADR 0114 Amendment 1).", "blocked_by": "ADR 0114 D8, confirmed 2026-10-11"},
  "R12-AGENCY-6093": {"status": "BLOCKED", "gap": "Outreach is refused; agents may draft for a person through the drafting adapter (ADR 0114 Amendment 1).", "blocked_by": "ADR 0114 D8, confirmed 2026-10-11"},
  "R12-AGENCY-6095": {"status": "BLOCKED", "gap": "Product pricing as an action is refused; a price analysis may be drafted for a person.", "blocked_by": "ADR 0114 D8, confirmed 2026-10-11"},
  "R12-AGENCY-6096": {"status": "BLOCKED", "gap": "Operational change by an agent is refused (and change-management rules forbid an agent deploying); a change may be drafted for a person.", "blocked_by": "ADR 0114 D8, confirmed 2026-10-11"},
  "R12-AGENCY-6097": {"status": "BLOCKED", "gap": "Moves real capital and collateral.", "blocked_by": "ADR 0003 and ADR 0021 (live capital); ADR 0108 not accepted"},
  "R12-AGENCY-6098": {"status": "BLOCKED", "gap": "Live market actions.", "blocked_by": "ADR 0003 (live orders refused); ADR 0108 not accepted"},
  "R12-GOV-6119": {"status": "BLOCKED", "gap": "Credentials for live custody, transfer, banking, market creation and commerce.", "blocked_by": "ADR 0003 and ADR 0021; ADR 0108 not accepted"},
  "R12-GCP-7006": {"status": "BLOCKED", "gap": "Stateful services on GKE are refused; stateful processes run on Compute Engine (fabricd, ledgerd) and state in Spanner (ADR 0111, ADR 0113).", "blocked_by": "ADR 0114 D4, confirmed by ADR 0114 Amendment 1, 2026-10-11"},
  "R12-FABRIC-7037": {"status": "PARTIAL", "gap": "Leadership is a fenced Spanner lease. Move stream definitions, partition placement, broker epochs and ACL metadata into the same regional database, written under lease-epoch checks (ADR 0111 Amendment 1); the 'no cloud database' clause is refused. Then the controller quorum game day.", "blocked_by": ""},
  "R12-FABRIC-7038": {"status": "MISSING", "gap": "Build fabric-mirror per ADR 0120: StreamPolicy Mirroring::Async arm, target-region replicating consumer into region-qualified streams, idempotent checkpoint in regional Spanner, Gap records, mTLS; prove with a two-broker local test. Deployment waits on a second region.", "blocked_by": ""},
  "R12-FABRIC-7046": {"status": "PARTIAL", "gap": "mTLS half done. QUIC only on a committed measurement of the cross-region peer-mesh hop (ADR 0112 §4, Amendment 1); commit the measurement.", "blocked_by": ""},
  "R12-FABRIC-7048": {"status": "BLOCKED", "gap": "prost is not admitted; the fabric codec stays canonical JSON under the schema lock until an external wire format requires protobuf.", "blocked_by": "ADR 0109 (prost not admitted), confirmed by ADR 0112 Amendment 1, 2026-10-11"},
  "R12-RES-7114": {"status": "MISSING", "gap": "With the ADR 0120 mirror: local streams stay writable with the mirror down, lag series and bound, stale marking past the bound, replay from checkpoint or archive on return; prove by a two-broker game day. Deployment waits on a second region (ADR 0035 stands).", "blocked_by": ""},

  "R12-FABRIC-7035": {"status": "PARTIAL", "gap": "Declare the fabric's regional Spanner database in Terraform (gated), prove SpannerManagedLog against the Spanner emulator, add fabricd to the image matrix, and deploy it in dev in ADR 0035's region once billing is enabled, plan shown first (ADR 0111 Amendment 1).", "blocked_by": ""},
  "R12-FABRIC-8085": {"status": "PARTIAL", "gap": "Wire leader_for into lease contention: an instance contends immediately only for partitions it is the rendezvous-preferred leader of, otherwise after expiry plus grace (ADR 0111 Amendment 1); followers in other zones via the broker module. P2-P4 stay RF1 by ADR 0111 §2.", "blocked_by": ""},
  "R12-FABRIC-8096": {"status": "PARTIAL", "gap": "Three C4/C4D brokers per production region, one per zone, meets the requirement's minimum (ADR 0111 Amendment 1). Open the module to multiple addresses, refuse two per zone and any non-C4/C4D shape, and show a prod plan.", "blocked_by": ""},
  "R12-GOV-6120": {"status": "PARTIAL", "gap": "The cell admits only through admit_bounded and refuses an envelope without AuthorityBounds; the centre issues bounded envelopes (ADR 0008 amendment, 2026-10-11).", "blocked_by": ""},
  "R12-GOV-6124": {"status": "PARTIAL", "gap": "Build the internal drafting adapter (ADR 0114 Amendment 1), whose only effect is an outbox append, so a shipped binary writes the third (Executed) record honestly.", "blocked_by": ""},
  "R12-GOV-6125": {"status": "PARTIAL", "gap": "Bind AuthorityLevel to an approval carrying an Attested presence from an enrolled WebAuthn key (ADR 0121) and integrate.", "blocked_by": ""},
  "R12-AGENCY-1006": {"status": "PARTIAL", "gap": "External action stays refused (ADR 0114 D8). Build the drafting adapter and closed-loop attribution of drafts' measured outcomes into the world model (ADR 0114 Amendment 1).", "blocked_by": ""},
  "R12-AGENCY-1025": {"status": "PARTIAL", "gap": "Integrate ToolRegistry/ExecutableScope with authority checks into a runtime path through the drafting adapter (ADR 0114 Amendment 1).", "blocked_by": ""},
  "R12-AGENCY-1056": {"status": "PARTIAL", "gap": "Resolve each citation against the evidence store and compare the claim with it; add factual-support and no_false_claims cases to the before-adapter guard test.", "blocked_by": ""},
  "R12-AGENCY-1057": {"status": "PARTIAL", "gap": "Resolve citations against the evidence store/manifest before trusting the source; refuse a draft with no citations where provenance is required.", "blocked_by": ""},
  "R12-AGENCY-1059": {"status": "PARTIAL", "gap": "Refuse an empty required_disclosures in ConductGate::new and add a missing-disclosure case to the before-adapter guard test.", "blocked_by": ""},
  "R12-AGENCY-1060": {"status": "PARTIAL", "gap": "Evaluate the step itself (tool, exposure, sequence) against conduct rules, not only draft vocabulary.", "blocked_by": ""},
  "R12-RISK-4111": {"status": "PARTIAL", "gap": "Implement ADR 0121 (attested operator presence), then ADR 0075 Gates B and C, so a grant is issued, and narrowed, in a deployed binary.", "blocked_by": ""},
  "R12-RISK-4112": {"status": "PARTIAL", "gap": "A kernel test driving dissent through the cycle into the sizing budget and the next grant, mutation-verified; ADR 0121 so a grant is issuable in a deployed binary.", "blocked_by": ""},
  "R12-RISK-4114": {"status": "PARTIAL", "gap": "A kernel test that an OOD score narrows the sizing budget; ADR 0121 so the narrowed grant is issued in a deployed binary.", "blocked_by": ""},
  "R12-RISK-4115": {"status": "PARTIAL", "gap": "A kernel test driving a regime mismatch into the sizing budget and the next grant; ADR 0121 so a grant is issuable in a deployed binary.", "blocked_by": ""},
  "R12-SEC-10053": {"status": "PARTIAL", "gap": "Replace kms_protection_level with per-purpose variables: signing keys HSM everywhere, CMEK HSM in stage and prod; a Terraform test refusing SOFTWARE where forbidden and admitting it in dev/test CMEK (ADR 0110 Amendment 1).", "blocked_by": ""},
  "R12-SEC-9109": {"status": "PARTIAL", "gap": "Write the four HSM signing keys and the publish workflow in Terraform, apply to dev through infra.yml with the plan shown, set QIP_ARTEFACT_SIGNER=kms in dev after a real-key verification (ADR 0110 Amendment 1).", "blocked_by": ""},
  "R12-TICK-2057": {"status": "PARTIAL", "gap": "Commit the bytes/day and drain-thread measurement, set QIP_TICK_LAKE_CODEC=zstd explicitly in the execution-node template and flip the default, unless the measurement shows spool pressure rising (ADR 0113 Amendment 1).", "blocked_by": ""},
  "R12-CICD-9097": {"status": "PARTIAL", "gap": "Apply modules/build-pool to dev with the plan shown, flip cloud_build_private_pool_enabled in a separate commit, and extend the two-build comparison to the portal with a commit-derived build id (ADR 0115 Amendment 1).", "blocked_by": ""},
  "R12-SEC-9077": {"status": "PARTIAL", "gap": "Fleet stays in dev until ADR 0113's split; add VPC egress restricted to Google APIs, roles/monitoring.viewer only, a narrowed bucket grant, and worker-side refusal of non-synthetic datasets (ADR 0102 amendment, 2026-10-11).", "blocked_by": ""}
}
```
