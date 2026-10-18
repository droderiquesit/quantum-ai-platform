# ADR 0131: Reflex packages sign under the model-pack class, promotion drafts the signable pack, and configuration becomes a signed artefact class

- **Status**: **Accepted**, 2026-10-18, under the owner's standing instruction
  to make routine engineering decisions and record them ("Make the decision,
  record the rationale, and continue"). It applies ADR 0110's accepted scheme
  to two more artefacts and adds one key to the key ring that record already
  declares; it admits no dependency and opens no live path.
- **Date**: 2026-10-18
- **Extends**: ADR 0110 decision 1 (one key per artefact class, naming
  "model and Cognitive Compiler packages" under `model-pack`), decision 4
  (two-person publish for widening artefacts) and Amendment 1 (the signed
  limits artefact named as the remaining gap).
- **Keeps**: ADR 0110 decisions 2 and 3 (KMS signs over REST, verification is
  local against a trust bundle baked into the attested image), ADR 0115 (one
  build, promoted by digest), ADR 0003.

## Context

**Reflex packages.** The deep brain's research loop distils promoted research
into bounded reflex packages (`qip-deepbrain/src/research.rs`,
`ResearchLoop`), and publishes one "only through an installed
`PackageSigner` (ADR 0110)". `qip-deepbrain/src/main.rs` installs none: "No
signer is installed: ADR 0110's KMS signer is a separate lane ... every
package is compiled, counted as `held_unsigned`, and written nowhere." The
register reads `qip_contracts::trust::ArtefactClass`, which has four arms
(`Policy`, `Halt`, `Envelope`, `ModelPack`), and concludes a reflex-package
class is missing (R12-REASON-10099, 3115, 7203). ADR 0110 decision 1 already
decided it: the `model-pack` key signs "model and Cognitive Compiler
packages". The gap is code, not decision.

**Model packs.** Two types share the name. `qip_kernel::model_serving::ModelPack`
is "the pack's *version and lineage* and nothing more" (its own doc), built
by `ModelPack::succeed` when `Platform::promote_model` admits a member.
`qip_contracts::policy::ModelPack` is the signable pack ADR 0110 verifies:
artifact digest, signature under `model-pack`, features, calibration,
allowed universes, budget, expiry, rollback parent, the two-person
`Publication` and the optional inline model. Promotion produces the first
and nothing produces the second, so R12-MODEL-4023 ("every accepted model is
signed, versioned, attested and added to a versioned Model Pack at
promotion") is PARTIAL.

**Configuration.** Environment-specific behaviour reaches the binaries as
reviewed configuration that `deploy.yml` deploys; no binary verifies it.
ADR 0110 Amendment 1 names exactly this as the remaining gap for limits:
"The limits an envelope or the `risk_envelope` slot may carry still arrive
as reviewed configuration deployed by `deploy.yml`, not as a separately
signed two-person artefact". R12-CICD-10150 asks that environment-specific
behaviour come from signed configuration.

Locate them with
`grep -n 'pub enum ArtefactClass' backend/crates/libs/qip-contracts/src/trust.rs`,
`grep -n 'pub trait PackageSigner\|pub struct SignedReflexPackage' backend/crates/libs/qip-contracts/src/reflex_package.rs`,
`grep -n 'held_unsigned' backend/crates/apps/qip-deepbrain/src/main.rs`,
`grep -n 'pub struct ModelPack' backend/crates/libs/qip-contracts/src/policy.rs backend/crates/runtime/qip-kernel/src/model_serving.rs` and
`grep -n 'pub fn promote_model' backend/crates/runtime/qip-kernel/src/model_serving.rs`.

## Decision

1. **Reflex packages sign under `model-pack`; no class is added for them.**
   A `SignedReflexPackage` is a Cognitive Compiler package in ADR 0110's
   words, and a separate key would split one publish rule across two keys
   for no difference in what either may do. The deep brain's `PackageSigner`
   is the KMS signer under the `model-pack` key; the cell's verifier checks a
   package against the trust bundle's `model-pack` entries. A reflex package
   widens what a cell may run, so it publishes two-person exactly as a model
   pack does, and the deep brain never holds `cloudkms.signer` alone.
2. **Promotion drafts the signable pack.** When `promote_model` admits a
   member, the kernel emits an unsigned `qip_contracts::policy::ModelPack`
   draft: the member's artifact digest, its features and calibration, the
   allowed universes and budget the promotion record states, an expiry, the
   previous signed pack's digest as `rollback_parent`, and a new additive
   `lineage` field holding the kernel pack's id, inside the signed bytes. The
   draft enters ADR 0110's two-person publish; only the signed pack is what
   a policy payload may name. The kernel pack stays the lineage record and
   is not merged away: it answers "what was promoted, in what order" and the
   signed pack answers "what may run". "Attested" in the row is the
   `Publication` (who published) plus the image attestation of the binary
   that built the artifact (ADR 0115); the pack records the artifact digest
   both cover.
3. **`configuration` is a fifth artefact class**, with its own key in the
   key ring ADR 0110 declares. It signs the environment configuration
   `deploy.yml` deploys — the values the composition roots read, the limits
   among them — as one canonical document per environment and service.
   Configuration can widen (a limit raised is a widening), so it publishes
   two-person through the same protected environment.
4. **The composition roots verify configuration before step 1.** `qip-api`,
   `qip-fastbrain` and `qip-deepbrain` verify the configuration document's
   signature against the trust bundle as the first act of reading
   configuration, and refuse to start on an unsigned, mis-signed or
   mismatched document in every deployment except `local` — the same
   fail-closed shape as `AutonomyLevel::deployable`. A verified document is
   then validated exactly as today; signing adds a check, it removes none.
5. **This closes the central half of ADR 0110 Amendment 1's limits gap, not
   the cell half.** The limits a cell enforces still arrive on the envelope
   and the `risk_envelope` slot; signing them as their own two-person
   artefact the cell verifies is not decided here.

## Consequences

- The deep brain stops writing reflex packages nowhere: they are signed,
  stored and loadable once the signer is wired, and the cell can refuse an
  unsigned one by the existing verifier.
- Code slices left: the KMS `PackageSigner` installed in
  `qip-deepbrain/src/main.rs` and the cell's loader, with
  `research_market.rs::a_fill_bias_found_by_research_is_promoted_and_published_as_a_loadable_reflex_package`
  run against the binary's signer (REASON-10099, 3115, 7203); the draft pack
  and its `lineage` field at promotion, with a test that a promoted member
  yields a draft whose digest and lineage match (MODEL-4023); the
  `configuration` class, the key, the signing step in `deploy.yml` (a
  protected path, so its patch goes to a person) and start-up verification
  in the three roots, with an `infrastructure.rs` or per-root test refusing
  an unsigned environment (CICD-10150).
- `ArtefactClass::ALL` grows to five, and the trust bundle gains a class;
  every reader that iterates the classes is touched.

## What it costs

- One more HSM key (a fixed monthly charge per key version, small) and one
  more two-person publish in the release path: every configuration change
  now waits for a second person, which slows a routine limit change.
- A start-up that refuses on a configuration signature failure turns a
  signing-pipeline fault into an outage of the central binaries. That is the
  fail-closed direction, and it is a cost.
- The kernel and contracts packs stay two types with one name; this record
  keeps them apart deliberately, and a reader still has to learn which is
  which.

## What would make this wrong

- If reflex packages and model packs ever needed different publish rules
  (for example a reflex package that may only narrow and so could be
  one-person), decision 1's shared key would force the stricter rule on both,
  and a separate class would then earn its key.
- If configuration changed often enough that two-person review became the
  bottleneck of operations, the right answer is a narrower signed document
  (limits only) with the rest left reviewed-but-unsigned — not a one-person
  configuration key.

## Rows

R12-MODEL-4023, R12-REASON-10099, R12-REASON-3115, R12-REASON-7203,
R12-CICD-10150. Each stays PARTIAL until its slice lands with a mutated test.
