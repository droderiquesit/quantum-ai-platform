# ADR 0535: A reflex package is drafted unsigned by the deep brain and signed only by the two-person model-pack publish, and no running process holds a package signer

- **Status**: **Accepted 2026-11-13 under the owner's 2026-10-31 delegation
  and ruling** (`docs/blueprint/lanes/wave1031-handoff-2026-10-31.json`,
  R12-REASON-7203, class `ADR_THEN_IMPLEMENT`), session t5-04 (wave580).
  It narrows ADR 0131 decision 1's "the deep brain's `PackageSigner` is the
  KMS signer under the `model-pack` key" to: the deep brain has no signer.
  It adds no dependency, no key and no figure, and touches no paper-trading
  layer.
- **Date**: 2026-11-13
- **Amends**: ADR 0131 decision 1, the clause naming the deep brain's
  `PackageSigner` as the KMS signer. Everything else in ADR 0131 stands.
- **Rows**: R12-REASON-7203 (stays PARTIAL; see "What this leaves open").

## Context

The Cognitive Compiler runs in `qip-deepbrain` and distils a promoted
fill-bias capability into a bounded `ReflexPackage`. `ResearchLoop` signs
and publishes it only through an installed `PackageSigner`; the binary
installs none, so every package was counted `held_unsigned` and nothing was
written. ADR 0131 decided reflex packages sign under the `model-pack` class,
two-person, and named the deep brain's signer as the KMS signer. The owner
ruled on 2026-10-31 that no running process signs a reflex package: the
deep brain writes an unsigned draft and the two-person model-pack publish
signs it, so the deep brain never holds the `model-pack` key.

## Decision

1. **The deep brain drafts.** With no signer and a package store,
   `ResearchLoop` writes a `qip_contracts::reflex_package::ReflexPackageDraft`
   (the package and its digest) under `drafts/` in
   `REFLEX_PACKAGES_NAMESPACE`. The count stays `held_unsigned`: nothing a
   cell can load was written. A draft does not decode as a signed or a
   published package, so a loader handed one refuses it.
2. **The publish signs, two-person, under `model-pack`.** The signed bytes
   are `reflex-package/v1 ‖ lp(digest) ‖ Publication::canonical()` under
   `ArtefactClass::ModelPack` (`ReflexPackageDraft::signing_canonical`). The
   publication is inside them, as it is in a `ModelPack`, so a package cannot
   be re-attributed after signing. `ReflexPackageDraft::publish` refuses a
   one-person publication.
3. **A cell loads only a `PublishedReflexPackage`.**
   `PublishedReflexPackage::load` re-admits the package, recomputes the
   digest, refuses a one-person publication, verifies the signature under a
   `model-pack` key the trust bundle holds, and refuses an expired package.
4. **The publish uses `qip reflex-package`.** `signing-message`,
   `attach-signature` and `verify` mirror `qip model-pack`; the publication
   is named on both signing steps because it is inside the signed bytes.
5. **No running process holds a package signer.** `qip-deepbrain`'s
   composition root never calls `ResearchLoop::with_package_signer`, and its
   banner says so. The in-process `SignedReflexPackage` path stays for tests
   and the local loop only.

## Evidence

- `qip-deepbrain/tests/research_market.rs::a_draft_the_deep_brain_writes_loads_only_once_the_two_person_model_pack_publish_signs_it`
  drives `sweep_over`, reads the draft, signs it with `qip_transport::trust::LocalSigner`
  under `model-pack`, and loads it through a `BundleVerifier`. Mutations,
  each restored byte-for-byte: dropping the publication from the signed bytes
  let the re-attributed package load; dropping `publication.validate()` in
  `publish` accepted the one-person publish.
- `research_market.rs::without_a_signer_a_compiled_package_is_held_and_nothing_is_published`
  now asserts the one key written is the draft under `drafts/` and that
  neither loader accepts it. Mutation: writing outside `drafts/` failed it.
- `qip-cli/tests/reflex_package.rs::a_reflex_package_verifies_only_under_the_two_person_publication_its_signature_covers`.
  Mutation: dropping the CLI's `publication.validate()` printed a
  one-person message and failed the test.

## What this leaves open

- The publish workflow, `.github/workflows/model-pack-publish.yml`, is a
  protected path. The step that calls `qip reflex-package` is written as
  `docs/blueprint/lanes/t5-04-R12-REASON-7203.patch` and not applied.
- The `model-pack` key version exists only where
  `infrastructure/terraform/artefact-signing.tf` has been applied; nothing
  in this lane creates it.
- No cell loads a reflex package: `qip-edge` has no caller of
  `PublishedReflexPackage::load`. The consumer is separate work.
- Moving a draft out of the deep brain's blob store into the reviewed
  repository path the workflow reads is a manual step.

## What it costs

A reflex package now waits for a human two-person publish before any cell
could use it. Two new types sit beside `SignedReflexPackage`, and the
in-process signer path is kept, so two load paths exist until the local one
is withdrawn.

## What would make this wrong

- If a composition root ever calls `with_package_signer` with a KMS signer,
  the deep brain holds the key the ruling says it must not.
- If the reflex package's signed bytes and a model pack's could collide, a
  signature could move between them. The domain tags `reflex-package/v1`
  and `model-pack/v1` keep them apart; a change to either tag must keep that.
- If the publish workflow signs a draft without the publication in the
  message, the re-attribution refusal holds only in tests.
