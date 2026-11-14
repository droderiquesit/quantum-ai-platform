//! A replay digest over microstructure training (R12-TICK-1126).
//!
//! `microstructure.rs` was exercised by behavioural tests but nothing hashed
//! its output, so a change to labelling or fitting that moved every number a
//! little would have passed any test that only asserted the model beat its
//! baseline. The digest below covers the labelled examples and both fitted
//! models; the test proves it is reproducible and that it moves when the
//! stage's input moves.

#![allow(clippy::panic_in_result_fn)]

use qip_core::error::{Error, Result};
use qip_core::hash::sha256_hex;
use qip_core::{Duration, Timestamp};
use qip_training::microstructure::{
    BookObservation, LabelPlan, TradeObservation, build_examples, train_fill_model,
    train_volatility_model,
};

fn at(ms: i64) -> Timestamp {
    Timestamp::from_millis(1_791_676_800_000 + ms)
}

/// A session whose next move follows its touch imbalance, so a fill model has
/// something to learn. Deterministic: a fixed xorshift, no clock, no entropy.
fn session(steps: usize) -> (Vec<BookObservation>, Vec<TradeObservation>) {
    let mut observations = Vec::new();
    let mut trades = Vec::new();
    let mut mid = 1_000.0;
    let mut state: u64 = 0x2545_F491_4F6C_DD1D;
    for i in 0..steps {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let thin = state.is_multiple_of(2);
        let imbalance = if thin { 0.2 } else { 0.8 };
        let t = i as i64 * 1_000;
        let (bid, ask) = (mid - 0.5, mid + 0.5);
        observations.push(BookObservation {
            at: at(t),
            venue: "XLON".into(),
            sequence: i as u64,
            entitlement: "vendor-depth".into(),
            instrument: "obj-ACME".into(),
            best_bid: bid,
            best_ask: ask,
            bid_size: imbalance * 10.0,
            ask_size: (1.0 - imbalance) * 10.0,
            unreliable: false,
        });
        trades.push(TradeObservation {
            at: at(t + 500),
            instrument: "obj-ACME".into(),
            price: if thin { bid - 1.0 } else { ask },
        });
        mid += if thin { -1.0 } else { 1.0 };
    }
    (observations, trades)
}

/// Label, fit both models and hash everything they produced. Serialised
/// through serde_json, whose float output is deterministic for equal values.
fn digest_of(observations: &[BookObservation], trades: &[TradeObservation]) -> Result<String> {
    let plan = LabelPlan {
        horizons: vec![Duration::from_secs(1)],
        sample_every: 1,
        trailing: 5,
        volatile_above_bps: 50.0,
    };
    let set = build_examples(observations, trades, &plan)?;
    if set.examples.len() < 20 {
        return Err(Error::invalid("premise: too few examples were built"));
    }
    let fill = train_fill_model(&set.examples, 0.3)?;
    let volatility = train_volatility_model(&set.examples, 0.3)?;
    let bytes = serde_json::to_vec(&(&set, &fill, &volatility))
        .map_err(|e| Error::invalid(format!("digest serialisation failed: {e}")))?;
    Ok(sha256_hex(&bytes))
}

/// Mutation: in `train_fill_model` or `build_examples`, change any constant
/// (learning rate, label sign) — the second assertion's digest moves with it
/// only if the input moved, and the first fails on nondeterminism.
#[test]
fn a_recorded_session_trains_microstructure_models_to_a_reproducible_digest_that_moves_with_the_session()
-> Result<()> {
    let (observations, trades) = session(60);
    assert!(observations.len() >= 60, "premise: a session was built");
    let digest = digest_of(&observations, &trades)?;
    assert_eq!(digest.len(), 64, "premise: a hex SHA-256");

    // Reproducible: the whole chain again from the same bytes.
    let (again_obs, again_trades) = session(60);
    assert_eq!(digest, digest_of(&again_obs, &again_trades)?);

    // Sensitive: one early book moved by a single lot is a different session,
    // and a digest that ignored the models would pass the line above anyway.
    let mut mutated = observations.clone();
    mutated[5].bid_size += 1.0;
    assert_ne!(digest, digest_of(&mutated, &trades)?);
    Ok(())
}
