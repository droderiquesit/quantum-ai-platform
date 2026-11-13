# R12-OBS-1172 — Operations/AIOps scope map (2026-11-12, session t50-17)

The row's verification: "each scope item of 'Operations/AIOps' maps to code
reached from a composition root with a named test; a missing item fails
inspection." Locate by symbol, not by line.

| Scope item | Code | Composition-root path | Named test(s) | State |
|---|---|---|---|---|
| Self-observing platform | `qip_observability::aiops::EntityGraph`; `Operations::observe` feeds all five signal classes | `qip-deepbrain` `main.rs` → `node.rs` → `Operations::observe` | `qip-deepbrain/tests/research.rs::signals_of_all_five_classes_attach_to_the_shared_deployment_entity`; `qip-observability/tests/aiops.rs::five_signal_classes_about_one_instrument_are_reachable_from_its_single_node` | present |
| SLOs | `qip_observability::slo`, `critical_paths::evaluate_where` + `record` in `Operations::observe`; kernel `blueprint_objectives` | as above, every iteration | `qip-acceptance/tests/slo.rs`; `qip-kernel/tests/self_observation.rs` | present |
| Anomaly detection | `aiops::detect_level_shift` over `qip_deepbrain_iteration_ms`; kernel `self_watch` | as above | `research.rs::a_level_shift_in_iteration_time_opens_one_incident_and_the_runbook_sheds_research` | present |
| Incident triage | `aiops::triage`, `EntityGraph::implicated`, `rank_incidents`; `Operations::open_incident` (ADR 0460) | as above, on every incident | `aiops.rs::a_trading_facing_incident_ranks_above_an_uncontained_one_and_a_contained_one_last`, `aiops.rs::the_owner_is_the_most_implicated_entity_in_the_window_and_the_process_when_none_is`, `research.rs::a_contained_shift_with_nothing_trading_facing_is_minor_and_owned_by_the_deep_brain`, `research.rs::a_venue_misbehaving_in_the_window_makes_the_incident_critical_and_owns_it` | present since this lane |
| Auto-remediation | `aiops::Remediator` + `CapabilitySwitch`, runbook-only (ADR 0213) | as above | `research.rs::an_incident_journals_the_authorising_record_before_the_effect_and_the_outcome_after` and its neighbours | present (one runbook, research shedding only) |
| Deployment verification | `aiops::release_gate`, `Platform::release_decision` | **none** — reached only from `qip-kernel/tests/self_observation.rs` | `self_observation.rs` (library-level only) | **missing a root caller** |
| Autonomous test loops | `Operations::check_fixtures` / `check_fixture` (ADR 0460) | as above, every `SHIFT_HALF` iterations | `research.rs::the_test_loop_re_runs_each_fixture_and_reports_whether_its_shift_has_cleared` | present since this lane |
