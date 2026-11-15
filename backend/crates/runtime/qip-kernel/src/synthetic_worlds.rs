//! The generated worlds SIMULATE draws from a seed, as `SyntheticEpisode`s
//! (R12-EXPAND-6033, ADR 0682).
//!
//! `scenario.evaluated` already carried an episode for every library entry and
//! every causal driver's shock (`SyntheticGenerator::Library` and
//! `Conditional`, no seed, because neither draws). Three things SIMULATE makes
//! were drawn from a seed and left nothing: the two generated tail worlds, the
//! generated joint world, and the capacity probe's agent simulation. The
//! contract's stochastic arm, `SyntheticGenerator::MonteCarlo`, therefore had
//! no producer and its seed field had no writer: a world that cost the book a
//! fifth of its equity could not be replayed from the log because the log did
//! not say it was drawn, or from what.
//!
//! This module is pure: it builds the episodes and the digests, and
//! `Platform::stage_simulate` journals them. No I/O, no environment.

use crate::tail::{SYNTHETIC_JOINT, SYNTHETIC_WORLDS, TailWorld};
use qip_contracts::expansion::{SyntheticEpisode, SyntheticGenerator};
use qip_core::error::Result;

/// The scenario name of the capacity probe's agent simulation, per subject.
/// The subject is the instrument with the longest history, so the name
/// follows the book and is not a fixed label.
pub const CAPACITY_PROBE_SCENARIO_PREFIX: &str = "capacity-probe:";

/// Whether `scenario` names a world drawn from the cycle's seed rather than
/// computed from the book alone. The other tail worlds are deterministic
/// functions of the book and the history and draw nothing, so they owe no
/// seed and are not episodes of this kind.
pub fn is_drawn_world(scenario: &str) -> bool {
    scenario == SYNTHETIC_JOINT || SYNTHETIC_WORLDS.contains(&scenario)
}

/// Free text is length-prefixed so a name containing the separator cannot
/// make two different inputs digest alike.
fn text(value: &str) -> String {
    format!("{}:{value}", value.len())
}

/// The SHA-256 of one drawn world as the stage computed it: its name, the
/// seed it was drawn from, the loss in both forms as their bits, and the
/// positions charged the unmeasured excursion. The bits, not a rounded
/// figure: two worlds that differ in the last place are two worlds.
pub fn world_digest(world: &TailWorld, seed: u64) -> String {
    let mut canonical = format!(
        "tail|{}|seed|{seed}|fraction|{}|loss|{}",
        text(&world.scenario),
        world.loss_fraction.to_bits(),
        world.loss.to_bits()
    );
    for object in &world.unmeasured {
        canonical.push_str(&format!("|unmeasured|{}", text(object)));
    }
    qip_core::sha256_hex(canonical.as_bytes())
}

/// One `MonteCarlo` episode for each world in `worlds` that was drawn from
/// `seed`, in the order given. A deterministic world is skipped, not recorded
/// under a seed nothing used (the contract refuses that on construction).
///
/// `seed` is the cycle's: the one number the generators in `tail.rs` XOR
/// with their own stream constants, so replaying the cycle regenerates them.
pub fn drawn_world_episodes<'a>(
    worlds: impl IntoIterator<Item = &'a TailWorld>,
    seed: u64,
) -> Result<Vec<SyntheticEpisode>> {
    worlds
        .into_iter()
        .filter(|world| is_drawn_world(&world.scenario))
        .map(|world| {
            SyntheticEpisode::new(
                world.scenario.clone(),
                SyntheticGenerator::MonteCarlo,
                world_digest(world, seed),
                Some(seed),
            )
        })
        .collect()
}

/// What the capacity probe's simulation was built from. Every figure that
/// shaped the run is here, so two probes with the same digest and seed are the
/// same run.
#[derive(Clone, Debug)]
pub struct ProbeInputs<'a> {
    pub subject: &'a str,
    /// The last price the market opens at, as text: a `Decimal`'s own string.
    pub initial_price: &'a str,
    pub quantity: &'a str,
    pub half_spread_bps: f64,
    pub level_spacing_bps: f64,
    pub step_volatility: f64,
    pub step_drift: f64,
    /// The calibrated feed delay, in nanoseconds.
    pub feed_delay_nanos: i64,
    /// The measured decision latency's base and jitter, in nanoseconds, or
    /// none when the probe ran with no latency condition.
    pub latency_nanos: Option<(i64, i64)>,
}

/// The digest of a capacity probe's inputs, as their bits.
pub fn probe_digest(inputs: &ProbeInputs<'_>) -> String {
    let canonical = format!(
        "probe|{}|price|{}|qty|{}|half|{}|spacing|{}|vol|{}|drift|{}|feed|{}|latency|{}",
        text(inputs.subject),
        text(inputs.initial_price),
        text(inputs.quantity),
        inputs.half_spread_bps.to_bits(),
        inputs.level_spacing_bps.to_bits(),
        inputs.step_volatility.to_bits(),
        inputs.step_drift.to_bits(),
        inputs.feed_delay_nanos,
        inputs
            .latency_nanos
            .map(|(base, jitter)| format!("{base}/{jitter}"))
            .unwrap_or_else(|| "none".to_string())
    );
    qip_core::sha256_hex(canonical.as_bytes())
}

/// The `MonteCarlo` episode for a capacity probe: the agent simulation runs
/// from `MarketSimulator::synthetic(market, seed)`, with `seed` the
/// platform's configured one.
pub fn probe_episode(inputs: &ProbeInputs<'_>, seed: u64) -> Result<SyntheticEpisode> {
    probe_episode_from_digest(inputs.subject, probe_digest(inputs), seed)
}

/// The same episode from a digest already taken, for the caller that holds
/// the digest and no longer the borrowed inputs.
pub fn probe_episode_from_digest(
    subject: &str,
    input_digest: String,
    seed: u64,
) -> Result<SyntheticEpisode> {
    SyntheticEpisode::new(
        format!("{CAPACITY_PROBE_SCENARIO_PREFIX}{subject}"),
        SyntheticGenerator::MonteCarlo,
        input_digest,
        Some(seed),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::Timestamp;

    fn world(name: &str, loss: f64) -> TailWorld {
        TailWorld {
            scenario: name.to_string(),
            at: Timestamp::from_secs(1_760_000_000),
            loss_fraction: loss / 1_000.0,
            loss,
            unmeasured: Vec::new(),
            detail: String::new(),
        }
    }

    fn inputs() -> ProbeInputs<'static> {
        ProbeInputs {
            subject: "AAPL",
            initial_price: "100",
            quantity: "10",
            half_spread_bps: 1.0,
            level_spacing_bps: 2.0,
            step_volatility: 0.01,
            step_drift: 0.0,
            feed_delay_nanos: 0,
            latency_nanos: None,
        }
    }

    /// Only the worlds drawn from the seed become episodes, each carrying the
    /// seed. Mutation: not run (nothing was compiled); the one that guards
    /// it is `is_drawn_world` returning true for every name, which fails the
    /// length assertion.
    #[test]
    fn only_drawn_worlds_become_monte_carlo_episodes_and_each_carries_the_seed() {
        let worlds = vec![
            world(SYNTHETIC_WORLDS[0], 10.0),
            world("model-failure", 20.0),
            world(SYNTHETIC_WORLDS[1], 30.0),
            world(SYNTHETIC_JOINT, 40.0),
        ];
        let episodes = drawn_world_episodes(&worlds, 7).expect("drawn worlds are admitted");
        assert_eq!(episodes.len(), 3, "{episodes:?}");
        for episode in &episodes {
            assert_eq!(episode.generator(), SyntheticGenerator::MonteCarlo);
            assert_eq!(episode.seed(), Some(7));
            assert_ne!(episode.scenario(), "model-failure");
        }
    }

    /// The digest moves with the seed and with the loss, so a replay under a
    /// different seed cannot be mistaken for the same draw.
    /// Mutation: not run.
    #[test]
    fn a_drawn_worlds_digest_moves_with_its_seed_and_its_loss() {
        let a = world(SYNTHETIC_WORLDS[0], 10.0);
        let b = world(SYNTHETIC_WORLDS[0], 11.0);
        assert_eq!(world_digest(&a, 1), world_digest(&a, 1));
        assert_ne!(world_digest(&a, 1), world_digest(&a, 2));
        assert_ne!(world_digest(&a, 1), world_digest(&b, 1));
    }

    /// The probe episode names its subject, is stochastic and keeps the seed;
    /// every input figure is in the digest.
    /// Mutation: not run.
    #[test]
    fn a_probe_episode_carries_its_seed_and_a_digest_of_every_input() {
        let base = inputs();
        let episode = probe_episode(&base, 0x5EED).expect("a probe episode is admitted");
        assert_eq!(episode.scenario(), "capacity-probe:AAPL");
        assert_eq!(episode.generator(), SyntheticGenerator::MonteCarlo);
        assert_eq!(episode.seed(), Some(0x5EED));
        let reference = probe_digest(&base);
        let variants = [
            ProbeInputs { quantity: "11", ..base.clone() },
            ProbeInputs { step_volatility: 0.02, ..base.clone() },
            ProbeInputs { step_drift: 0.001, ..base.clone() },
            ProbeInputs { feed_delay_nanos: 1, ..base.clone() },
            ProbeInputs { latency_nanos: Some((1, 0)), ..base.clone() },
            ProbeInputs { half_spread_bps: 1.5, ..base.clone() },
        ];
        for variant in &variants {
            assert_ne!(probe_digest(variant), reference, "{variant:?}");
        }
    }
}
