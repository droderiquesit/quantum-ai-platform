//! The NOW Brain (blueprint v12.0 §9.6; GCP v3.0 §11.2): latent state at
//! eight scopes, competing estimators, a fenced reconciler, partitioned
//! state, surprise, and compact deltas.
//!
//! Each test asserts its premise before its claim, and each names in its doc
//! comment the mutation of `src/now.rs` that should make it fail.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // a panic is the failure report

use qip_core::error::{Error, Result};
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::time::{Duration, Timestamp};
use qip_world_model::now::{
    Candidate, Catalogue, Constraint, Domain, EstimatorKind, Evidence, KalmanFilter, ManifestRef,
    NowBrain, NowConfig, NowVariable, Observation, PartitionKey, Prior, RateOfChange, Scale, Scope,
    ScopeKind, Specialist, StateFilter, VariableKey, VariableSpec,
};
use std::collections::{BTreeMap, BTreeSet};

fn t0() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn later(t: Timestamp, mins: i64) -> Timestamp {
    t.saturating_add(Duration::from_mins(mins))
}

fn scope(kind: ScopeKind, id: &str) -> Scope {
    if kind == ScopeKind::Global {
        Scope::global()
    } else {
        Scope::new(kind, id).unwrap()
    }
}

fn obs(
    scope: Scope,
    name: &str,
    domain: Domain,
    region: &str,
    value: f64,
    source: &str,
    at: Timestamp,
) -> Observation {
    Observation {
        key: VariableKey::new(scope, name).unwrap(),
        partition: PartitionKey::new(domain, region).unwrap(),
        value,
        source: source.to_string(),
        valid_at: at,
        known_at: at,
        manifest: ManifestRef::new(source, format!("{name}@{at}")).unwrap(),
    }
}

fn ret(id: &str, region: &str, value: f64, source: &str, at: Timestamp) -> Observation {
    obs(
        scope(ScopeKind::Instrument, id),
        "return",
        Domain::Markets,
        region,
        value,
        source,
        at,
    )
}

fn kalman_only() -> NowBrain {
    NowBrain::new(NowConfig::default(), Catalogue::standard().unwrap())
        .unwrap()
        .with_worker(Box::new(KalmanFilter))
}

/// A worker that proposes the readings' mean shifted by a fixed amount: a
/// second opinion that is reliably different, so disagreement is real.
#[derive(Debug)]
struct Shifted(&'static str, f64);

impl StateFilter for Shifted {
    fn id(&self) -> &str {
        self.0
    }
    fn kind(&self) -> EstimatorKind {
        EstimatorKind::Graph
    }
    fn propose(&mut self, e: &Evidence<'_>) -> Result<Option<Candidate>> {
        let mean = e.readings.iter().sum::<f64>() / e.readings.len() as f64;
        Ok(Some(Candidate {
            mean: mean + self.1,
            variance: e.measurement_variance,
        }))
    }
}

/// A worker that always fails, as a crashed estimator would.
#[derive(Debug)]
struct Broken;

impl StateFilter for Broken {
    fn id(&self) -> &str {
        "broken"
    }
    fn kind(&self) -> EstimatorKind {
        EstimatorKind::Neural
    }
    fn propose(&mut self, _: &Evidence<'_>) -> Result<Option<Candidate>> {
        Err(Error::numeric("this worker is down"))
    }
}

fn only<'a>(step: &'a qip_world_model::now::NowStep, key: &VariableKey) -> &'a NowVariable {
    step.promoted
        .iter()
        .find(|v| v.key == *key)
        .unwrap_or_else(|| panic!("{key} was promoted; refused: {:?}", step.refused))
}

/// WORLD-3075: contradictory sources widen the estimate. Mutation: drop the
/// readings' sample variance from the measurement variance in
/// `NowBrain::update`, and the split pair reads as certain as the agreeing
/// pair.
#[test]
fn a_contradictory_pair_of_sources_widens_the_posterior_and_an_agreeing_pair_does_not() {
    let key = VariableKey::new(scope(ScopeKind::Instrument, "AAA"), "return").unwrap();
    let mut agree = kalman_only();
    let token = agree.lead();
    let a = agree
        .step(
            token,
            vec![
                ret("AAA", "US", 0.01, "s1", t0()),
                ret("AAA", "US", 0.01, "s2", t0()),
            ],
            t0(),
        )
        .unwrap();
    let mut split = kalman_only();
    let token = split.lead();
    let b = split
        .step(
            token,
            vec![
                ret("AAA", "US", -0.04, "s1", t0()),
                ret("AAA", "US", 0.06, "s2", t0()),
            ],
            t0(),
        )
        .unwrap();
    let (a, b) = (only(&a, &key), only(&b, &key));
    // Premise: the two sweeps have the same mean, so only the spread differs.
    assert!((a.mean - b.mean).abs() < 1e-12, "{} vs {}", a.mean, b.mean);
    assert_eq!(a.source_diversity, 2);
    assert!(
        b.uncertainty > a.uncertainty * 10.0,
        "a split must read far less certain than agreement: {} vs {}",
        a.uncertainty,
        b.uncertainty
    );
    assert!(b.confidence_bp < a.confidence_bp);
}

/// WORLD-3076: every state variable carries the five §9.6 fields, and one
/// missing any of them is refused. Mutation: make any of the five an
/// `Option` with `#[serde(default)]`, and its removal decodes.
#[test]
fn a_now_variable_missing_any_of_the_five_fields_is_refused_at_decode() {
    let mut brain = NowBrain::standard(7).unwrap();
    let token = brain.lead();
    let step = brain
        .step(token, vec![ret("AAA", "US", 0.01, "s1", t0())], t0())
        .unwrap();
    let variable = step
        .promoted
        .first()
        .expect("premise: one variable promoted");
    let encoded = serde_json::to_value(variable).unwrap();
    // Premise: the whole record decodes.
    let whole: NowVariable = serde_json::from_value(encoded.clone()).unwrap();
    assert_eq!(&whole, variable);
    for field in [
        "confidence_bp",
        "freshness",
        "rate_of_change",
        "source_diversity",
        "uncertainty",
    ] {
        let mut missing = encoded.clone();
        assert!(
            missing.as_object_mut().unwrap().remove(field).is_some(),
            "premise: `{field}` is on the wire"
        );
        assert!(
            serde_json::from_value::<NowVariable>(missing).is_err(),
            "a variable missing `{field}` was admitted"
        );
    }
    // Present but meaningless is refused too: zero sources.
    let mut zero = encoded;
    zero["source_diversity"] = serde_json::json!(0);
    assert!(serde_json::from_value::<NowVariable>(zero).is_err());
    assert_eq!(variable.rate_of_change, RateOfChange::Unmeasured);
}

/// WORLD-3077: a state vector exists at each of the eight scopes, each from
/// the estimators. Mutation: refuse any one scope kind in `Scope::new`.
#[test]
fn every_one_of_the_eight_scopes_gets_a_state_vector_from_the_estimators() {
    let mut brain = NowBrain::standard(7).unwrap();
    brain
        .catalogue_mut()
        .declare(VariableSpec {
            name: "fundamental.revenue".into(),
            domain: Domain::Companies,
            scale: Scale::Relative { floor: 1e-9 },
            fresh_for: Duration::from_days(92),
            expires_after: Duration::from_days(400),
            constraint: Constraint::Unbounded,
            drift_per_day: 0.01,
            reading_noise: 0.05,
        })
        .unwrap();
    let token = brain.lead();
    let at = t0();
    let observations = vec![
        obs(
            scope(ScopeKind::Global, "global"),
            "return",
            Domain::Markets,
            "global",
            0.002,
            "a",
            at,
        ),
        obs(
            scope(ScopeKind::Region, "US"),
            "return",
            Domain::Markets,
            "US",
            0.003,
            "a",
            at,
        ),
        obs(
            scope(ScopeKind::Sector, "tech"),
            "return",
            Domain::Markets,
            "global",
            0.004,
            "a",
            at,
        ),
        obs(
            scope(ScopeKind::Company, "acme"),
            "fundamental.revenue",
            Domain::Companies,
            "global",
            1.5e9,
            "a",
            at,
        ),
        ret("AAA", "US", 0.005, "a", at),
        obs(
            scope(ScopeKind::Venue, "XNYS"),
            "spread_ratio",
            Domain::Liquidity,
            "global",
            1.1,
            "a",
            at,
        ),
        obs(
            scope(ScopeKind::Portfolio, "desk"),
            "exposure_fraction",
            Domain::Positioning,
            "US",
            0.3,
            "a",
            at,
        ),
        obs(
            scope(ScopeKind::Infrastructure, "US"),
            "halted",
            Domain::SystemHealth,
            "US",
            0.0,
            "a",
            at,
        ),
    ];
    let step = brain.step(token, observations, at).unwrap();
    assert!(step.refused.is_empty(), "{:?}", step.refused);
    let kinds: BTreeSet<ScopeKind> = step.promoted.iter().map(|v| v.key.scope.kind).collect();
    assert_eq!(kinds, ScopeKind::ALL.into_iter().collect::<BTreeSet<_>>());
    assert_eq!(step.promoted.len(), 8);
    // At least the Kalman and robust filters, whose estimates sit on their
    // readings; the particle filter's may be vetoed on a bounded variable
    // read at its bound (a halted flag of zero), which is the constraint
    // working, not a missing scope.
    for v in &step.promoted {
        assert!(v.ensemble >= 2, "{} had {} estimator(s)", v.key, v.ensemble);
    }
}

/// WORLD-3078: two estimators for one variable both contribute and their
/// disagreement is recorded. Mutation: promote the first candidate alone in
/// `NowBrain::update`, and the ensemble reads one and no dispute is raised.
#[test]
fn two_estimators_on_one_variable_both_contribute_and_their_disagreement_is_recorded() {
    let mut brain = kalman_only().with_worker(Box::new(Shifted("graph-shift", 0.05)));
    let token = brain.lead();
    let key = VariableKey::new(scope(ScopeKind::Instrument, "AAA"), "return").unwrap();
    let step = brain
        .step(token, vec![ret("AAA", "US", 0.01, "s1", t0())], t0())
        .unwrap();
    let v = only(&step, &key);
    assert_eq!(v.ensemble, 2);
    assert!((v.disagreement - 0.05).abs() < 1e-9, "{}", v.disagreement);
    let dispute = step
        .disagreements
        .first()
        .expect("the disagreement is recorded");
    let views: BTreeSet<&str> = dispute.views.iter().map(|w| w.id.as_str()).collect();
    assert_eq!(views, BTreeSet::from(["kalman-local-level", "graph-shift"]));
}

/// WORLD-9019: a missing worker reduces diversity rather than failing
/// estimation, and a failing one is recorded and skipped. Mutation: return
/// the error from a worker's `propose` instead of recording it, and the
/// variable is refused.
#[test]
fn removing_or_losing_a_worker_shrinks_the_reported_ensemble_and_estimation_continues() {
    let mut brain = NowBrain::standard(7).unwrap().with_worker(Box::new(Broken));
    let token = brain.lead();
    let key = VariableKey::new(scope(ScopeKind::Instrument, "AAA"), "return").unwrap();
    let first = brain
        .step(token, vec![ret("AAA", "US", 0.01, "s1", t0())], t0())
        .unwrap();
    assert_eq!(
        only(&first, &key).ensemble,
        3,
        "premise: three healthy workers"
    );
    assert_eq!(first.worker_failures.get("broken"), Some(&1));
    assert!(brain.remove_worker("particle-bootstrap"));
    let t1 = later(t0(), 60);
    let second = brain
        .step(token, vec![ret("AAA", "US", 0.012, "s1", t1)], t1)
        .unwrap();
    assert_eq!(only(&second, &key).ensemble, 2);
}

/// WORLD-9020: a reconciler holding a stale fencing epoch cannot promote.
/// Mutation: make `NowStore::fenced` accept any epoch.
#[test]
fn a_reconciler_holding_a_stale_fencing_epoch_cannot_promote() {
    let mut brain = NowBrain::standard(7).unwrap();
    let stale = brain.lead();
    let current = brain.lead();
    assert!(current.epoch() > stale.epoch(), "premise: the lead moved");
    let refused = brain.step(stale, vec![ret("AAA", "US", 0.01, "s1", t0())], t0());
    assert!(refused.is_err());
    assert!(
        brain.store().is_empty(),
        "a fenced reconciler wrote nothing"
    );
    let ok = brain
        .step(current, vec![ret("AAA", "US", 0.01, "s1", t0())], t0())
        .unwrap();
    assert_eq!(ok.promoted.len(), 1);
    assert_eq!(ok.promoted[0].epoch, current.epoch());
}

/// WORLD-9020 and WORLD-9021: a candidate that breaks its symbolic
/// constraint is not promoted, and when nothing admissible remains the last
/// promoted state stands. Mutation: drop the `constraint.admits` check.
#[test]
fn a_candidate_that_breaks_its_constraint_is_vetoed_and_the_last_promoted_state_stands() {
    let vol = |v: f64, at: Timestamp| {
        obs(
            scope(ScopeKind::Instrument, "AAA"),
            "realised_volatility",
            Domain::Volatility,
            "US",
            v,
            "s1",
            at,
        )
    };
    let key = VariableKey::new(scope(ScopeKind::Instrument, "AAA"), "realised_volatility").unwrap();
    let partition = PartitionKey::new(Domain::Volatility, "US").unwrap();
    let mut brain = kalman_only().with_worker(Box::new(Shifted("negative", -1.0)));
    let token = brain.lead();
    let step = brain.step(token, vec![vol(0.02, t0())], t0()).unwrap();
    assert_eq!(step.vetoed.get("negative"), Some(&1));
    assert_eq!(only(&step, &key).ensemble, 1);
    let before = brain.store().get(&partition, &key).cloned().unwrap();
    // Only the violating worker remains: nothing admissible this step.
    assert!(brain.remove_worker("kalman-local-level"));
    let t1 = later(t0(), 30);
    let step = brain.step(token, vec![vol(0.03, t1)], t1).unwrap();
    assert!(step.promoted.is_empty());
    assert!(step.refused.iter().any(|(k, _)| *k == key.to_string()));
    assert_eq!(brain.store().get(&partition, &key), Some(&before));
}

/// WORLD-9018 and WORLD-9021: past freshness the state is still served at a
/// lower confidence; at expiry it is refused. Mutation: return the
/// promotion confidence from `confidence_at` regardless of age, or drop the
/// expiry check in `NowBrain::read`.
#[test]
fn stale_state_is_served_at_decaying_confidence_until_expiry_and_refused_after() {
    let mut brain = NowBrain::standard(7).unwrap();
    let token = brain.lead();
    let key = VariableKey::new(scope(ScopeKind::Infrastructure, "US"), "halted").unwrap();
    let partition = PartitionKey::new(Domain::SystemHealth, "US").unwrap();
    brain
        .step(
            token,
            vec![obs(
                scope(ScopeKind::Infrastructure, "US"),
                "halted",
                Domain::SystemHealth,
                "US",
                0.0,
                "k",
                t0(),
            )],
            t0(),
        )
        .unwrap();
    let fresh = brain.read(&partition, &key, later(t0(), 30)).unwrap();
    assert!(!fresh.stale);
    assert_eq!(fresh.confidence_bp, fresh.variable.confidence_bp);
    assert!(fresh.confidence_bp > 0, "premise: a confident estimate");
    let stale = brain.read(&partition, &key, later(t0(), 180)).unwrap();
    assert!(stale.stale);
    assert!(stale.confidence_bp < fresh.confidence_bp && stale.confidence_bp > 0);
    assert!(brain.read(&partition, &key, later(t0(), 24 * 60)).is_err());
}

/// WORLD-9017: a write to one partition does not touch another. Mutation:
/// key the store by variable alone, and the regions collide.
#[test]
fn a_write_to_one_partition_leaves_every_other_partition_untouched() {
    let mut brain = NowBrain::standard(7).unwrap();
    let token = brain.lead();
    brain
        .step(
            token,
            vec![
                ret("AAA", "US", 0.01, "s1", t0()),
                ret("AAA", "GB", 0.02, "s1", t0()),
            ],
            t0(),
        )
        .unwrap();
    let us = PartitionKey::new(Domain::Markets, "US").unwrap();
    let gb = PartitionKey::new(Domain::Markets, "GB").unwrap();
    let gb_before = brain
        .store()
        .partition(&gb)
        .cloned()
        .expect("premise: GB holds state");
    let us_before = brain
        .store()
        .partition(&us)
        .cloned()
        .expect("premise: US holds state");
    let t1 = later(t0(), 60);
    brain
        .step(token, vec![ret("AAA", "US", 0.03, "s1", t1)], t1)
        .unwrap();
    assert_eq!(brain.store().partition(&gb), Some(&gb_before));
    assert_ne!(brain.store().partition(&us), Some(&us_before));
}

/// Point in time: a reading not yet knowable at the step is refused.
/// Mutation: drop the `known_at > now` refusal in `NowBrain::step`.
#[test]
fn a_reading_knowable_only_after_the_step_is_refused_as_look_ahead() {
    let mut brain = NowBrain::standard(7).unwrap();
    let token = brain.lead();
    let mut future = ret("AAA", "US", 0.01, "s1", t0());
    future.known_at = later(t0(), 1);
    let step = brain.step(token, vec![future], t0()).unwrap();
    assert!(step.promoted.is_empty());
    assert_eq!(step.refused.len(), 1);
    assert!(
        step.refused[0].1.contains("did not yet have"),
        "{:?}",
        step.refused
    );
}

/// WORLD-9016 and AMBIENT-3079: a residual past the bound is a surprise that
/// wakes all four specialists; an ordinary move is not. Mutation: compare
/// the residual against the bound with the wrong sign, or emit no wakes.
#[test]
fn a_residual_past_the_bound_raises_a_surprise_that_wakes_all_four_specialists() {
    let run = |second: f64| {
        let mut brain = NowBrain::standard(7).unwrap();
        let token = brain.lead();
        brain
            .step(token, vec![ret("AAA", "US", 0.001, "s1", t0())], t0())
            .unwrap();
        let t1 = later(t0(), 60);
        brain
            .step(token, vec![ret("AAA", "US", second, "s1", t1)], t1)
            .unwrap()
    };
    let calm = run(0.002);
    assert!(calm.surprises.is_empty(), "{:?}", calm.surprises);
    assert_eq!(calm.promoted.len(), 1, "premise: the calm step updated");
    let shock = run(0.25);
    let surprise = shock
        .surprises
        .first()
        .expect("a 25% return against a 0.1% state is a surprise");
    assert!(surprise.sigmas > 3.0);
    assert_eq!(surprise.wakes, Specialist::ALL.to_vec());
    assert!(!surprise.manifests.is_empty());
}

/// DATA-3080 and EVID-3081: a delta carries moments and manifests only; a
/// raw payload field fails the schema, and a delta citing no manifest is
/// refused. Mutation: remove `deny_unknown_fields` from `BeliefDeltaFields`,
/// or the empty-manifest refusal.
#[test]
fn a_belief_delta_carries_moments_and_manifests_and_a_raw_payload_is_refused() {
    let mut brain = NowBrain::standard(7).unwrap();
    let token = brain.lead();
    let first = brain
        .step(token, vec![ret("AAA", "US", 0.01, "s1", t0())], t0())
        .unwrap();
    let t1 = later(t0(), 60);
    let second = brain
        .step(token, vec![ret("AAA", "US", 0.012, "s1", t1)], t1)
        .unwrap();
    assert_eq!(first.deltas[0].prior, Prior::Genesis);
    assert!(matches!(second.deltas[0].prior, Prior::Was { .. }));
    let delta = &second.deltas[0];
    assert_eq!(delta.manifests[0].source, "s1");
    let encoded = serde_json::to_value(delta).unwrap();
    // Premise: the record round-trips.
    assert!(serde_json::from_value::<qip_world_model::now::BeliefDelta>(encoded.clone()).is_ok());
    let mut raw = encoded.clone();
    raw["payload"] = serde_json::json!({"close": 101.2});
    assert!(serde_json::from_value::<qip_world_model::now::BeliefDelta>(raw).is_err());
    let mut uncited = encoded;
    uncited["manifests"] = serde_json::json!([]);
    assert!(serde_json::from_value::<qip_world_model::now::BeliefDelta>(uncited).is_err());
}

/// Idempotency: a reading already absorbed is not absorbed twice. Mutation:
/// drop the `valid_at > observed_at` filter, and the second step shrinks the
/// uncertainty on no new evidence.
#[test]
fn a_reading_already_absorbed_is_not_counted_again() {
    let mut brain = NowBrain::standard(7).unwrap();
    let token = brain.lead();
    let reading = ret("AAA", "US", 0.01, "s1", t0());
    let first = brain.step(token, vec![reading.clone()], t0()).unwrap();
    assert_eq!(first.promoted.len(), 1, "premise: the first absorbs");
    let again = brain.step(token, vec![reading], later(t0(), 5)).unwrap();
    assert_eq!(again.unchanged, 1);
    assert!(again.promoted.is_empty() && again.deltas.is_empty());
}

/// How one step of a generated schedule treats one variable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Delivery {
    /// A reading newer than anything the variable rests on.
    Fresh,
    /// Nothing at all for this variable this step.
    Withheld,
    /// The reading the variable last absorbed, delivered again unchanged.
    Redelivered,
    /// A reading the step must refuse: not yet knowable, or not a number.
    Refused,
}

/// One promoted variable across one step: how the step treated it, and its
/// posterior variance either side.
#[derive(Clone, Debug)]
struct Transition {
    seed: u64,
    step: usize,
    instrument: &'static str,
    delivery: Delivery,
    before: f64,
    after: f64,
}

/// RM-P2-15: the posterior variance of a NOW variable never shrinks across a
/// step that brought it no new reading, whether the reading was withheld, was
/// one the variable had already absorbed, or was refused as not yet knowable
/// or not a number.
///
/// The failure this prevents is confidence manufactured from silence.
/// `uncertainty` is the posterior variance every reader sizes its trust on:
/// `confidence_bp` is computed from it, and the forecast lattice and the deep
/// brain's surprise router read both. A step that tightened it on no evidence
/// would report a state the brain has not looked at again as better known
/// than when it last did. `NowBrain::update`'s idempotency comment names the
/// shape that takes in the cycle: a pass that re-reads the same last bar,
/// shrinking the uncertainty every time.
/// `a_reading_already_absorbed_is_not_counted_again` holds one re-delivery
/// after one reading. This holds every way of bringing nothing, interleaved
/// with fresh readings, across generated schedules.
///
/// Property-style, on the in-tree generator because no property-testing
/// crate is on the allow-list: 24 seeded schedules of 60 steps over three
/// instruments in two partitions, under the standard three-filter ensemble,
/// each step choosing per instrument between a fresh reading and the three
/// ways of bringing none. A violation names the seed and step that reproduce
/// it.
///
/// Mutations: drop the `valid_at > observed_at` filter in `NowBrain::update`
/// (fires on a re-delivered step); in `NowBrain::step`, carry every promoted
/// variable that brought no admissible reading forward as a reading of its
/// own mean at `now` (fires on a withheld or refused step).
#[test]
fn the_state_covariance_never_shrinks_when_an_observation_is_withheld() {
    const SCHEDULES: u64 = 24;
    const STEPS: usize = 60;
    // AAA and BBB share a partition and CCC has its own, so a step's write to
    // one variable sits beside variables the same step left alone.
    let instruments: [(&'static str, &'static str); 3] =
        [("AAA", "US"), ("BBB", "US"), ("CCC", "GB")];
    let uncertainty = |brain: &NowBrain, id: &str, region: &str| {
        brain
            .store()
            .get(
                &PartitionKey::new(Domain::Markets, region).unwrap(),
                &VariableKey::new(scope(ScopeKind::Instrument, id), "return").unwrap(),
            )
            .map(|variable| variable.uncertainty)
    };

    let mut transitions: Vec<Transition> = Vec::new();
    let mut vanished: Vec<String> = Vec::new();
    let mut unrefused: Vec<String> = Vec::new();
    for seed in 0..SCHEDULES {
        let mut rng = Xoshiro256::seeded(seed);
        let mut brain = NowBrain::standard(seed).unwrap();
        let token = brain.lead();
        // The reading each instrument's variable most recently absorbed.
        let mut absorbed: BTreeMap<&str, Observation> = BTreeMap::new();
        let mut now = t0();
        for step in 0..STEPS {
            // A minute to an hour per step, so sixty steps stay well inside
            // the seven days after which a `return` expires: a variable can
            // leave the store mid-schedule only if something removes it.
            now = later(now, 1 + rng.below(60) as i64);
            let mut deliveries: BTreeMap<&str, Delivery> = BTreeMap::new();
            let mut fresh: BTreeMap<&str, Observation> = BTreeMap::new();
            let mut observations = Vec::new();
            for &(id, region) in &instruments {
                // A variable not yet promoted has no variance to shrink, so
                // its first delivery is always a reading.
                let delivery = if absorbed.contains_key(id) {
                    match rng.below(4) {
                        0 => Delivery::Fresh,
                        1 => Delivery::Withheld,
                        2 => Delivery::Redelivered,
                        _ => Delivery::Refused,
                    }
                } else {
                    Delivery::Fresh
                };
                let value = rng.normal_with(0.001, 0.01);
                match delivery {
                    Delivery::Fresh => {
                        let reading = ret(id, region, value, "s1", now);
                        fresh.insert(id, reading.clone());
                        observations.push(reading);
                    }
                    Delivery::Withheld => {}
                    Delivery::Redelivered => observations.push(absorbed[id].clone()),
                    Delivery::Refused => {
                        let mut reading = ret(id, region, value, "s1", now);
                        if rng.bernoulli(0.5) {
                            reading.known_at = later(now, 1);
                        } else {
                            reading.value = f64::NAN;
                        }
                        observations.push(reading);
                    }
                }
                deliveries.insert(id, delivery);
            }

            let before: BTreeMap<&str, Option<f64>> = instruments
                .iter()
                .map(|&(id, region)| (id, uncertainty(&brain, id, region)))
                .collect();
            let outcome = brain.step(token, observations, now).unwrap();
            let refused = deliveries
                .values()
                .filter(|delivery| **delivery == Delivery::Refused)
                .count();
            if outcome.refused.len() != refused {
                unrefused.push(format!(
                    "seed {seed} step {step}: {refused} refusable reading(s), {} refusal(s): {:?}",
                    outcome.refused.len(),
                    outcome.refused
                ));
            }
            for &(id, region) in &instruments {
                let after = uncertainty(&brain, id, region);
                match (before[id], after) {
                    (Some(before), Some(after)) => transitions.push(Transition {
                        seed,
                        step,
                        instrument: id,
                        delivery: deliveries[id],
                        before,
                        after,
                    }),
                    (Some(_), None) => vanished.push(format!("seed {seed} step {step}: {id}")),
                    (None, _) => {}
                }
                // Remember a fresh reading as absorbed only once the
                // promoted state says it rests on it.
                if let Some(reading) = fresh.remove(id) {
                    let partition = PartitionKey::new(Domain::Markets, region).unwrap();
                    let rests_on = brain
                        .store()
                        .get(&partition, &reading.key)
                        .is_some_and(|variable| variable.freshness.observed_at == reading.valid_at);
                    if rests_on {
                        absorbed.insert(id, reading);
                    }
                }
            }
        }
    }

    // Premise: the schedules did what they claim. Every way of bringing
    // nothing occurred often on a promoted variable; nothing left the store
    // mid-schedule, which would hide a step from the property; every reading
    // meant to be refused was refused rather than absorbed; and a fresh
    // reading does shrink the variance, so a brain that never moved it could
    // not satisfy the property by standing still.
    let count = |wanted: Delivery| {
        transitions
            .iter()
            .filter(|transition| transition.delivery == wanted)
            .count()
    };
    for delivery in [Delivery::Withheld, Delivery::Redelivered, Delivery::Refused] {
        assert!(
            count(delivery) >= 100,
            "premise: only {} {delivery:?} step(s) on a promoted variable",
            count(delivery)
        );
    }
    assert!(
        vanished.is_empty(),
        "premise: a promoted variable left the store mid-schedule: {vanished:?}"
    );
    assert!(
        unrefused.is_empty(),
        "premise: a reading meant to be refused was not: {unrefused:?}"
    );
    let shrank_on_fresh = transitions
        .iter()
        .filter(|transition| {
            transition.delivery == Delivery::Fresh && transition.after < transition.before
        })
        .count();
    assert!(
        shrank_on_fresh > 0,
        "premise: no fresh reading ever shrank the variance, so the brain cannot shrink it at \
         all and the property below would hold vacuously"
    );

    // The property.
    let violations: Vec<String> = transitions
        .iter()
        .filter(|transition| {
            transition.delivery != Delivery::Fresh && transition.after < transition.before
        })
        .map(|transition| {
            format!(
                "seed {} step {}: {} {:?} took the variance from {:e} to {:e}",
                transition.seed,
                transition.step,
                transition.instrument,
                transition.delivery,
                transition.before,
                transition.after
            )
        })
        .collect();
    assert!(
        violations.is_empty(),
        "the posterior variance shrank on {} step(s) that brought no new reading; first: {:?}",
        violations.len(),
        violations.iter().take(3).collect::<Vec<_>>()
    );
}

/// WORLD-9016 and lane H's §27 contract: the coordinator's typed outputs.
/// Mutation: let `regional_state` include another region's partitions.
#[test]
fn the_global_regional_and_world_model_projections_are_typed_and_scoped() {
    let mut brain = NowBrain::standard(7).unwrap();
    let token = brain.lead();
    brain
        .step(
            token,
            vec![
                obs(
                    scope(ScopeKind::Global, "global"),
                    "return",
                    Domain::Markets,
                    "global",
                    0.002,
                    "a",
                    t0(),
                ),
                ret("AAA", "US", 0.01, "s1", t0()),
                ret("BBB", "GB", 0.01, "s1", t0()),
            ],
            t0(),
        )
        .unwrap();
    let global = brain.global_state(t0());
    assert_eq!(global.variables.len(), 1);
    let us = brain.regional_state("US", t0());
    assert_eq!(us.variables.len(), 1);
    assert!(us.variables.iter().all(|v| v.partition.region == "US"));
    let partition = PartitionKey::new(Domain::Markets, "US").unwrap();
    let state = brain.world_model_state(&partition, t0()).unwrap();
    assert_eq!(state.state.len(), 1);
    assert_eq!(state.scope, "markets/US");
    assert!(matches!(
        state.calibration,
        qip_contracts::forecast::Calibration::Uncalibrated
    ));
    let t1 = later(t0(), 60);
    brain
        .step(token, vec![ret("AAA", "US", 0.011, "s1", t1)], t1)
        .unwrap();
    let state = brain.world_model_state(&partition, t1).unwrap();
    assert!(matches!(
        state.calibration,
        qip_contracts::forecast::Calibration::Measured { resolved: 1, .. }
    ));
}
