# Capital AI v4 — the target architecture

The desk's architecture document, added verbatim as
[capital-ai-architecture-v4.html](capital-ai-architecture-v4.html). It is a
TOGAF-layered, five-diagram specification of the platform's target state on
Google Cloud. Its own words set the frame, and this README keeps them:
the 100/100 grade "measures specification completeness, not implementation."
Everything below reconciles that target with what this repository actually
provisions today, so nobody mistakes a diagram for a deployment.

## The target, in one table

| v4 element | Target | In this repository today |
|---|---|---|
| Regions | Three: `us-east4`, `europe-west2`, `asia-northeast1`; global anycast edge | One environment at a time via `infrastructure/environments/<env>/`; dev targets `us-east4`. Multi-region scale-out is tracked work. |
| Edge | Cloud DNS + Global LB + Armor + CDN, one front door | Not yet provisioned; the portal fronts through its own ingress once deployed. |
| Identity | Identity Platform, MFA, 15-minute JWTs; IAP + JIT for admins | Identity Platform Terraform exists (`modules/identity`); portal has the adapter seam and a local dev identity. IAP/JIT not yet provisioned. |
| App tier | Custom portal + APIs on Cloud Run, scale to zero | Portal and landing are Next.js apps, currently deployable to GKE alongside qip-api; Cloud Run hosting is the intended landing point for the stateless tier. |
| Warm tier | One GKE Autopilot cluster per region, default-deny namespaces | One private GKE standard cluster per environment, default-deny network policy, Binary Authorization, Workload Identity. Autopilot per region is a target-state divergence, recorded here. |
| Trading cell | Compute Engine, in-process risk+OMS+execution, NVMe append-only journal | `qip-edge` cells and the seven-stage kernel implement the cell logic in Rust; the journal is the hash-chained event log. Deployed onto GKE today, not dedicated Compute Engine. |
| Data plane | Spanner ledger, Bigtable time series, BigQuery analysis, Pub/Sub backbone, Firestore control, WORM evidence | Deliberately different today: ADR 0002/0009/0011 keep the decision core at two dependencies with in-tree storage, and the evidence bucket (WORM-style, KMS-encrypted) is provisioned by Terraform. Managed data services are admitted only through a new ADR, per the standing dependency policy. |
| Wallet zone | Cloud HSM (FIPS 140-2 L3), signer on Confidential VM, separate perimeter | Not provisioned. Nothing in the platform signs money movement today, and the paper-trading boundary (three layers) makes a live path structurally absent. This row activates only if custody ever becomes real scope, with its own ADR and review. |
| Supply chain | SLSA + Cosign, Binary Authorization, signed digests only | Implemented: deploy pipeline signs and attests; the cluster admits attested digests only. |
| Zero trust | Workload Identity everywhere, no SA keys, VPC-SC perimeters | Workload Identity Federation and no-key policy are enforced (rules and acceptance tests). VPC Service Controls are not yet provisioned. |

## How the reconciliation is used

The v4 document is the direction of travel; the ADRs are the record of what
is admitted when. Where the two disagree — managed databases against the
two-dependency core, Autopilot against the hardened standard cluster — the
divergence is stated here and resolved through an ADR, not silently in a
diff. The scale-out order follows the document's own footprint: `us-east4`
first, then `europe-west2`, then `asia-northeast1`.
