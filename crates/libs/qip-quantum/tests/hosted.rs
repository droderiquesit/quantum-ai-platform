//! The hosted quantum adapter against a real socket.
//!
//! One property dominates this file, and it is the one
//! [`ProviderCapabilities::simulated`] exists for: **a hardware result and a
//! simulated result must stay distinguishable.** The adapter reports
//! `simulated: false`, so the only way that stays true is for it never to
//! return a result the service attributed to a simulator — and the only way to
//! test that is against a service that says it is one.
//!
//! The rest follows from the same suspicion of a remote peer. The energy in a
//! response is a number somebody else computed, so it is recomputed here from
//! the assignment; a job that never finishes inside the polling budget produces
//! the job's identifier rather than an answer; a status nobody recognises is
//! not "still running".
//!
//! Every test names its own premise as an assertion rather than assuming it,
//! and the queue is driven by a recording sleeper so a bounded wait costs no
//! wall-clock time.

#![allow(clippy::panic_in_result_fn)]

mod server;

use qip_core::error::Result;
use qip_core::time::Duration;
use qip_numerics::anneal::Qubo;
use qip_quantum::benchmark::SolverBenchmark;
use qip_quantum::provider::{
    HostedConfig, HostedProvider, HostedToken, HostedTransport, ProviderCapabilities,
    QuantumProvider, SimulatedProvider,
};
use qip_quantum::qaoa::QaoaSettings;
use qip_quantum::solver::{ClassicalSolver, ProviderSolver, QuboSolver, SolverKind};
use qip_transport::retry::RecordingSleeper;
use server::{Action, Route, TestService};
use std::sync::Arc;
use std::time::Duration as StdDuration;

const TOKEN: &str = "ibm-api-token-nobody-logs";
const BACKEND: &str = "ibm_torino";

fn config() -> HostedConfig {
    HostedConfig {
        vendor: "ibm-quantum".to_string(),
        backend: BACKEND.to_string(),
        credential_env: "QIP_QUANTUM_TOKEN".to_string(),
        endpoint: "https://quantum.cloud.ibm.com/api".to_string(),
        max_qubits: 133,
        cost_per_job_micros: 1_500_000,
    }
}

/// A four-variable problem whose optimum is easy to reason about: every
/// variable carries a negative linear term, so turning all of them on is best.
fn problem() -> Qubo {
    let mut qubo = Qubo::new(4);
    for i in 0..4 {
        qubo.add_linear(i, -1.0);
    }
    qubo.add(0, 1, 0.5);
    qubo
}

fn connected(base_url: &str) -> Result<(HostedProvider, Arc<RecordingSleeper>)> {
    let sleeper = Arc::new(RecordingSleeper::new());
    let mut transport = HostedTransport::new(base_url, HostedToken::new(TOKEN)?);
    transport.sleeper = sleeper.clone();
    transport.poll_interval = Duration::from_secs(5);
    transport.max_polls = 4;
    // Shorter than the production default, so the test that needs the peer to
    // go quiet reaches the real read timeout in half a second rather than in
    // twenty. The path under test is the same one; only the bound differs.
    transport.limits.read_timeout = StdDuration::from_millis(500);
    Ok((HostedProvider::connected(config(), transport)?, sleeper))
}

fn backend_path() -> String {
    format!("/api/v1/backends/{BACKEND}/configuration")
}

fn hardware_configuration() -> Action {
    Action::json(
        200,
        format!(r#"{{"backend_name":"{BACKEND}","simulator":false,"n_qubits":133}}"#),
    )
}

fn simulator_configuration() -> Action {
    Action::json(
        200,
        format!(r#"{{"backend_name":"{BACKEND}","simulator":true,"n_qubits":32}}"#),
    )
}

fn submitted(id: &str) -> Action {
    Action::json(200, format!(r#"{{"id":"{id}","backend":"{BACKEND}"}}"#))
}

fn job_status(id: &str, status: &str) -> Action {
    Action::json(
        200,
        format!(r#"{{"id":"{id}","backend":"{BACKEND}","state":{{"status":"{status}"}}}}"#),
    )
}

/// A result for [`problem`]: every bit on. The objective is
/// `-1 -1 -1 -1 + 0.5 = -3.5`, which the adapter recomputes rather than trusts.
fn completed_result(energy: &str) -> Action {
    Action::json(
        200,
        format!(
            r#"{{"assignment":[1,1,1,1],"energy":{energy},"expectation":-2.9,
                "success_probability":0.42,"layers":3,"angles":[0.1,0.2,0.3,0.4,0.5,0.6],
                "evaluations":300}}"#
        ),
    )
}

/// The four routes a successful run needs, in the order it needs them.
fn working_routes(id: &str, energy: &str) -> Vec<Route> {
    vec![
        Route::new("GET", &backend_path(), hardware_configuration()),
        Route::new("POST", "/api/v1/jobs", submitted(id)),
        Route::in_turn(
            "GET",
            &format!("/api/v1/jobs/{id}"),
            vec![
                job_status(id, "Queued"),
                job_status(id, "Running"),
                job_status(id, "Completed"),
            ],
        ),
        Route::new(
            "GET",
            &format!("/api/v1/jobs/{id}/results"),
            completed_result(energy),
        ),
    ]
}

// --- the flag that keeps the two kinds of result apart -------------------------

#[test]
fn a_backend_the_service_calls_a_simulator_produces_no_result_at_all() -> Result<()> {
    let service = TestService::routed(vec![
        Route::new("GET", &backend_path(), simulator_configuration()),
        // Scripted so that a submit, if one happened, would succeed. The test's
        // claim is that it does not happen.
        Route::new("POST", "/api/v1/jobs", submitted("job-1")),
    ]);
    let (provider, _sleeper) = connected(&service.url())?;

    // The premise: this adapter says it is not a simulator, and everything
    // downstream keys off that.
    let capabilities: ProviderCapabilities = provider.capabilities();
    assert!(!capabilities.simulated);

    let error = provider
        .solve_qubo(&problem(), &QaoaSettings::default())
        .expect_err("a simulated result was returned by a provider claiming hardware");

    assert_eq!(error.code(), "guard", "{}", error.message());
    assert!(
        error
            .message()
            .contains("indistinguishable from a hardware one"),
        "the refusal must say what would go wrong: {}",
        error.message()
    );
    assert!(
        error.message().contains("SimulatedProvider"),
        "the refusal must name the provider that does report itself as a simulator: {}",
        error.message()
    );
    assert_eq!(provider.stats().simulator_refusals, 1);

    // Nothing was submitted, so no simulated result exists anywhere for
    // something downstream to mislabel.
    assert_eq!(provider.stats().jobs_submitted, 0);
    assert!(
        service.requests_to("POST", "/api/v1/jobs").is_empty(),
        "a job was submitted to a device this adapter had already decided to refuse"
    );

    // And the capabilities did not quietly change under the caller.
    assert!(
        !provider.capabilities().simulated,
        "the flag must not flip in response to what the service said; refusing is the answer"
    );
    let device = provider
        .confirmed_device()
        .expect("the service was asked and answered");
    assert!(
        device.simulator,
        "the provenance is published even though the run was refused"
    );
    Ok(())
}

#[test]
fn a_hardware_result_carries_the_devices_own_provenance() -> Result<()> {
    let service = TestService::routed(working_routes("job-7", "-3.5"));
    let (provider, sleeper) = connected(&service.url())?;

    let result = provider.solve_qubo(&problem(), &QaoaSettings::default())?;
    assert_eq!(result.assignment, vec![1, 1, 1, 1]);
    assert!(
        (result.energy - -3.5).abs() < 1e-12,
        "the objective must be the one this process recomputed: {}",
        result.energy
    );
    assert_eq!(result.layers, 3);

    let device = provider
        .confirmed_device()
        .expect("a run confirms the device before it submits");
    assert_eq!(device.backend, BACKEND);
    assert!(
        !device.simulator,
        "the vendor's own statement is what makes this a hardware result"
    );
    assert_eq!(device.qubits, 133);

    let stats = provider.stats();
    assert_eq!(stats.device_confirmations, 1);
    assert_eq!(stats.jobs_submitted, 1);
    assert_eq!(stats.polls_sent, 3, "queued, running, then completed");
    assert_eq!(stats.results_accepted, 1);

    // The queue was waited on through the sleeper rather than the wall clock.
    assert_eq!(
        sleeper.recorded(),
        vec![Duration::from_secs(5), Duration::from_secs(5)],
        "one wait between each pair of polls, and none before the first"
    );

    // The token travelled in a header and never in a URL.
    let submits = service.requests_to("POST", "/api/v1/jobs");
    assert_eq!(submits.len(), 1);
    assert_eq!(
        submits[0].header("authorization"),
        Some(format!("Bearer {TOKEN}").as_str())
    );
    assert!(!submits[0].target.contains(TOKEN));
    assert!(
        submits[0].body.contains("\"n\":4"),
        "the job must carry the problem so the result can be checked against it: {}",
        submits[0].body
    );
    Ok(())
}

#[test]
fn the_device_is_confirmed_once_rather_than_on_every_run() -> Result<()> {
    let service = TestService::routed(working_routes("job-7", "-3.5"));
    let (provider, _sleeper) = connected(&service.url())?;

    provider.solve_qubo(&problem(), &QaoaSettings::default())?;
    // The job route's last action repeats, so a second run completes on its
    // first poll.
    provider.solve_qubo(&problem(), &QaoaSettings::default())?;

    assert_eq!(provider.stats().jobs_submitted, 2);
    assert_eq!(
        provider.stats().device_confirmations,
        1,
        "a device does not become a simulator between two jobs"
    );
    assert_eq!(service.requests_to("GET", &backend_path()).len(), 1);
    Ok(())
}

// --- the peer is not trusted ----------------------------------------------------

#[test]
fn a_claimed_objective_that_does_not_match_its_assignment_is_refused() -> Result<()> {
    // The same bits, with a flattering number attached. Recomputing costs one
    // pass over the QUBO and catches what the number alone cannot show.
    let service = TestService::routed(working_routes("job-8", "-99.0"));
    let (provider, _sleeper) = connected(&service.url())?;

    let error = provider
        .solve_qubo(&problem(), &QaoaSettings::default())
        .expect_err("a claim that does not match its assignment was recorded");

    assert_eq!(error.code(), "numeric", "{}", error.message());
    assert!(
        error.message().contains("-3.5") && error.message().contains("-99"),
        "the refusal must show both numbers: {}",
        error.message()
    );
    assert_eq!(provider.stats().arithmetic_refusals, 1);
    assert_eq!(provider.stats().results_accepted, 0);
    Ok(())
}

#[test]
fn an_assignment_of_the_wrong_length_scores_a_different_problem() -> Result<()> {
    let id = "job-9";
    let mut routes = working_routes(id, "-3.5");
    routes.pop();
    routes.push(Route::new(
        "GET",
        &format!("/api/v1/jobs/{id}/results"),
        Action::json(
            200,
            r#"{"assignment":[1,1],"energy":-2.0,"expectation":-1.5,
                "success_probability":0.3,"angles":[],"evaluations":10}"#,
        ),
    ));
    let service = TestService::routed(routes);
    let (provider, _sleeper) = connected(&service.url())?;

    let error = provider
        .solve_qubo(&problem(), &QaoaSettings::default())
        .expect_err("a truncated assignment was accepted");
    assert!(
        error
            .message()
            .contains("2 bit(s) for a 4-variable problem"),
        "{}",
        error.message()
    );
    Ok(())
}

#[test]
fn a_job_that_does_not_finish_inside_the_budget_yields_its_identifier_not_an_answer() -> Result<()>
{
    let id = "job-10";
    let service = TestService::routed(vec![
        Route::new("GET", &backend_path(), hardware_configuration()),
        Route::new("POST", "/api/v1/jobs", submitted(id)),
        Route::new(
            "GET",
            &format!("/api/v1/jobs/{id}"),
            job_status(id, "Queued"),
        ),
    ]);
    let (provider, sleeper) = connected(&service.url())?;

    let error = provider
        .solve_qubo(&problem(), &QaoaSettings::default())
        .expect_err("an unfinished job produced a result");

    assert_eq!(error.code(), "timeout", "{}", error.message());
    assert!(
        error.message().contains(id),
        "an operator needs the identifier to do anything about it: {}",
        error.message()
    );
    assert!(
        error.message().contains("still billing"),
        "the refusal must say the job is still running and still costing: {}",
        error.message()
    );
    assert_eq!(provider.stats().polls_sent, 4, "the configured budget");
    assert_eq!(provider.stats().poll_budgets_exhausted, 1);
    assert_eq!(sleeper.recorded().len(), 3, "one wait between each poll");
    Ok(())
}

#[test]
fn a_service_that_goes_quiet_mid_poll_produces_an_error_and_never_a_result() -> Result<()> {
    // The job was accepted and then the peer stopped answering. Whether it
    // finished is exactly what nobody knows, and there is no partial result to
    // report — so the caller gets an error and the job is left where it is.
    let id = "job-15";
    let service = TestService::routed(vec![
        Route::new("GET", &backend_path(), hardware_configuration()),
        Route::new("POST", "/api/v1/jobs", submitted(id)),
        Route::new(
            "GET",
            &format!("/api/v1/jobs/{id}"),
            Action::Silent(StdDuration::from_secs(30)),
        ),
    ]);
    let (provider, _sleeper) = connected(&service.url())?;

    let error = provider
        .solve_qubo(&problem(), &QaoaSettings::default())
        .expect_err("a peer that went quiet produced a result");
    assert!(
        !error.message().contains("assignment"),
        "no answer may be synthesised from silence: {}",
        error.message()
    );
    assert_eq!(
        provider.stats().results_accepted,
        0,
        "nothing was read, so nothing may be counted as read"
    );
    assert_eq!(
        provider.stats().jobs_submitted,
        1,
        "the job really was submitted, which is why the silence matters"
    );
    Ok(())
}

#[test]
fn a_job_status_this_decoder_cannot_name_is_not_treated_as_still_running() -> Result<()> {
    let id = "job-11";
    let service = TestService::routed(vec![
        Route::new("GET", &backend_path(), hardware_configuration()),
        Route::new("POST", "/api/v1/jobs", submitted(id)),
        Route::new(
            "GET",
            &format!("/api/v1/jobs/{id}"),
            job_status(id, "Reticulating"),
        ),
    ]);
    let (provider, _sleeper) = connected(&service.url())?;

    let error = provider
        .solve_qubo(&problem(), &QaoaSettings::default())
        .expect_err("an unknown status was polled through");
    assert_eq!(error.code(), "schema", "{}", error.message());
    assert!(
        error.message().contains("hung thread and a bill"),
        "the refusal must say why defaulting to running is the expensive mistake: {}",
        error.message()
    );
    assert_eq!(
        provider.stats().polls_sent,
        1,
        "the loop stopped on the first unreadable answer rather than spending the budget"
    );
    Ok(())
}

#[test]
fn a_job_the_service_reports_as_failed_is_an_error_and_not_an_empty_result() -> Result<()> {
    let id = "job-12";
    let service = TestService::routed(vec![
        Route::new("GET", &backend_path(), hardware_configuration()),
        Route::new("POST", "/api/v1/jobs", submitted(id)),
        Route::new(
            "GET",
            &format!("/api/v1/jobs/{id}"),
            Action::json(
                200,
                format!(
                    r#"{{"id":"{id}","state":{{"status":"Failed","reason":"transpilation error"}}}}"#
                ),
            ),
        ),
    ]);
    let (provider, _sleeper) = connected(&service.url())?;

    let error = provider
        .solve_qubo(&problem(), &QaoaSettings::default())
        .expect_err("a failed job produced a result");
    assert!(
        error.message().contains("transpilation error"),
        "the service's own reason is what an operator reads first: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn a_backend_configuration_that_does_not_say_whether_it_is_a_simulator_is_unreadable() -> Result<()>
{
    // A missing flag is not `false`. The safe direction is refusing the whole
    // response, because the alternative is an unstated simulator reported as
    // hardware.
    let service = TestService::routed(vec![Route::new(
        "GET",
        &backend_path(),
        Action::json(
            200,
            format!(r#"{{"backend_name":"{BACKEND}","n_qubits":133}}"#),
        ),
    )]);
    let (provider, _sleeper) = connected(&service.url())?;

    let error = provider
        .solve_qubo(&problem(), &QaoaSettings::default())
        .expect_err("a configuration with no simulator flag was accepted");
    assert!(
        error.message().contains("will not assume one"),
        "{}",
        error.message()
    );
    assert!(provider.confirmed_device().is_none());
    Ok(())
}

#[test]
fn a_configuration_for_a_different_device_is_refused_before_anything_is_submitted() -> Result<()> {
    let service = TestService::routed(vec![
        Route::new(
            "GET",
            &backend_path(),
            Action::json(
                200,
                r#"{"backend_name":"ibm_kyiv","simulator":false,"n_qubits":127}"#,
            ),
        ),
        Route::new("POST", "/api/v1/jobs", submitted("job-13")),
    ]);
    let (provider, _sleeper) = connected(&service.url())?;

    let error = provider
        .solve_qubo(&problem(), &QaoaSettings::default())
        .expect_err("a device nobody chose was accepted");
    assert_eq!(error.code(), "guard", "{}", error.message());
    assert!(
        error.message().contains("coupling map"),
        "the refusal must say why one device is not another: {}",
        error.message()
    );
    assert!(service.requests_to("POST", "/api/v1/jobs").is_empty());
    Ok(())
}

#[test]
fn the_device_the_service_describes_bounds_the_problem_independently_of_configuration() -> Result<()>
{
    // Configured for 133 qubits; the service says this device has 4. A problem
    // larger than the hardware is refused by the number here rather than by the
    // vendor after a queue wait.
    let service = TestService::routed(vec![Route::new(
        "GET",
        &backend_path(),
        Action::json(
            200,
            format!(r#"{{"backend_name":"{BACKEND}","simulator":false,"n_qubits":2}}"#),
        ),
    )]);
    let (provider, _sleeper) = connected(&service.url())?;

    assert_eq!(
        provider.capabilities().max_qubits,
        133,
        "before the device is confirmed, the configured number is all there is"
    );
    let error = provider
        .solve_qubo(&problem(), &QaoaSettings::default())
        .expect_err("a problem larger than the device was submitted");
    assert!(
        error.message().contains("exceeds the 2 qubit(s)"),
        "{}",
        error.message()
    );
    assert_eq!(
        provider.capabilities().max_qubits,
        2,
        "once the service has said, its number is the smaller and therefore the one"
    );
    Ok(())
}

// --- the credential ---------------------------------------------------------------

#[test]
fn an_expired_token_is_a_refusal_naming_the_token_and_never_a_result() -> Result<()> {
    let service = TestService::routed(vec![Route::new(
        "GET",
        &backend_path(),
        Action::json(401, r#"{"errors":[{"message":"invalid credentials"}]}"#),
    )]);
    let (provider, _sleeper) = connected(&service.url())?;

    let error = provider
        .solve_qubo(&problem(), &QaoaSettings::default())
        .expect_err("a 401 produced a result");
    assert_eq!(error.code(), "denied", "{}", error.message());
    assert!(
        error.message().contains("cannot mint or refresh one"),
        "this adapter takes a token it is given, and the refusal has to say so: {}",
        error.message()
    );
    assert!(
        !error.message().contains(TOKEN),
        "the refusal quoted the token"
    );
    Ok(())
}

#[test]
fn the_token_is_redacted_everywhere_it_could_be_printed() -> Result<()> {
    let service = TestService::routed(vec![]);
    let (provider, _sleeper) = connected(&service.url())?;

    let rendered = format!("{provider:?}");
    assert!(
        !rendered.contains(TOKEN),
        "the provider's Debug leaked the token: {rendered}"
    );
    assert!(rendered.contains("<redacted>"), "{rendered}");
    assert!(!provider.requirement().contains(TOKEN));

    let blank = HostedToken::new("  ").expect_err("a blank token was accepted");
    assert!(
        blank.message().contains("absent rather than empty"),
        "{}",
        blank.message()
    );
    Ok(())
}

#[test]
fn an_https_endpoint_is_refused_because_this_transport_has_no_tls() -> Result<()> {
    let error = HostedProvider::connected(
        config(),
        HostedTransport::new(
            "https://quantum.cloud.ibm.com/api",
            HostedToken::new(TOKEN)?,
        ),
    )
    .expect_err("an https endpoint was accepted");

    assert!(
        error.message().contains("TLS-terminating egress proxy"),
        "{}",
        error.message()
    );
    assert!(
        error.message().contains("clear text"),
        "{}",
        error.message()
    );
    Ok(())
}

// --- what a working connection still does not buy -----------------------------------

#[test]
fn a_hardware_result_is_still_a_candidate_that_needs_a_classical_baseline() -> Result<()> {
    let service = TestService::routed(working_routes("job-14", "-3.5"));
    let (provider, _sleeper) = connected(&service.url())?;
    let provider = Arc::new(provider);

    // Through the same seam the compute router and the benchmark use.
    let solver = ProviderSolver::new(provider.clone(), QaoaSettings::default());
    assert_eq!(solver.kind(), SolverKind::Quantum);
    assert!(
        solver.kind().needs_a_classical_baseline(),
        "a device's answer has no more claim on being believed than a heuristic's"
    );

    let qubo = problem();
    let report = SolverBenchmark::new(ClassicalSolver::exhaustive(20))
        .with_solver(Arc::new(solver))
        .with_repeats(1)
        .run(&qubo)?;

    // The baseline is a non-optional field on the report, by type, and the
    // quantum entrant is measured against it rather than reported on its own.
    assert_eq!(report.classical_baseline.kind, SolverKind::Classical);
    assert!(report.classical_baseline.usable_solution().is_some());

    let device_record = report
        .record_for(BACKEND)
        .expect("the hosted provider entered the benchmark");
    assert_eq!(device_record.kind, SolverKind::Quantum);

    // And the only usable form of its answer is one that was re-evaluated
    // classically. There is no field holding an unvalidated result.
    let validated = device_record
        .usable_solution()
        .expect("the device's assignment re-scores to what it claimed");
    assert_eq!(validated.kind(), SolverKind::Quantum);
    assert_eq!(validated.assignment(), &[1, 1, 1, 1]);
    Ok(())
}

#[test]
fn a_simulated_provider_and_a_hosted_one_never_report_the_same_capabilities() -> Result<()> {
    let service = TestService::routed(vec![Route::new(
        "GET",
        &backend_path(),
        hardware_configuration(),
    )]);
    let (hosted, _sleeper) = connected(&service.url())?;
    let simulated = SimulatedProvider::new(7);

    assert!(
        simulated.capabilities().simulated,
        "the in-tree simulator says what it is"
    );
    assert!(
        !hosted.capabilities().simulated,
        "and the hosted adapter says what it is, enforced by refusing anything else"
    );
    assert!(!simulated.capabilities().noisy);
    assert!(hosted.capabilities().noisy);
    assert_eq!(simulated.capabilities().cost_per_job_micros, 0);
    assert!(hosted.capabilities().cost_per_job_micros > 0);
    Ok(())
}

#[test]
fn a_connected_adapter_still_reports_what_production_owes() -> Result<()> {
    let service = TestService::routed(vec![]);
    let (provider, _sleeper) = connected(&service.url())?;

    assert!(provider.is_available());
    assert!(provider.has_transport());

    let requirement = provider.requirement();
    for fragment in [
        "TLS-terminating egress proxy",
        "published runtime program",
        "alert on queue time",
        "classical baseline solved on the same problem",
    ] {
        assert!(
            requirement.contains(fragment),
            "the standing requirements omit {fragment}: {requirement}"
        );
    }
    assert!(
        requirement.contains("Nothing falls back"),
        "an operator must not read this as a path that degrades gracefully: {requirement}"
    );
    Ok(())
}

#[test]
fn a_port_with_no_transport_still_refuses_and_opens_nothing() -> Result<()> {
    // The behaviour that predates the transport, kept. A listener stands by so
    // "opens nothing" is a measurement rather than a claim.
    let service = TestService::routed(vec![Route::new(
        "GET",
        &backend_path(),
        hardware_configuration(),
    )]);
    let provider = HostedProvider::with_credential(config(), true);

    assert!(!provider.is_available());
    assert!(!provider.has_transport());
    let error = provider
        .solve_qubo(&problem(), &QaoaSettings::default())
        .expect_err("a provider with no transport ran a job");
    assert_eq!(error.code(), "unavailable", "{}", error.message());
    assert!(
        error.message().contains("HTTPS transport"),
        "{}",
        error.message()
    );
    assert_eq!(
        service.served(),
        0,
        "a port that reports itself unavailable must open no connection at all"
    );
    Ok(())
}
