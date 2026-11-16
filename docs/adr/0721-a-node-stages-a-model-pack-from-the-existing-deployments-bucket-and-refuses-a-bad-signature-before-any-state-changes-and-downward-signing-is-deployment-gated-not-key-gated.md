# ADR 0721: A node may stage a model pack from the existing deployments bucket and refuses a bad signature before any state changes, and downward signing waits on deployment evidence, not on a missing key type

- **Status**: Proposed 2026-11-16, for the owner. Decision 2 amends Accepted
  ADR 0110, which delivers packs inline in the signed payload, and asks for a
  read grant on the execution-node identity, which is an IAM change. Nothing
  here is applied.
- **Date**: 2026-11-16
- **Rows**: R12-MESH-1071, R12-REFLEX-10085

## Context

R12-MESH-1071 requires two components to exchange signed policy and artifacts
downward over a real channel, with an unsigned or invalid message refused.
Its register row is PARTIAL: policy, halts and capital envelopes reach the cell
only down a verified signed path, and the acceptance suite
`downward_signed_path` covers it. The row's gap text says asymmetric signing is
an open gap under ADR 0043. That blocker is stale. ADR 0110 (Accepted) builds
signing as `TrustRoot::Bundle` over `qip_transport::trust::BundleVerifier`. What
remains is that nothing is deployed.

R12-REFLEX-10085 requires the Reflex package manager to stage a model or policy
bundle from a registry or bucket and verify its signature before activation.
Its check is that a bundle with a bad signature is refused before any state
changes. Today a cell verifies a signed payload and installs strategies only
from a verified payload that names the plan bytes. No model bundle is staged
from a store. ADR 0110 delivers packs inline in the signed payload, and the
node identity deliberately holds no Artifact Registry grant
(`modules/execution-node/main.tf`). The 2026-10-31 owner ruling says no new
repository is bought for bundles: packs are staged from the existing
`qip-deployments-<env>` bucket. It also says an ADR amending ADR 0110 must
decide the store, the node's read grant and the refusal order before a fetch
step is built.

## Decision

1. **R12-MESH-1071: leave to the owner, on deployment evidence.** The code path
   is not the gap. The row closes when the owner applies the signing
   infrastructure (`artefact_signing` under R12-SEC-9137) and a signed payload
   is shown crossing a real channel to a deployed node with an invalid one
   refused. Until then it stays PARTIAL and `deployable` stays false. The
   ADR 0043 blocker is recorded here as superseded by ADR 0110, so the row's
   gap text should name deployment alone. This decision builds nothing and
   relaxes nothing.

2. **R12-REFLEX-10085: build, a stated way, if the owner accepts the
   amendment.** ADR 0110's inline delivery stays the default and stays
   sufficient. A second route is added beside it: a signed payload may name a
   pack by object path and digest in `qip-deployments-<env>`. The node reads
   that object, and the payload, not the bucket, is the trust anchor. A pack is
   admitted only if the signed payload names its digest and the pack's own
   signature verifies against the trust bundle.

3. **Refusal comes before any state change.** The node fetches into a scratch
   buffer held in memory. It checks the digest, then the signature, then the
   two-person publish record that ADR 0110 already requires. Only after all
   three does anything reach the installer. A fetch failure, a digest mismatch,
   a bad signature or a missing second approver leaves the installed set
   unchanged, journals a refusal naming which check failed, and does not retry
   with a relaxed check. No fallback to an unsigned pack exists.

4. **The node identity gets one read grant, on that one bucket, and nothing
   else.** The grant is object read on `qip-deployments-<env>`, with no list
   permission over other buckets, no write, and no Artifact Registry grant. It
   is the single missing permission, not a widening to make an error go away.
   Because it changes IAM, it needs the owner's decision.

5. **The fetch carries an explicit timeout and a size bound.** The node has no
   pack-size figure today. **A figure is needed from the owner: the maximum
   pack size a node will read.** None is chosen here. Until it is given, the
   fetch step is not enabled, and inline delivery is the only route.

6. **The fetch step runs off the order path.** It stages while the cell works
   and activates between passes, so no network read happens inside
   `Cell::work`, consistent with ADR 0109's rule keeping the decision core
   free of async.

7. **Tests required, none written here.** The row's check is
   `a_bundle_with_a_bad_signature_is_refused_before_any_state_changes`, with a
   mutation that moves the activation ahead of the verify. It is shared with
   R12-REFLEX-8070's second-approver step, which this decision does not
   replace.

8. **No new dependency.** The read uses the in-tree HTTP client behind the
   existing egress arrangements. If a Google client crate looks necessary, that
   is its own ADR under ADR 0109, not this one.

## What it costs

- The execution node gains a network read it did not have, and a read grant
  that must be reviewed and applied. Until the owner applies `artefact_signing`
  and the grant, the amendment has no effect.
- A second delivery route means two paths to keep consistent. The inline route
  remains the one with fewer moving parts.
- Reading from a bucket adds an availability dependency at staging time. A cell
  that cannot stage keeps running its installed set, which is the existing
  behaviour of a cell that cannot reach the centre.
- Signing infrastructure carries a monthly bill the register already records
  against R12-SEC-9137. This record adds none.

## What would make this wrong

- If a node is ever allowed to activate a pack on a bucket read alone, the
  signature check has become advisory. The signed payload must stay the anchor.
- If the read grant is later widened to ease an error, the least-privilege
  argument in decision 4 is gone.
- If inline delivery proves large enough to carry every pack, the second route
  is complexity with no benefit, and decision 2 should be withdrawn.
- If the owner's deployment evidence shows the signed downward channel failing
  on a real network, decision 1's reading that only deployment is missing is
  wrong, and the row needs a new finding.
- This record touches no quantum family, so ADR 0150 is untouched. It does not
  touch the paper-trading boundary: a pack changes what a cell may install
  under its existing envelope, never the ceiling.
