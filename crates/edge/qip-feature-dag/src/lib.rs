//! `qip-feature-dag` — compute each changed feature once, and share it.
//!
//! Twenty strategies asking for a twenty-period realised volatility on the
//! same instrument is one computation, not twenty. That is the entire claim,
//! and everything here exists to make it true and to keep it true:
//!
//! * **One node per [`qip_contracts::FeatureKey`].** Identity is
//!   [`qip_contracts::FeatureKey::canonical`], so two independently-built keys
//!   for the same thing are the same node. A second registration is another
//!   consumer, not another computation.
//! * **Cycles are refused at registration**, with the cycle named. A cycle
//!   found during evaluation is an unbounded loop inside the latency budget,
//!   discovered at the worst possible moment.
//! * **A message dirties only what it can affect.** Nodes declare the
//!   instruments and the aspects of market state they read;
//!   [`qip_contracts::MessageBody::may_move_touch`] decides whether a message
//!   can move the top of book at all. Dirtiness then travels to dependents and
//!   stops.
//! * **Evaluation is incremental but not approximate.** Evaluating a stream of
//!   messages incrementally produces exactly the values that rebuilding from
//!   scratch would. The randomised equivalence test in `tests/` is the one
//!   that must never be weakened.
//! * **Missing is not zero.** A feature without the history it needs, or whose
//!   instrument has gone stale, returns [`qip_contracts::FeatureValue::Undefined`].
//!   A default that looks like data is how a strategy trades on nothing.
//!
//! Definitions are pure functions of their declared inputs. Nothing here reads
//! a clock or draws a random number: the evaluation instant arrives as a
//! parameter, so a replay of the same messages at the same instants produces
//! the same values bit for bit.
//!
//! ```
//! use qip_core::{Duration, ObjectId, Timestamp};
//! use qip_feature_dag::{FeatureEngine, MarketState, features};
//!
//! let subject = ObjectId::from_string("OBJ00000000000000000000AAA");
//! let mut engine = FeatureEngine::new(MarketState::default(), Duration::from_secs(30));
//! for definition in features::standard_suite(&subject) {
//!     engine.register(definition)?;
//! }
//! // No messages yet, so every feature is honestly undefined.
//! let vector = engine.evaluate(Timestamp::from_secs(1))?;
//! assert!(vector.undefined().len() == vector.len());
//! # Ok::<(), qip_core::Error>(())
//! ```

pub mod definition;
pub mod engine;
pub mod features;
pub mod graph;
pub mod state;

pub use definition::{FeatureContext, FeatureDefinition, ValueKind};
pub use engine::FeatureEngine;
pub use graph::{FeatureGraph, FeatureId};
pub use state::{InstrumentState, MarketReads, MarketState, TradePrint};
