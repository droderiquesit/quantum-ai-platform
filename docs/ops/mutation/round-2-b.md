# Round 2 mutation verification — lane B

Branch `ccr-dc3c4d84-dayxv0-r3-mutation-b`, from `a07946ac`. Round 2 is
`b6ee74ad..a07946ac`. Its implementation lanes were told not to run tests,
so most of the tests they added had never been shown to fail when the
behaviour they guard is broken.

**Territory.** `qip-transport` (tls, trust, kms, server), `qip-contracts`
(trust, ledger operation, authority bounds, section 27), `qip-core`
(deployment), `qip-storage`, `qip-fabricd`, `qip-ledgerd`, `qip-api`,
`qip-fastbrain`, `qip-deepbrain`, `qip-cli`, `qip-agency`,
`qip-optimization-engine`, `qip-acceptance`. Nothing under peer/mesh was
touched.

Round 2 added no tests to `qip-evidence`, `qip-market-ingestion`,
`qip-core::ids`, `qip-contracts::{captured_event,peer}` or
`qip-transport::server`. This was checked by comparing `#[test]` function
names at `b6ee74ad` and at `a07946ac` in each file round 2 changed.

**Method.** For each test:

1. Apply the mutation its doc comment names, as an exact string
   replacement, or neutralise the behaviour it guards when no mutation is
   named.
2. Run only that test.
3. Restore the file byte-for-byte (compared against the original bytes) and
   confirm the test is green again.

Each run was checked for `COMPILE-ERROR` and for a filter that matched no
test. A mutation that does not compile, or a run that executed nothing,
proves nothing; when either happened the run was redone. `fired` means the
test failed **for the stated reason**, and the quoted text is that
assertion's message.

The harness is `mut.py`, kept in the session scratchpad and not committed.
Two sub-agents ran the agency/CLI/optimization and deepbrain/storage rows in
isolated worktrees with the same harness. Their commits were cherry-picked
here.

## Summary

| | count |
|---|---|
| round-2 tests checked | 139 |
| mutations run | about 200 |
| tests that did not fire on a mutation they claimed to catch (fixed) | 12 |
| doc comments that named the wrong mutation or the wrong failing assertion (corrected) | 6 |
| implementation defects found | 0 |

### Tests that did not fire, and what was done

| test | mutation that survived | fix | commit |
|---|---|---|---|
| qip-transport `trust::a_signature_verifies_only_over_its_own_bytes_class_and_key` | `BundleVerifier::verify` takes the class from the bundle entry the signature names, so a policy signature presented as a halt verifies | added the wrong-class arm its doc promised: "a policy signature verified as a halt" | feaa680f |
| qip-transport `tls` empty-material test (the chain half) | `if false && chain.is_empty()` in `WorkloadIdentity::from_pem`. All three inputs were `b""`, so the key parse refused first | new `an_identity_with_no_certificate_is_refused_beside_a_good_key_and_roots`, which matches the chain's own message (the roots message shares "holds no certificate") | feaa680f |
| qip-api / qip-fastbrain / qip-deepbrain `an_unset_survival_configuration_…` | the root's `platform.set_survival_policy(policy)` deleted. The test called the kernel itself | `apply_survival_policy_from(platform, lookup)`; the test now drives the root's install | f67ac416 |
| qip-ledgerd `a_ledger_reaches_the_fabric_in_plaintext_only_where_a_local_deployment_chose_it` | `parse` accepts `https://` on any link | added the https-on-plaintext arm: "https on a plaintext link" | 29744caf |
| qip-fabricd `managed::a_killed_leader_is_replaced_by_its_follower_…` | follower `catch_up` skipped in `tend_leases`. Promotion catches up too, so nothing failed after the kill | asserts a warm follower before the kill: "the follower never caught up with the leader's committed records" | 29143082 |
| qip-fabricd `chaos::seeded_kills_and_partitions_…` | `MemoryManagedLog::commit` without its epoch check. A stale commit lands before the new leader's first, so epochs stay monotonic | asserts that every acknowledger holds the lease at its epoch: "seed 2 step 355: zone-b#5 acknowledged in epoch 6 without holding the lease" | c6e33d6e |
| qip-deepbrain `breadth::a_malformed_misspelt_or_empty_plan_is_a_configuration_error` | `deny_unknown_fields` removed from `BreadthPlan`. The misspelt plan was then refused as "no section" under the same prefix | the misspelt plan carries a real section, and each case asserts its own reason | fb70aab4 |
| qip-agency `causal::an_interventions_effect_is_computed_by_graph_surgery_…` | `evaluate` keeps a fixed variable's parents (no surgery). Exogenous terms were 0, so the confounder never moved | evaluates with z = 1 and asserts x = 3, y = 3.5, plus the premise that the uncut model moves x | ed6220b9 |
| qip-agency `causal::effect_sizes_are_learned_from_data_…` | fitting y without z fired, but on an `unwrap` of the missing z→y edge, not on the confounded x→y | the x→y assertion runs first: "x->y learned 2.0576…" | ed6220b9 |
| qip-agency `journal::a_guard_resumed_from_the_journal_has_already_spent_…` | the floor-raise in `ActionGuard::resume` deleted. The window-0 check ran on a guard that had begun window 1 itself | a freshly resumed guard is asked for window 0 and must refuse | 6c768d83 |

`tls::empty_tls_material_…`'s roots half fired. The tests that fired only
after the fix are not repeated in the table below.

### Doc comments corrected (the test fired, but not as described)

- qip-transport `trust::a_signature_under_a_key_outside_the_bundle_is_refused`
  named "skip `key_for`". A verifier has no public key except the ones the
  bundle supplies, so substituting the bundle's first key held, because
  `ring` refused the forgery under it. The comment now names a mutation
  that exists (`verify` returns Ok without calling `ring`), and that one
  fires.
- qip-transport `mtls::a_client_with_no_certificate_never_reaches_a_handler`
  said the handler runs under `allow_unauthenticated()`. It does not: the
  `None` arm answers 403. The test fires on `outcome.is_err()`.
- qip-fabricd managed and chaos: see above.
- qip-agency `causal` graph-surgery: the description of mutation (a) was
  wrong.
- qip-cli `agency::the_binary_refuses_to_run_a_pass_without_…` said the
  refusal cases "exit zero". For the policy case only the message assertion
  guards it.

## Full table

### Signing, trust and verification

| test | mutation | fired | fix |
|---|---|---|---|
| contracts `trust::the_signed_message_separates_class_key_and_bytes_injectively` | drop the length prefixes in `signing_message` | yes — "a key-version/bytes split collided" | — |
| contracts `trust::a_shared_key_mac_never_parses_as_an_asymmetric_signature` | `parse` accepts bare hex | yes — "an HMAC parsed as ECDSA" | — |
| contracts `trust::a_key_published_for_one_class_refuses_every_other_class` | skip the class comparison in `key_for` | yes — "the halt key verified a policy artefact" | — |
| contracts `trust::a_revoked_or_lapsed_key_verifies_nothing` | ignore `revoked`; ignore `not_after` | yes, both — `key_for(.., "policy/1", now).is_err()`; `key_for(.., "policy/2", after).is_err()` | — |
| contracts `trust::a_bundle_with_a_duplicate_or_malformed_entry_is_refused_whole` | drop the duplicate check; drop the point check | yes, both — `duplicate.is_err()`; `TrustBundle::new(vec![bad], ..).is_err()` | — |
| contracts `trust::a_publication_naming_one_person_twice_is_refused` | byte comparison; accept an empty name | yes, both — `"alice"/"Alice " passed as two people`; `"alice"/"" passed` | — |
| transport `trust::a_signature_verifies_only_over_its_own_bytes_class_and_key` | `verify` returns Ok without `ring`; class taken from the key | yes; **no → fixed** — "a policy signature verified as a halt" | feaa680f |
| transport `trust::a_signature_under_a_key_outside_the_bundle_is_refused` | skip `ring` (the named "skip `key_for`" cannot be expressed) | yes — `verify(Halt, b"halt", &forged, now).is_err()` | doc, feaa680f |
| transport `kms::an_answer_for_another_key_version_is_refused` | drop the name comparison | yes — the self-check message, which does not contain "asked" | — |
| transport `kms::an_answer_from_a_key_that_is_not_hsm_protected_is_refused` | drop the protection-level check | yes — the self-check message, which does not contain "HSM" | — |
| transport `kms::a_signature_that_does_not_verify_under_the_bundle_is_refused` | swallow the self-check error | yes — "a signature the bundle rejects was returned" | — |
| transport `kms::base64_matches_every_rfc_4648_vector_in_both_directions` | wrong padding arm | yes — `left == right` | — |
| transport `kms::non_canonical_base64_is_refused` | drop the zero-bits check; drop the padding-position check | yes, both — `"Zh==" decoded`; `"Zg==Zg==" decoded` | — |
| api `signing::a_local_signer_is_refused_in_every_deployment_but_local` | drop the deployment check on `local` | yes — the refusal names the keys file, not `kms` | — |
| api `signing::the_shared_key_posture_ends_on_its_removal_date_outside_local` | ignore `refuse_shared_key_after_removal` | yes — `configure(.., Prod, after, ..).is_err()` | — |
| api `signing::a_local_signer_whose_key_is_not_in_the_bundle_stops_start_up` | skip `prove_signer` in `take` | yes — "a signer the bundle cannot verify was installed" | — |
| api `signing::a_deployed_api_with_no_release_holds_the_model_slot_to_nothing_…` | skip `admit_model_packs` in the `None` arm | yes — `deployed.model_packs_gate_the_manifest()` | — |
| contracts `authority_bounds::the_signed_payload_covers_leverage_product_classes_and_regulated_functions` | drop each bound from `signing_payload` (3 mutations) | yes, all three — "leverage", "product class", "regulated function" | — |
| contracts `authority_bounds::a_bounded_admission_refuses_outside_…` | drop the class, leverage and function branches (3 mutations) | yes, all three — "class: Full", "leverage: Full", "function: Full" | — |
| contracts `section27::a_model_pack_missing_any_of_its_nine_fields_is_refused_on_decode` | `#[serde(default)]` on `rollback_parent`; a default `publication` | yes, both — "decoded without `rollback_parent`", "decoded without `publication`" | — |

### Seed key, deployment, mTLS admission

| test | mutation | fired | fix |
|---|---|---|---|
| core `deployment::an_absent_or_blank_deployment_is_refused_rather_than_read_as_local` | `parse(None)` gives `Local` | yes — "an unnamed deployment was accepted: Local" | — |
| core `deployment::a_near_miss_name_is_refused_rather_than_guessed` | case-insensitive match; prefix match | yes, both — `"Prod" was admitted`; `"production" was admitted` | — |
| core `deployment::only_local_relaxes_either_guard_and_the_terraform_test_environment_does_not` | `Test` relaxes reproducible keys | yes — `left == right` | — |
| api / fastbrain / deepbrain `trust::a_seed_derived_key_is_refused_in_every_deployment_but_local` | restore the old guard (refuse only when live-capable) | yes, all three — "a seed-derived key started outside a local deployment: SeedDerived" | — |
| api / fastbrain / deepbrain `trust::an_operator_key_starts_in_a_deployed_environment` | refuse regardless of the key | yes, all three (the test returns Err) | — |
| api / fastbrain / deepbrain `main::an_unset_survival_configuration_…` | default for unset; NotUnicode read as unset; kernel body deleted; **root call deleted** | yes ×3 per binary; root call: **no → fixed** — `left == right` | f67ac416 |
| transport `tls::plaintext_is_refused_in_every_deployed_environment_…` | plaintext admitted everywhere | yes — "plaintext was admitted in dev" | — |
| transport `tls::an_unconfigured_link_never_comes_up_plaintext` | absent mode means plaintext; mtls with no files gives plaintext | yes, both — "local with mode None and no certificate came up" | — |
| transport `tls::a_half_configured_workload_certificate_is_refused_by_name` | half an identity treated as unset | yes — "half an identity was accepted: None" | — |
| transport `tls::empty_tls_material_is_refused_at_load_…` | accept empty roots; accept an empty chain | roots yes — `server_authenticated_client(b"").is_err()`; chain **no → new test** | feaa680f |
| transport `mtls::a_client_presenting_a_certificate_the_roots_chain_is_served_and_named` | serve a TLS server's socket as plaintext; don't strip the identity header | yes, both — "received corrupt message"; `left == right` ("operator" survived) | — |
| transport `mtls::a_client_with_no_certificate_never_reaches_a_handler` | `allow_unauthenticated()` | yes — Ok(403); the handler count stayed 0 | doc, feaa680f |
| transport `mtls::a_certificate_from_another_authority_is_refused_at_the_handshake` | a client verifier that accepts any chain | yes — "a foreign certificate was admitted" | — |
| transport `mtls::a_plaintext_client_gets_nothing_from_a_mutual_tls_server` | route `GET ` bytes to the plaintext path | yes — "a plaintext request was served by a TLS server" | — |
| transport `mtls::a_server_naming_its_peers_admits_only_those_names` | `admit` always returns Ok(None) | yes — "the handler was not told which workload called" | — |
| transport `mtls::neither_client_will_send_the_other_schemes_url` | drop each client's scheme check | yes, both — `left == right` on the error code | — |
| transport `mtls::the_fabric_transport_over_a_mutual_tls_link_is_answered_by_a_tls_broker` | `over` builds a plaintext transport for MutualTls | yes — `transport.is_mutual_tls()` | — |
| fabricd `config::a_broker_serves_plaintext_only_where_a_local_deployment_chose_it` | link_mode given `Local` | yes — "plaintext was admitted in prod" | — |
| fabricd `config::the_managed_log_is_local_by_default_complete_when_asked_for_and_refused_half_configured` | unknown `MANAGED_LOG` gives `Ok(None)`; drop the stray check | yes, both — "an unknown managed log"; "a database with the switch off" | — |

### Ledger store selection and the ledger of record

| test | mutation | fired | fix |
|---|---|---|---|
| ledgerd `config::a_ledger_refuses_an_environment_and_a_deployment_that_disagree_…` | delete the agreement check | yes — "a prod ledger on a local deployment is refused" | — |
| ledgerd `config::a_deployed_ledger_refuses_the_local_store_and_defaults_to_spanner` | drop the `is_deployed` guard on `local`; deployed defaults to local | yes, both | — |
| ledgerd `config::a_ledger_reaches_the_fabric_in_plaintext_only_where_…` | plaintext outside local; http on TLS; **https on plaintext** | yes, yes; **no → fixed** — "https on a plaintext link" | 29744caf |
| ledgerd `spanner::a_ledger_on_spanner_books_each_delivery_in_one_transaction_…` | one commit per mutation | yes — "one delivery, one Spanner transaction" | — |
| ledgerd `spanner::a_second_writer_claiming_the_spanner_ledger_fences_the_first_out` | delete the writer-row comparison | yes — "the fenced writer is refused: Posted { postings: 4 }" | — |
| ledgerd `spanner::a_deployed_ledger_starts_only_on_a_spanner_database_it_can_reach` | fall back to a local store on a `SpannerKv::new` error; on a `claim` error | yes, both — "no credential, no start: Running"; "an unreachable database starts a ledger" | — |
| ledgerd `history::a_balance_written_outside_the_event_history_stops_the_ledger_opening` | delete `verify_history()?` | yes | — |
| ledgerd `history::a_balance_key_that_cannot_be_parsed_stops_the_ledger_…` | `continue` on a parse error | yes | — |
| ledgerd `history::an_operation_posts_to_its_subledger_and_the_ledger_reopens_…` | drop the balance-row loop | yes — `left == right` | — |
| ledgerd `history::a_redelivered_operation_moves_nothing_and_a_different_one_…` | delete the digest early return; delete the held check | yes, both | — |
| ledgerd `history::releasing_more_than_is_reserved_is_refused_and_moves_nothing` | delete the `holds_a_quantity` refusal | yes — "Posted { postings: 2 }" | — |
| ledgerd `history::a_tax_lot_is_read_from_its_postings_…` | drop the `lot_key` check | yes — "a lot id is opened once: Posted" | — |
| contracts `ledger_operation::every_operation_kind_posts_to_its_own_subledgers_…` | treasury allocation credits custody | yes — `touched {"custody", "trading"}` | — |
| contracts `ledger_operation::an_unwind_reverses_exactly_the_accounts_its_establish_posted` | `pair` ignores the phase | yes | — |
| contracts `ledger_operation::an_account_reads_back_from_its_text_form_…` | `trading:cell` parses | yes — "trading:cell-only was accepted" | — |
| contracts `ledger_operation::an_operation_read_off_the_wire_meets_the_same_refusals_…` | remove both positive-amount layers (each alone survives, as the doc says) | yes — `from_str(&zero).is_err()` | — |
| contracts `ledger_operation::a_capital_grant_is_funded_for_its_deployable_capital_…` | post `grant.amount`; drop `is_live` | yes, both | — |
| contracts `ledger_operation::a_transfer_to_itself_and_an_inventory_move_…` | no mutation named; each of the three refusals neutralised | yes, all three | — |

### Fabric

| test | mutation | fired | fix |
|---|---|---|---|
| fabricd `spanner::a_commit_under_a_superseded_lease_epoch_…` | drop `epoch != lease.epoch` | yes — "the script ran out" | — |
| fabricd `spanner::a_commit_under_the_current_lease_inserts_…` | `CURRENT_TIMESTAMP()` | yes | — |
| fabricd `spanner::a_commit_behind_the_committed_log_rolls_back_as_a_conflict` | `>` instead of `!=` | yes | — |
| fabricd `spanner::an_aborted_commit_is_retried_…` | return the error on Aborted | yes — `Unavailable("aborted")` | — |
| fabricd `spanner::an_expired_lease_is_taken_at_the_next_epoch_…` | take a live lease | yes | — |
| fabricd `spanner::base64_matches_rfc_4648_…` | `'='` whenever the chunk is shorter than 3 | yes | — |
| fabricd `bench::percentiles_are_nearest_rank_…` | `/ 100` instead of `div_ceil` | yes — p99 9 ≠ 10 | — |
| fabricd `tests/bench::the_bench_harness_reports_a_sample_per_produce_and_fetch_…` | one fetch sample; fetch at `offset + 1` | yes, both | — |
| fabricd `managed::a_killed_leader_is_replaced_by_its_follower_…` | drop the lease thread; **skip follower catch_up** | yes; **no → fixed** | 29143082 |
| fabricd `managed::a_managed_majority_producer_refuses_…` | accept an ack with no managed epoch | yes | — |
| fabricd `managed::a_seated_broker_refuses_a_local_p1_stream_…` | delete the converse refusal | yes | — |
| fabricd `chaos::seeded_kills_and_partitions_…` | **drop the epoch check**; append before commit | **no → fixed**; yes | c6e33d6e |

### Acceptance

| test | mutation | fired | fix |
|---|---|---|---|
| `exec_governance::a_seeded_market_rests_its_rule_at_the_simulated_venue` | drop `venue.list_book(book)` | yes — `left == right` (depth) | — |
| `order_route_risk_gate::every_cell_the_edge_node_assembles_carries_the_deterministic_risk_gate` | delete `.with_risk_gate` | yes | — |
| `order_route_risk_gate::a_strategy_order_that_breaches_the_cells_limit_never_reaches_the_venue` | delete `risk_admit` in `Cell::send`; delete `.with_risk_gate` | yes, both | — |
| `order_route_risk_gate::a_requote_whose_replacement_breaches_…` | skip `admit_replacement` | yes | — |
| `order_route_risk_gate::a_desk_order_that_breaches_a_limit_never_reaches_the_broker` | ignore the checker's decision | yes — `!breach.accepted` | — |
| `survival_independence::the_hedge_brain_and_the_grant_governor_import_no_alpha_component` | `use qip_strategy::*;` in hedge.rs | yes — "names qip_strategy" | — |
| `architecture::no_crate_declares_a_third_party_dependency_the_allow_list_does_not_admit` | remove `ring` from the allow-list | yes — `["qip-transport: ring"]` | — |

### Agency, CLI, optimization (sub-agent, worktree)

Every test fired: 54 mutations across causal (12), journal (4), CLI agency
(4) and optimization learning (5). The exceptions are listed above. Raw
specs and outputs were left in the session scratchpad.

### Deepbrain, storage (sub-agent, worktree)

25 of 26 tests fired on their named mutations. `breadth::a_malformed_…` was
fixed (fb70aab4).

Two limits are recorded, not fixed:

- The `0x7f` boundary in `qip-storage`'s Spanner prefix range is guarded
  twice, by the guard and by `String::from_utf8`. The mutant is equivalent:
  no test can detect a mutation of the guard alone.
- `tick_desk`'s "skip `pin_market_history`" fires only through the missing
  manifest. A reader that bypasses the manifest would need new code to
  express as a mutation.

## Limits of this pass

- **The `None` arm in `serve_tls_connection`.** It answers 403 to a session
  with no peer certificate. No test can reach it without also mutating the
  verifier, because the handshake refuses first. It is defence in depth and
  has no test of its own.
- **Spanner `INSERT` timestamp.** The test asserts the SQL text sent to
  Spanner. That text is the wire behaviour, but a rewording that kept the
  semantics would fail the test.
- **Self-transfer test premise.** The test has no premise that a valid
  transfer is admitted. Each of its refusals fired when neutralised.
