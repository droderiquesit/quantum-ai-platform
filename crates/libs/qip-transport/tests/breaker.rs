//! The circuit breaker: every transition, and the ones that must not happen.
//!
//! A breaker is a piece of code whose entire job is to refuse work, which
//! makes it the easiest component in a transport to get subtly wrong and never
//! notice. Both directions of wrongness are expensive. A breaker that opens
//! too eagerly stops a healthy peer being reached during the incident it was
//! meant to help with; a breaker that never closes turns a peer's brief outage
//! into a permanent one.
//!
//! So these tests drive the state machine rather than sampling it. Time is a
//! `ManualClock` the test advances deliberately, because a breaker tested
//! against wall time is a breaker whose test is a race.

#![allow(clippy::panic_in_result_fn)]

use qip_core::error::{Error, Result};
use qip_core::time::{Duration, Timestamp};
use qip_core::{Clock, ManualClock};
use qip_transport::breaker::{BreakerPolicy, BreakerState, CircuitBreaker, Outcome};
use std::sync::Arc;

const PEER: &str = "cell-london-1";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn clock() -> Arc<ManualClock> {
    Arc::new(ManualClock::new(start()))
}

/// A breaker that opens on the second consecutive failure and closes on one
/// good probe — small numbers, so a test reads as a sequence rather than a loop.
fn breaker(clock: Arc<ManualClock>) -> Result<CircuitBreaker> {
    CircuitBreaker::new(
        BreakerPolicy {
            failure_threshold: 2,
            success_threshold: 1,
            cooldown: Duration::from_secs(10),
            max_cooldown: Duration::from_secs(600),
            cooldown_multiplier: 4,
            // No jitter: these tests are about the machine, and the jitter has
            // its own test below where it is the subject rather than noise.
            jitter_basis_points: 0,
            concurrent_probes: 1,
        },
        clock as Arc<dyn Clock>,
        7,
        16,
    )
}

fn fail(breaker: &mut CircuitBreaker, peer: &str) -> Result<()> {
    let permit = breaker.admit(peer).permit().ok_or_else(|| {
        Error::invalid("the circuit refused a call the test expected it to admit")
    })?;
    breaker.record(permit, Outcome::failed(&"the peer is unreachable"));
    Ok(())
}

fn succeed(breaker: &mut CircuitBreaker, peer: &str) -> Result<()> {
    let permit = breaker.admit(peer).permit().ok_or_else(|| {
        Error::invalid("the circuit refused a call the test expected it to admit")
    })?;
    breaker.record(permit, Outcome::Success);
    Ok(())
}

// --- the state machine ------------------------------------------------------

#[test]
fn a_circuit_opens_only_after_the_threshold_and_then_refuses_without_calling() -> Result<()> {
    let time = clock();
    let mut breaker = breaker(Arc::clone(&time))?;

    // Below the threshold the circuit stays closed. A breaker that opened on
    // the first failure would trip on the ordinary transient the retry ladder
    // exists to absorb.
    fail(&mut breaker, PEER)?;
    assert_eq!(breaker.state(PEER), BreakerState::Closed);
    assert!(breaker.would_admit(PEER));

    fail(&mut breaker, PEER)?;
    assert_eq!(breaker.state(PEER), BreakerState::Open);

    // Open means refused without a call. That is the whole point: the peer is
    // down and the ladder is being spent proving it, message after message.
    let decision = breaker.admit(PEER);
    assert!(!decision.is_admitted(), "an open circuit admitted a call");
    let refusal = decision
        .refusal()
        .ok_or_else(|| Error::not_found("a refusal on an open circuit"))?;
    assert_eq!(refusal.code(), "circuit_open");
    assert_eq!(refusal.consecutive_failures, 2);
    assert!(
        refusal.retry_after > Duration::ZERO,
        "an open circuit gave no time to wait, so a caller cannot back off"
    );
    assert!(
        refusal.last_error.contains("unreachable"),
        "the refusal does not carry why the circuit opened: {}",
        refusal.last_error
    );
    Ok(())
}

#[test]
fn the_cooldown_must_actually_elapse_before_a_probe_is_admitted() -> Result<()> {
    let time = clock();
    let mut breaker = breaker(Arc::clone(&time))?;
    fail(&mut breaker, PEER)?;
    fail(&mut breaker, PEER)?;

    // One second short: still refused. A breaker that rounded this the
    // generous way would probe a peer that has had no time to recover.
    time.advance(Duration::from_secs(9));
    assert!(!breaker.would_admit(PEER));
    assert!(!breaker.admit(PEER).is_admitted());
    assert_eq!(breaker.state(PEER), BreakerState::Open);

    // `would_admit` is the question, not `state`. The circuit is ready to
    // offer a probe, and it still *reports* Open until something asks — see
    // the test below, which pins that on purpose.
    time.advance(Duration::from_secs(1));
    assert!(
        breaker.would_admit(PEER),
        "the cooldown elapsed and the circuit would still admit nothing"
    );
    let decision = breaker.admit(PEER);
    let permit = decision
        .permit()
        .ok_or_else(|| Error::not_found("a probe permit"))?;
    assert!(permit.is_probe(), "the admitted call is not marked a probe");
    assert_eq!(breaker.state(PEER), BreakerState::HalfOpen);
    Ok(())
}

#[test]
fn observing_a_breaker_never_advances_it() {
    // The transition from open to half-open happens on `admit`, not on
    // `state`. That is deliberate and worth pinning: a state machine that
    // advanced while being read would make two consecutive observations of an
    // idle breaker disagree, and an operator refreshing a health page would
    // be changing the thing they are looking at.
    let time = clock();
    let mut breaker = breaker(Arc::clone(&time)).expect("a valid policy");
    for _ in 0..2 {
        let permit = breaker.admit(PEER).permit().expect("admitted while closed");
        breaker.record(permit, Outcome::failed(&"down"));
    }
    time.advance(Duration::from_secs(60));

    let first = breaker.state(PEER);
    let second = breaker.state(PEER);
    assert_eq!(first, second, "two reads of an idle breaker disagreed");
    assert_eq!(first, BreakerState::Open, "a read advanced the machine");
    assert!(
        breaker.would_admit(PEER),
        "the circuit reports Open and would also admit nothing, which is a stuck breaker"
    );

    // The snapshot an operator reads agrees with the state, and carries the
    // instant that resolves the ambiguity for them.
    let snapshot = breaker.snapshot();
    let circuit = snapshot
        .iter()
        .find(|entry| entry.peer == PEER)
        .expect("the tracked peer");
    assert_eq!(circuit.state, BreakerState::Open);
    assert!(
        circuit.retry_at <= time.now(),
        "the snapshot says Open without showing that its retry instant has passed"
    );
}

#[test]
fn a_half_open_probe_that_fails_reopens_rather_than_closing() -> Result<()> {
    // The transition most likely to be written backwards, and the most
    // expensive to get wrong: a peer that is still down would be flooded the
    // moment its cooldown expired.
    let time = clock();
    let mut breaker = breaker(Arc::clone(&time))?;
    fail(&mut breaker, PEER)?;
    fail(&mut breaker, PEER)?;
    time.advance(Duration::from_secs(10));

    // Take the probe explicitly: this is the call that moves the circuit to
    // half-open, and the test is about what happens when that probe fails.
    let probe = breaker
        .admit(PEER)
        .permit()
        .ok_or_else(|| Error::not_found("a probe permit"))?;
    assert!(probe.is_probe());
    assert_eq!(breaker.state(PEER), BreakerState::HalfOpen);
    breaker.record(probe, Outcome::failed(&"still unreachable"));
    assert_eq!(
        breaker.state(PEER),
        BreakerState::Open,
        "a failing probe closed the circuit"
    );
    assert!(!breaker.admit(PEER).is_admitted());
    Ok(())
}

#[test]
fn a_half_open_probe_that_succeeds_closes_the_circuit_and_resets_the_count() -> Result<()> {
    let time = clock();
    let mut breaker = breaker(Arc::clone(&time))?;
    fail(&mut breaker, PEER)?;
    fail(&mut breaker, PEER)?;
    time.advance(Duration::from_secs(10));

    succeed(&mut breaker, PEER)?;
    assert_eq!(breaker.state(PEER), BreakerState::Closed);
    assert!(breaker.state(PEER).is_healthy());

    // The failure count reset with it. If it had not, the next single failure
    // would reopen a circuit that has just proved itself.
    fail(&mut breaker, PEER)?;
    assert_eq!(
        breaker.state(PEER),
        BreakerState::Closed,
        "one failure after recovery reopened the circuit; the count did not reset"
    );
    Ok(())
}

#[test]
fn a_half_open_circuit_admits_only_its_probe_budget_at_once() -> Result<()> {
    let time = clock();
    let mut breaker = breaker(Arc::clone(&time))?;
    fail(&mut breaker, PEER)?;
    fail(&mut breaker, PEER)?;
    time.advance(Duration::from_secs(10));

    let probe = breaker
        .admit(PEER)
        .permit()
        .ok_or_else(|| Error::not_found("the first probe"))?;

    // The second caller is refused while the first is outstanding — otherwise
    // "half open" would be "open to everyone", and the whole traffic would
    // arrive at a peer that has proved nothing yet.
    let second = breaker.admit(PEER);
    assert!(
        !second.is_admitted(),
        "a second probe was admitted concurrently"
    );
    let refusal = second
        .refusal()
        .ok_or_else(|| Error::not_found("a refusal"))?;
    assert_eq!(
        refusal.retry_after,
        Duration::ZERO,
        "a probe-slot refusal names a timer; it clears when the probe reports, not on a clock"
    );

    breaker.record(probe, Outcome::Success);
    assert!(breaker.admit(PEER).is_admitted());
    Ok(())
}

// --- the parts that keep an outage from becoming a self-inflicted one -------

#[test]
fn a_peer_that_stays_down_is_probed_less_and_less_often() -> Result<()> {
    // Without the multiplier, an outage measured in hours is probed every
    // cooldown for its whole duration, which is a slow denial of service
    // aimed at a peer that is already struggling.
    let time = clock();
    let mut breaker = breaker(Arc::clone(&time))?;
    fail(&mut breaker, PEER)?;
    fail(&mut breaker, PEER)?;

    let mut waits = Vec::new();
    for _ in 0..3 {
        let refusal_wait = breaker
            .admit(PEER)
            .refusal()
            .map(|refusal| refusal.retry_after)
            .ok_or_else(|| Error::not_found("a refusal while open"))?;
        waits.push(refusal_wait);
        time.advance(refusal_wait);
        // The probe fails: the peer is still down.
        fail(&mut breaker, PEER)?;
    }

    assert!(
        waits[1] > waits[0] && waits[2] > waits[1],
        "the cooldown did not grow across reopens: {waits:?}"
    );
    Ok(())
}

#[test]
fn the_cooldown_never_exceeds_its_ceiling() -> Result<()> {
    let time = clock();
    let mut breaker = CircuitBreaker::new(
        BreakerPolicy {
            failure_threshold: 1,
            success_threshold: 1,
            cooldown: Duration::from_secs(10),
            max_cooldown: Duration::from_secs(45),
            cooldown_multiplier: 8,
            jitter_basis_points: 0,
            concurrent_probes: 1,
        },
        Arc::clone(&time) as Arc<dyn Clock>,
        3,
        16,
    )?;

    // Reopen far more often than the ceiling needs, and it must never pass it.
    for _ in 0..8 {
        fail(&mut breaker, PEER)?;
        let wait = breaker
            .admit(PEER)
            .refusal()
            .map(|refusal| refusal.retry_after)
            .ok_or_else(|| Error::not_found("a refusal"))?;
        assert!(
            wait <= Duration::from_secs(45),
            "the cooldown passed its ceiling: {wait:?}"
        );
        time.advance(wait);
    }
    Ok(())
}

#[test]
fn one_peer_being_down_does_not_refuse_calls_to_another() -> Result<()> {
    // Per-peer, not global. A shared circuit would let one unreachable cell
    // stop the central plane talking to the eight that are fine.
    let time = clock();
    let mut breaker = breaker(Arc::clone(&time))?;
    fail(&mut breaker, PEER)?;
    fail(&mut breaker, PEER)?;
    assert_eq!(breaker.state(PEER), BreakerState::Open);

    assert_eq!(breaker.state("cell-tokyo-1"), BreakerState::Closed);
    assert!(breaker.admit("cell-tokyo-1").is_admitted());
    Ok(())
}

#[test]
fn the_jitter_is_seeded_so_a_run_reproduces_and_two_breakers_spread() -> Result<()> {
    // Jitter exists so that many publishers do not all probe a recovering peer
    // in the same millisecond. It has to be reproducible for the same reason
    // everything else here is: a schedule nobody can replay is a schedule
    // nobody can debug.
    let waits = |seed: u64| -> Result<Vec<Duration>> {
        let time = clock();
        let mut breaker = CircuitBreaker::new(
            BreakerPolicy {
                failure_threshold: 1,
                jitter_basis_points: 5_000,
                ..BreakerPolicy::default()
            },
            Arc::clone(&time) as Arc<dyn Clock>,
            seed,
            16,
        )?;
        let mut observed = Vec::new();
        for _ in 0..6 {
            fail(&mut breaker, PEER)?;
            let wait = breaker
                .admit(PEER)
                .refusal()
                .map(|refusal| refusal.retry_after)
                .ok_or_else(|| Error::not_found("a refusal"))?;
            observed.push(wait);
            time.advance(wait);
        }
        Ok(observed)
    };

    let first = waits(11)?;
    assert_eq!(
        first,
        waits(11)?,
        "the same seed produced a different schedule"
    );
    assert_ne!(
        first,
        waits(12)?,
        "two seeds produced identical schedules, so the jitter is not seeded"
    );

    // Jitter pulls *below* the computed cooldown and never above, so the
    // ceiling stays a real ceiling rather than an average.
    let policy = BreakerPolicy {
        failure_threshold: 1,
        jitter_basis_points: 5_000,
        ..BreakerPolicy::default()
    };
    for wait in first {
        assert!(
            wait <= policy.max_cooldown,
            "jitter pushed a wait above the ceiling: {wait:?}"
        );
    }
    Ok(())
}

// --- refusing a policy that cannot do what it says --------------------------

#[test]
fn a_policy_that_could_not_work_is_refused_at_construction() {
    // Each of these presents at run time as "the transport stopped sending",
    // during the incident the breaker was added for. Construction is a much
    // better place to find out.
    let cases = [
        BreakerPolicy {
            failure_threshold: 0,
            ..BreakerPolicy::default()
        },
        BreakerPolicy {
            success_threshold: 0,
            ..BreakerPolicy::default()
        },
        BreakerPolicy {
            cooldown: Duration::ZERO,
            ..BreakerPolicy::default()
        },
        BreakerPolicy {
            cooldown: Duration::from_secs(60),
            max_cooldown: Duration::from_secs(10),
            ..BreakerPolicy::default()
        },
        BreakerPolicy {
            concurrent_probes: 0,
            ..BreakerPolicy::default()
        },
        BreakerPolicy {
            jitter_basis_points: 10_001,
            ..BreakerPolicy::default()
        },
    ];
    for policy in cases {
        assert!(
            policy.validate().is_err(),
            "a policy that cannot work was accepted: {policy:?}"
        );
    }
    assert!(BreakerPolicy::default().validate().is_ok());

    // And a breaker that can track nothing protects nothing.
    assert!(
        CircuitBreaker::new(BreakerPolicy::default(), clock() as Arc<dyn Clock>, 1, 0).is_err()
    );
}
