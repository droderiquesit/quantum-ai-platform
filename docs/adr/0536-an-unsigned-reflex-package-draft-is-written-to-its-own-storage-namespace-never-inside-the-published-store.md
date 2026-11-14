# ADR 0536: An unsigned reflex package draft is written to its own storage namespace, never inside the published store

- **Status**: **Accepted 2026-11-14 under the owner's 2026-10-31 delegation**,
  session t5-04 (wave580). A narrowing: it moves where a draft is written.
  It changes no signing rule, adds no dependency or figure, and touches no
  paper-trading layer.
- **Date**: 2026-11-14
- **Amends**: ADR 0535 decision 1, the clause "under `drafts/` in
  `REFLEX_PACKAGES_NAMESPACE`". Every other decision of ADR 0535 stands.
- **Rows**: R12-REASON-7203 (stays PARTIAL).

## Context

ADR 0535 had the deep brain write each unsigned `ReflexPackageDraft` under a
`drafts/` prefix inside the published reflex-package store. Round 50's gate
failed on it: `qip-acceptance/tests/phase_16_exit.rs::a_compiled_cognitive_package_with_no_signer_is_held_unsigned_and_never_published`
asserts that store holds nothing once a package is held unsigned, and the
draft was there. The test is right. A prefix keeps a draft out of a loader
only while every reader filters by it; anything that lists the store whole
— that gate, an operator's sync, a future cell loader — meets the draft
where published packages live.

## Decision

1. Drafts are written to `REFLEX_PACKAGE_DRAFTS_NAMESPACE`
   (`reflex-package-drafts`), a storage class of its own in
   `qip_storage::classes`, owned by `qip-deepbrain`.
2. `ResearchLoop::with_draft_store` is the only way a draft is written;
   `with_package_store` receives signed packages only. With no draft store,
   a held package is counted and nothing is written.
3. `qip-deepbrain`'s composition root opens the two namespaces separately.

## Evidence

- `qip-acceptance --test phase_16_exit` passes on this tree without edit.
- `qip-deepbrain/tests/research_market.rs::without_a_signer_a_compiled_package_is_held_and_nothing_is_published`
  asserts the package store is empty and the draft store holds exactly the
  draft. Mutation: writing the draft to the package store fails it.
- `research_market.rs::a_draft_the_deep_brain_writes_loads_only_once_the_two_person_model_pack_publish_signs_it`
  reads the draft from the draft store and asserts the package store empty.

## What it costs

One more storage class and one more `blobs()` open at start-up. The publish
workflow reads drafts from a second namespace.

## What would make this wrong

- If a deployment maps both namespaces to one prefix, the separation is
  gone and the exit gate no longer describes the deployment.
- If a later reader lists the drafts namespace as a source of loadable
  packages, the namespace is a prefix again by another name.
