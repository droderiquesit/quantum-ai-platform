//! The Vertex AI adapter against a real socket.
//!
//! Every test here scripts a loopback service and asserts on what the adapter
//! did with the answer, because the whole of this module's value is in what it
//! does when the answer is *not* a job record. The refusing port that preceded
//! it was tested for refusing; this suite is for the two things a real client
//! can get wrong, and one of them is much worse than the other:
//!
//! * Reporting a state the service did not send. That writes a model card
//!   recording a training run that never happened, and a model card is the
//!   artefact nobody re-derives.
//! * Reporting an error for a job that exists. That is recoverable, and it is
//!   the direction every ambiguity here is resolved in.
//!
//! Each test names its own premise as an assertion rather than assuming it: the
//! adapter is connected, the service was asked, the job was tracked.

#![allow(clippy::panic_in_result_fn)]

mod server;

use qip_core::error::Result;
use qip_core::{JobId, Timestamp};
use qip_training::dataset::TrainingDataset;
use qip_training::job::{JobState, TrainingJob, TrainingProvider, TrainingSpec};
use qip_training::local::ModelFamily;
use qip_training::vertex::{
    VertexAccessToken, VertexAiConfig, VertexAiProvider, VertexTransport, VertexWorkload,
    WorkloadIdentityBinding,
};
use server::{Action, Route, TestService};
use std::time::Duration as StdDuration;

const TOKEN: &str = "ya29.a-token-nobody-logs";
const PROJECT: &str = "qip-research";
const REGION: &str = "europe-west4";

fn at() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn config() -> VertexAiConfig {
    VertexAiConfig {
        project_id: PROJECT.to_string(),
        region: REGION.to_string(),
        staging_bucket: "gs://qip-research-staging".to_string(),
        workload: VertexWorkload::CustomContainer {
            image_uri: "europe-west4-docker.pkg.dev/qip-research/trainers/boosted:1".to_string(),
            machine_type: "n1-standard-8".to_string(),
            accelerator: None,
        },
        workload_identity: WorkloadIdentityBinding {
            kubernetes_service_account: "qip-training".to_string(),
            google_service_account: "qip-training@qip-research.iam.gserviceaccount.com".to_string(),
            roles: vec!["roles/aiplatform.user".to_string()],
        },
    }
}

fn spec() -> TrainingSpec {
    TrainingSpec::new(
        "edge-teacher",
        "3",
        "quant-research",
        "book-pressure-2026",
        ModelFamily::boosted(),
    )
}

/// The display name `spec()` always produces, computed the same way the adapter
/// does. Written out here rather than imported so a change to the derivation
/// shows up as a failing test rather than as two functions agreeing with each
/// other.
const DISPLAY_NAME: &str = "edge-teacher-3-6840123409045651457";

fn dataset() -> Result<TrainingDataset> {
    let rows: Vec<Vec<f64>> = (0..40)
        .map(|i| vec![f64::from(i), f64::from(i) * 0.5])
        .collect();
    let targets: Vec<f64> = (0..40).map(|i| f64::from(i) * 0.25).collect();
    let times: Vec<Timestamp> = (0..40)
        .map(|i| at().saturating_add(qip_core::Duration::from_secs(i64::from(i))))
        .collect();
    TrainingDataset::new(
        "book-pressure-2026",
        vec!["imbalance".to_string(), "spread".to_string()],
        rows,
        targets,
        times,
    )
}

fn connected(base_url: &str) -> Result<VertexAiProvider> {
    let mut transport = VertexTransport::new(base_url, VertexAccessToken::new(TOKEN)?);
    // Shorter than the production default, so the tests that need a peer to go
    // quiet reach the real read timeout in seconds rather than in twenty of
    // them. The path under test is the same one; only the bound differs.
    transport.limits.read_timeout = StdDuration::from_millis(500);
    VertexAiProvider::connected(config(), transport)
}

fn resource(id: &str) -> String {
    format!("projects/{PROJECT}/locations/{REGION}/customJobs/{id}")
}

fn job_body(id: &str, state: &str) -> String {
    format!(
        r#"{{"name":"{}","displayName":"{DISPLAY_NAME}","state":"{state}"}}"#,
        resource(id)
    )
}

fn jobs_path() -> String {
    format!("/v1/projects/{PROJECT}/locations/{REGION}/customJobs")
}

// --- what the service said ---------------------------------------------------

#[test]
fn a_submitted_job_reports_the_state_vertex_stated_and_nothing_else() -> Result<()> {
    let service = TestService::routed(vec![Route::new(
        "POST",
        &jobs_path(),
        Action::json(200, job_body("991", "JOB_STATE_QUEUED")),
    )]);
    let mut provider = connected(&service.url())?;

    // The premise: a connected adapter with a complete configuration is
    // available. Everything below is about what it does, not about whether it
    // would refuse first.
    assert!(
        provider.is_available(),
        "the adapter under test must be reachable: {}",
        provider.requirement()
    );

    let job = provider.submit(spec(), &dataset()?, at())?;
    assert_eq!(
        job.state,
        JobState::Queued,
        "the state must be the one the service sent"
    );
    assert_eq!(
        job.id.as_str(),
        "991",
        "the platform's job id is the service's own, so there is no mapping to lose"
    );
    assert_eq!(job.provider, "vertex-ai");
    assert_eq!(job.rows, 40);
    assert_eq!(provider.stats().jobs_created, 1);
    assert!(provider.unresolved_submissions().is_empty());

    // The bearer token travelled in a header and never in the URL, because a
    // URL is written to every access log on the path.
    let submits = service.requests_to("POST", &jobs_path());
    assert_eq!(submits.len(), 1, "exactly one job was created");
    assert_eq!(
        submits[0].header("authorization"),
        Some(format!("Bearer {TOKEN}").as_str())
    );
    assert!(
        !submits[0].target.contains(TOKEN),
        "the token reached the request target: {}",
        submits[0].target
    );
    assert!(
        submits[0].body.contains(DISPLAY_NAME),
        "the submit must carry the display name a reconciliation filters on: {}",
        submits[0].body
    );
    assert!(
        submits[0].body.contains("n1-standard-8"),
        "the submit must describe the machine the fit runs on: {}",
        submits[0].body
    );
    Ok(())
}

#[test]
fn a_submit_the_service_never_answers_is_unresolved_rather_than_succeeded() -> Result<()> {
    // The peer accepts the connection and says nothing for longer than the
    // adapter will wait. Whether the job was created is exactly what nobody
    // knows.
    let service = TestService::routed(vec![Route::new(
        "POST",
        &jobs_path(),
        Action::Silent(StdDuration::from_secs(60)),
    )]);
    let mut provider = connected(&service.url())?;

    let error = provider
        .submit(spec(), &dataset()?, at())
        .expect_err("an unanswered submit was reported as a job");
    assert!(
        !error.message().contains("succeeded"),
        "{}",
        error.message()
    );

    let unresolved = provider.unresolved_submissions();
    assert_eq!(
        unresolved.len(),
        1,
        "the submission nobody could read must be recorded, not discarded"
    );
    assert_eq!(unresolved[0].display_name, DISPLAY_NAME);
    assert!(
        unresolved[0].reason.contains("may never have been created"),
        "the record must say what is unknown about it: {}",
        unresolved[0].reason
    );
    assert_eq!(provider.stats().jobs_created, 0);
    assert_eq!(provider.stats().entered_unresolved, 1);
    assert!(
        provider.tracked(&JobId::from_string("991")).is_none(),
        "no job may be recorded from a submit nobody read an answer to"
    );
    Ok(())
}

#[test]
fn a_poll_that_cannot_be_read_leaves_the_last_stated_state_untouched() -> Result<()> {
    let service = TestService::routed(vec![
        Route::new(
            "POST",
            &jobs_path(),
            Action::json(200, job_body("991", "JOB_STATE_RUNNING")),
        ),
        Route::new(
            "GET",
            &format!("/v1/{}", resource("991")),
            Action::json(503, r#"{"error":{"message":"backend unavailable"}}"#),
        ),
    ]);
    let mut provider = connected(&service.url())?;
    let submitted = provider.submit(spec(), &dataset()?, at())?;

    // The premise: the service stated Running, and that is what is recorded.
    assert_eq!(submitted.state, JobState::Running);

    let error = provider
        .poll(&submitted.id, at())
        .expect_err("a 503 was read as a job state");
    assert_eq!(error.code(), "unavailable", "{}", error.message());

    let tracked: &TrainingJob = provider
        .tracked(&submitted.id)
        .expect("the job is still tracked");
    assert_eq!(
        tracked.state,
        JobState::Running,
        "an unreadable poll must not advance, retreat or clear a state the service stated"
    );
    let stale = provider.stale_jobs();
    assert_eq!(stale.len(), 1, "the job must be marked as not trustworthy");
    assert_eq!(stale[0].0, submitted.id);
    assert!(
        stale[0].1.contains("may no longer be true"),
        "the mark must say what is doubtful about it: {}",
        stale[0].1
    );
    Ok(())
}

#[test]
fn a_state_this_decoder_cannot_name_is_never_defaulted_to_running_or_succeeded() -> Result<()> {
    let service = TestService::routed(vec![Route::new(
        "POST",
        &jobs_path(),
        Action::json(200, job_body("991", "JOB_STATE_TELEPORTING")),
    )]);
    let mut provider = connected(&service.url())?;

    let error = provider
        .submit(spec(), &dataset()?, at())
        .expect_err("an unknown state was mapped onto a JobState");
    assert_eq!(error.code(), "schema", "{}", error.message());
    assert!(
        error.message().contains("model card entry nobody wrote"),
        "the refusal must say why a default here is unacceptable: {}",
        error.message()
    );
    assert_eq!(provider.stats().unmappable_states, 1);
    assert_eq!(
        provider.unresolved_submissions().len(),
        1,
        "a job the service described in terms this adapter cannot record still exists"
    );
    Ok(())
}

#[test]
fn a_partially_succeeded_job_is_refused_rather_than_rounded_to_a_success() -> Result<()> {
    let service = TestService::routed(vec![Route::new(
        "POST",
        &jobs_path(),
        Action::json(200, job_body("991", "JOB_STATE_PARTIALLY_SUCCEEDED")),
    )]);
    let mut provider = connected(&service.url())?;

    let error = provider
        .submit(spec(), &dataset()?, at())
        .expect_err("a partial success was recorded as one of the five states");
    assert!(
        error
            .message()
            .contains("partial fit on a model card as a whole one"),
        "{}",
        error.message()
    );
    assert!(
        error
            .message()
            .contains("discard work the service says happened"),
        "the refusal must be symmetric about both wrong answers: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn a_cancel_reports_the_state_the_service_reached_and_not_the_one_requested() -> Result<()> {
    let service = TestService::routed(vec![
        Route::new(
            "POST",
            &jobs_path(),
            Action::json(200, job_body("991", "JOB_STATE_RUNNING")),
        ),
        Route::new(
            "POST",
            &format!("/v1/{}:cancel", resource("991")),
            // Vertex's cancel returns an empty body: it records the intent.
            Action::json(200, "{}"),
        ),
        Route::new(
            "GET",
            &format!("/v1/{}", resource("991")),
            Action::json(200, job_body("991", "JOB_STATE_CANCELLING")),
        ),
    ]);
    let mut provider = connected(&service.url())?;
    let submitted = provider.submit(spec(), &dataset()?, at())?;

    let cancelled = provider.cancel(&submitted.id, at())?;
    assert_eq!(
        cancelled.state,
        JobState::Running,
        "a job that is cancelling has not stopped, and reporting Cancelled here would claim a \
         terminal state the service has not reached"
    );
    assert!(
        !cancelled.state.is_terminal(),
        "nothing downstream may read an artifact from a job that is still running"
    );

    // The intent did reach the service, which is the other half of the claim.
    assert_eq!(
        service
            .requests_to("POST", &format!("/v1/{}:cancel", resource("991")))
            .len(),
        1
    );
    Ok(())
}

// --- the way out of unresolved ------------------------------------------------

#[test]
fn an_unresolved_submission_is_resolved_only_by_a_job_the_service_names() -> Result<()> {
    let filter_path = jobs_path();
    let service = TestService::routed(vec![
        Route::new(
            "POST",
            &filter_path,
            Action::Silent(StdDuration::from_secs(60)),
        ),
        Route::in_turn(
            "GET",
            &filter_path,
            vec![
                // An index that has not caught up. Absence is not evidence.
                Action::json(200, r#"{"customJobs":[]}"#),
                Action::json(
                    200,
                    format!(
                        r#"{{"customJobs":[{}]}}"#,
                        job_body("991", "JOB_STATE_RUNNING")
                    ),
                ),
            ],
        ),
    ]);
    let mut provider = connected(&service.url())?;

    provider
        .submit(spec(), &dataset()?, at())
        .expect_err("the premise is a submit that could not be read");
    assert_eq!(provider.unresolved_submissions().len(), 1);

    let none = provider.reconcile(DISPLAY_NAME, at())?;
    assert!(none.is_empty());
    assert_eq!(
        provider.unresolved_submissions().len(),
        1,
        "an empty list may be an index catching up; treating it as absence is how a running, \
         billing job becomes invisible"
    );
    assert_eq!(provider.stats().reconciled, 0);

    let found = provider.reconcile(DISPLAY_NAME, at())?;
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].state, JobState::Running);
    assert_eq!(found[0].id.as_str(), "991");
    assert!(
        provider.unresolved_submissions().is_empty(),
        "a submission the service named is no longer unresolved"
    );
    assert_eq!(provider.stats().reconciled, 1);

    // And the reconciled job is a job like any other: pollable, because its
    // resource name came from the service.
    assert!(provider.tracked(&found[0].id).is_some());

    // The filter went in the query string, encoded, and the token did not.
    let lists = service.requests_to("GET", &filter_path);
    assert!(!lists.is_empty());
    assert!(
        lists[0].target.contains("filter=") && lists[0].target.contains("display_name"),
        "the reconciliation must ask the service for this display name: {}",
        lists[0].target
    );
    assert!(!lists[0].target.contains(TOKEN));
    Ok(())
}

#[test]
fn abandoning_an_unresolved_submission_needs_a_person_and_asserts_nothing() -> Result<()> {
    let service = TestService::routed(vec![Route::new(
        "POST",
        &jobs_path(),
        Action::Silent(StdDuration::from_secs(60)),
    )]);
    let mut provider = connected(&service.url())?;
    provider
        .submit(spec(), &dataset()?, at())
        .expect_err("the premise is a submit that could not be read");

    let error = provider
        .abandon_unresolved(DISPLAY_NAME, "  ")
        .expect_err("an unattributed abandonment was accepted");
    assert!(
        error.message().contains("audit trail"),
        "{}",
        error.message()
    );
    assert_eq!(provider.unresolved_submissions().len(), 1);

    provider.abandon_unresolved(DISPLAY_NAME, "quant-research-oncall")?;
    assert!(provider.unresolved_submissions().is_empty());
    assert!(
        provider.tracked(&JobId::from_string("991")).is_none(),
        "abandoning records no job, no state and no outcome, because nobody knows one"
    );
    Ok(())
}

// --- the credential -----------------------------------------------------------

#[test]
fn an_expired_token_is_a_refusal_naming_the_token_and_never_a_job_state() -> Result<()> {
    let service = TestService::routed(vec![Route::new(
        "POST",
        &jobs_path(),
        Action::json(401, r#"{"error":{"message":"Invalid Credentials"}}"#),
    )]);
    let mut provider = connected(&service.url())?;

    let error = provider
        .submit(spec(), &dataset()?, at())
        .expect_err("a 401 was read as a job state");
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

    // Conservative on purpose: a 401 almost certainly created nothing, and
    // "almost certainly" is not a basis for deciding whether a training run
    // exists.
    assert_eq!(provider.unresolved_submissions().len(), 1);
    Ok(())
}

#[test]
fn a_blank_token_is_absent_rather_than_empty() {
    let error = VertexAccessToken::new("   ").expect_err("a blank token was accepted");
    assert!(
        error.message().contains("empty Authorization header"),
        "{}",
        error.message()
    );
    let control = VertexAccessToken::new("ya29.tok\r\nx-admin: yes")
        .expect_err("a header-splitting token was accepted");
    assert!(
        control.message().contains("control character"),
        "{}",
        control.message()
    );
}

#[test]
fn the_token_is_redacted_everywhere_it_could_be_printed() -> Result<()> {
    let service = TestService::routed(vec![]);
    let provider = connected(&service.url())?;

    let rendered = format!("{provider:?}");
    assert!(
        !rendered.contains(TOKEN),
        "the provider's Debug leaked the token: {rendered}"
    );
    assert!(
        rendered.contains("<redacted>"),
        "the token's presence is worth knowing; its value never is: {rendered}"
    );
    assert!(
        !provider.requirement().contains(TOKEN),
        "the requirement text leaked the token"
    );
    Ok(())
}

#[test]
fn an_https_endpoint_is_refused_because_this_transport_has_no_tls() -> Result<()> {
    // Refused at construction rather than on the first submit, and refused
    // rather than downgraded: the alternative is a bearer token crossing the
    // internet in clear text.
    let error = VertexAiProvider::connected(
        config(),
        VertexTransport::new(
            "https://europe-west4-aiplatform.googleapis.com",
            VertexAccessToken::new(TOKEN)?,
        ),
    )
    .expect_err("an https endpoint was accepted");

    assert!(
        error.message().contains("TLS-terminating egress proxy"),
        "the refusal must say what a deployment has to put there instead: {}",
        error.message()
    );
    assert!(
        error.message().contains("clear text"),
        "{}",
        error.message()
    );
    Ok(())
}

// --- what a working connection still does not buy -------------------------------

#[test]
fn a_job_vertex_reports_as_succeeded_still_has_no_artifact_here() -> Result<()> {
    let service = TestService::routed(vec![Route::new(
        "POST",
        &jobs_path(),
        Action::json(200, job_body("991", "JOB_STATE_SUCCEEDED")),
    )]);
    let mut provider = connected(&service.url())?;
    let job = provider.submit(spec(), &dataset()?, at())?;

    // The premise: the service really did say the fit finished.
    assert_eq!(job.state, JobState::Succeeded);
    assert!(job.state.produced_an_artifact());

    let error = provider
        .artifact(&job.id)
        .expect_err("a TrainedTeacher was produced for a fit this process did not run");
    assert_eq!(error.code(), "unavailable", "{}", error.message());
    assert!(
        error.message().contains("gs://qip-research-staging"),
        "the refusal must say where the model actually is: {}",
        error.message()
    );
    assert!(
        error
            .message()
            .contains("record this platform's model against a run Vertex performed"),
        "the refusal must say why fabricating one is the failure: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn a_connected_adapter_still_reports_what_production_owes() -> Result<()> {
    let service = TestService::routed(vec![]);
    let provider = connected(&service.url())?;

    assert!(provider.is_available(), "{}", provider.requirement());
    assert!(
        provider.missing().is_empty(),
        "nothing configurable is outstanding: {:?}",
        provider.missing()
    );

    // And the requirement text is still not empty, because an available
    // adapter is not a finished deployment.
    let requirement = provider.requirement();
    for fragment in [
        "TLS-terminating egress proxy",
        "mints and refreshes the OAuth2 access token",
        "alert on the count of unresolved submissions",
        "importer for Vertex's model format",
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
fn an_answer_about_another_job_is_not_a_state_for_this_one() -> Result<()> {
    let service = TestService::routed(vec![Route::new(
        "POST",
        &jobs_path(),
        Action::json(
            200,
            format!(
                r#"{{"name":"{}","displayName":"someone-elses-run-1","state":"JOB_STATE_SUCCEEDED"}}"#,
                resource("991")
            ),
        ),
    )]);
    let mut provider = connected(&service.url())?;

    let error = provider
        .submit(spec(), &dataset()?, at())
        .expect_err("a record for another job was adopted as this one's state");
    assert!(
        error
            .message()
            .contains("someone else's run on this model's card"),
        "{}",
        error.message()
    );
    assert!(provider.tracked(&JobId::from_string("991")).is_none());
    Ok(())
}

#[test]
fn a_port_with_no_transport_still_refuses_every_call() -> Result<()> {
    // The behaviour that predates the transport, kept: an adapter built the
    // old way opens nothing and says what it is missing. A listener stands by
    // so "opens nothing" is a measurement rather than a claim.
    let service = TestService::routed(vec![Route::new(
        "POST",
        &jobs_path(),
        Action::json(200, job_body("991", "JOB_STATE_SUCCEEDED")),
    )]);
    let mut provider = VertexAiProvider::with_credentials(config(), true);
    assert!(!provider.is_available());
    assert!(!provider.has_transport());

    let error = provider
        .submit(spec(), &dataset()?, at())
        .expect_err("a provider with no transport submitted a job");
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
