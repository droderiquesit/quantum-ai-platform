//! The Quantum Gateway: submit-and-poll, never inline (R12-QUANT-7016).

#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_numerics::anneal::Qubo;
use qip_quantum::gateway::{Delivery, GatewayLimits, JobPoll, QuantumGateway};
use qip_quantum::provider::{ProviderCapabilities, QuantumProvider, SimulatedProvider};
use qip_quantum::qaoa::{QaoaResult, QaoaSettings};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Answers only when the test releases it, and counts its calls.
#[derive(Debug)]
struct Held {
    release: Mutex<Receiver<()>>,
    calls: AtomicUsize,
}

impl Held {
    fn new() -> (Arc<Self>, Sender<()>) {
        let (sender, receiver) = channel();
        (
            Arc::new(Self {
                release: Mutex::new(receiver),
                calls: AtomicUsize::new(0),
            }),
            sender,
        )
    }
}

impl QuantumProvider for Held {
    fn name(&self) -> &str {
        "held"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::simulator(8)
    }
    fn solve_qubo(&self, qubo: &Qubo, _settings: &QaoaSettings) -> Result<QaoaResult> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Ok(release) = self.release.lock() {
            let _ = release.recv();
        }
        let assignment = vec![0; qubo.n];
        Ok(QaoaResult {
            energy: qubo.evaluate(&assignment),
            assignment,
            expectation: 0.0,
            success_probability: 1.0,
            layers: 1,
            angles: vec![0.0, 0.0],
            evaluations: 1,
        })
    }
}

fn qubo() -> Qubo {
    let mut qubo = Qubo::new(3);
    qubo.entries.push((0, 0, -1.0));
    qubo.entries.push((1, 2, 0.5));
    qubo
}

fn wait_until_ready(gateway: &QuantumGateway, ticket: qip_quantum::JobTicket) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !gateway.is_ready(ticket)? {
        assert!(Instant::now() < deadline, "the worker never answered");
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

#[test]
fn an_inline_quantum_job_is_refused_and_the_provider_is_never_called() -> Result<()> {
    let (provider, _hold) = Held::new();
    let gateway = QuantumGateway::new(provider.clone(), GatewayLimits::default())?;

    let refusal = gateway
        .submit(
            "optimisation",
            Delivery::Inline,
            qubo(),
            QaoaSettings::default(),
        )
        .expect_err("an inline quantum job was admitted");
    assert_eq!(refusal.code(), "denied", "{}", refusal.message());
    assert!(
        refusal.message().contains("synchronous"),
        "{}",
        refusal.message()
    );

    // Premise: the same job polled is admitted, so the refusal above is the
    // delivery and not the job.
    let ticket = gateway.submit(
        "optimisation",
        Delivery::Polled,
        qubo(),
        QaoaSettings::default(),
    )?;
    let deadline = Instant::now() + Duration::from_secs(30);
    while provider.calls.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "the polled job never started");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        provider.calls.load(Ordering::SeqCst),
        1,
        "the refused inline job reached the provider"
    );
    let stats = gateway.stats()?;
    assert_eq!((stats.refused, stats.submitted), (1, 1));
    assert!(matches!(
        gateway.poll(ticket)?,
        JobPoll::Pending { polls: 1 }
    ));
    Ok(())
}

#[test]
fn a_job_outside_the_admitted_classes_is_refused_by_its_whole_name() -> Result<()> {
    let gateway = QuantumGateway::new(
        Arc::new(SimulatedProvider::new(1)),
        GatewayLimits::default(),
    )?;
    // A substring of an admitted class name is not that class.
    let refusal = gateway
        .submit(
            "optimisation-extra",
            Delivery::Polled,
            qubo(),
            QaoaSettings::default(),
        )
        .expect_err("an unadmitted class was accepted");
    assert!(
        refusal.message().contains("not admitted"),
        "{}",
        refusal.message()
    );
    Ok(())
}

#[test]
fn submitting_returns_before_the_provider_answers_and_a_poll_never_waits() -> Result<()> {
    let (provider, hold) = Held::new();
    let gateway = QuantumGateway::new(provider, GatewayLimits::default())?;
    let started = Instant::now();
    let ticket = gateway.submit(
        "optimisation",
        Delivery::Polled,
        qubo(),
        QaoaSettings::default(),
    )?;
    let poll = gateway.poll(ticket)?;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "submit and poll took {:?} against a provider that has not answered",
        started.elapsed()
    );
    assert!(matches!(poll, JobPoll::Pending { polls: 1 }), "{poll:?}");

    hold.send(())
        .map_err(|_| qip_core::error::Error::io("the provider hung up"))?;
    wait_until_ready(&gateway, ticket)?;
    match gateway.poll(ticket)? {
        JobPoll::Completed { polls, result } => {
            assert_eq!(polls, 1);
            assert_eq!(result?.assignment, vec![0, 0, 0]);
        }
        other => panic!("a returned job was reported as {other:?}"),
    }
    // Harvested once: a second poll is refused rather than reported pending.
    assert!(gateway.poll(ticket).is_err());
    Ok(())
}

#[test]
fn a_job_past_its_patience_is_abandoned_keeps_its_slot_and_its_late_answer_is_discarded()
-> Result<()> {
    let (provider, hold) = Held::new();
    let gateway = QuantumGateway::new(
        provider,
        GatewayLimits {
            max_in_flight: 1,
            patience_polls: 2,
        },
    )?;
    let ticket = gateway.submit(
        "optimisation",
        Delivery::Polled,
        qubo(),
        QaoaSettings::default(),
    )?;
    assert!(matches!(
        gateway.poll(ticket)?,
        JobPoll::Pending { polls: 1 }
    ));
    assert!(matches!(
        gateway.poll(ticket)?,
        JobPoll::Abandoned { polls: 2 }
    ));

    // The provider has not returned, so the slot is still held and a second
    // job is refused rather than stacked on a queue that is not moving.
    assert_eq!(gateway.in_flight()?, 1);
    let refusal = gateway
        .submit(
            "optimisation",
            Delivery::Polled,
            qubo(),
            QaoaSettings::default(),
        )
        .expect_err("a second job was stacked on an abandoned one");
    assert!(
        refusal.message().contains("occupied"),
        "{}",
        refusal.message()
    );

    // When the late answer lands it is discarded unread and the slot frees.
    hold.send(())
        .map_err(|_| qip_core::error::Error::io("the provider hung up"))?;
    wait_until_ready(&gateway, ticket)?;
    assert_eq!(gateway.in_flight()?, 0);
    let stats = gateway.stats()?;
    assert_eq!(
        (stats.abandoned, stats.discarded_late, stats.completed),
        (1, 1, 0)
    );
    assert!(
        gateway.poll(ticket).is_err(),
        "an abandoned job's answer was handed over"
    );
    Ok(())
}

#[test]
fn a_gateway_with_no_slots_or_no_patience_is_refused() {
    for limits in [
        GatewayLimits {
            max_in_flight: 0,
            patience_polls: 3,
        },
        GatewayLimits {
            max_in_flight: 1,
            patience_polls: 0,
        },
    ] {
        assert!(
            QuantumGateway::new(Arc::new(SimulatedProvider::new(1)), limits).is_err(),
            "{limits:?} was accepted"
        );
    }
}
