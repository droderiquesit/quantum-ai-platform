//! Blueprint §25.3's three levels no instrument record can carry: per family,
//! per factor and per causal driver.
//!
//! # The gap this closes
//!
//! Seven of the envelope's ten levels already fire, because every one of them
//! is a fact the reference data vouches for: an instrument has one sector, one
//! country, one asset class, one listing venue, and a fill has one
//! counterparty. `qip-kernel`'s `exposure_axes_of` reads those off the record
//! and [`crate::aggregate::RiskAggregates::apply_fill`] keeps a running
//! counter per bucket, which [`LimitKind::MaxAxisWeight`] and
//! `LimitKind::MaxBucketExposure` then read.
//!
//! The remaining three are not on the record and never will be. A strategy
//! family is a fact about the search that produced the strategy; a factor
//! loading is a regression over the tape; a causal driver is an edge in the
//! world model's graph. So the kernel's own comment on that projection was
//! right that "nothing else the blueprint's gate names — factor, family,
//! causal driver — is carried by the instrument record", and wrong only in
//! the conclusion it was read to license: that those levels therefore had no
//! producer. They have one, it is simply not the instrument record.
//!
//! # Why this feeds the same axes rather than three new limit kinds
//!
//! Because `MaxCounterpartyExposure` already made the other choice and it was
//! wrong. That cap read a `RiskState::counterparty_exposures` map of its own,
//! nothing filled it, and the fix was not to find it a producer but to charge
//! the counterparty to the axis mechanism every other bucket limit already
//! reads — "two representations of one fact disagree eventually; there is now
//! one". Three more limit kinds here would be three more such maps. Instead
//! the producer fills [`RiskState::axis_exposures`] under three reserved
//! names, and the veto that fires is the one already proven to fire.
//!
//! # The figure, and why it is not a partition
//!
//! A bucket holds the **gross notional of the positions that share the named
//! cause, each weighted by how much of that cause it actually carries**, which
//! the two bucket limits then take as a fraction of equity. Sector and country
//! partition the book; a driver does not. One instrument can sit downstream of
//! four causal edges and load on three factors, so the buckets of these axes
//! overlap and their sum exceeds the book's gross on any real portfolio. Two
//! consequences follow and both are load-bearing:
//!
//! * [`LimitKind::MaxAxisWeight`] and `LimitKind::MaxBucketExposure` are
//!   meaningful over these axes, because both divide a bucket by **equity**,
//!   which no driver membership moves.
//! * `LimitKind::MaxConcentration` is **not**, because it divides a bucket by
//!   the sum of the buckets, and over an overlapping axis that denominator is
//!   a number nobody computed — it would read 0.25 for a book whose every
//!   position shares one cause with three others. The shipped set names
//!   neither of these axes on that kind, and [`SharedCauseExposure::apply`]
//!   cannot cause it to: the axis a limit names is the limit's own field.
//!
//! Every contribution is non-negative — an absolute notional times a
//! non-negative weight — so a bucket is monotone non-decreasing in the book's
//! positions and in the drivers attributed to them. There is deliberately no
//! netting anywhere below. Netting a driver would let a long and a short that
//! move together *reduce* the measured exposure to the mechanism they share,
//! which is the diversification illusion §25.3 calls "the concentration that
//! ends firms", written into the control meant to catch it.
//!
//! # Four states, and the difference between the last two
//!
//! After [`SharedCauseExposure::apply`], a reader can tell these apart, and
//! the whole point of the type is that they do not collapse into one:
//!
//! | State | How it reads |
//! |---|---|
//! | No producer ran | the axis is absent from [`RiskState::axis_exposures`] |
//! | The producer ran and the book shares no named cause | the axis is **present and empty** |
//! | The producer ran and found drivers | the axis carries one bucket per driver the book holds |
//! | The producer could not read its source | [`RiskState::unevaluated`] carries the axis, and `PreTradeChecker::check` refuses every order while it stands |
//!
//! Present-and-empty rather than absent is the legibility this file exists to
//! provide. An idle control and an unwired one are the same silence at the
//! venue, and this platform has already shipped that confusion twice — the
//! liquidity floor that abstained on a refused ladder, and the tail limits
//! that looked up a key nothing wrote.
//!
//! # What this bounds, stated smaller than it could be
//!
//! The standing book, not the book the order under check would produce.
//! `PreTradeChecker::project` moves a bucket by the order's own change in
//! gross for every axis the *order* names, and an order names at most one
//! bucket per axis — a `BTreeMap<String, String>` cannot say that an
//! instrument sits downstream of four causes. So a breach here refuses new
//! risk on a book already past the bound, and an order that would take a
//! clean book past it is caught on the next state rather than before it goes.
//!
//! That is a smaller claim than the sector cap makes, and it is written down
//! rather than quietly implied, because the gap between "checked before the
//! order exists" and "checked against the book the order leaves behind" is
//! exactly the distinction `PreTradeChecker` was built on. Closing it needs a
//! proposed order that can carry a *set* of buckets per axis, which is a
//! change to the order type and to every caller that builds one.

use crate::limits::{LimitKind, RiskState};
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The axis a strategy family's aggregate exposure is charged to — §25.3's
/// "per family" row.
pub const FAMILY_AXIS: &str = "family";

/// The axis a systematic factor's exposure is charged to — §25.3's "per
/// factor" row, decomposed from the platform's own tape.
pub const FACTOR_AXIS: &str = "factor";

/// The axis a shared causal driver's exposure is charged to — §25.3's "per
/// causal driver" row, surfaced from the world model's causal graph.
///
/// The row the section calls new, and the one it calls the concentration that
/// ends firms: positions diversified by instrument *and* by factor can still
/// sit downstream of one mechanism, and only the graph can see it.
pub const CAUSAL_DRIVER_AXIS: &str = "causal_driver";

/// The three axes this module may write, in the order it writes them.
///
/// A fixed array rather than an open string: these names also become
/// [`RiskState::unevaluated`] keys, which reach a cycle report, and an axis
/// named from a driver id would put an unbounded cardinality there. Every
/// entry point below refuses a name outside it, so no caller can write a
/// bucket into `sector` — an axis silently overwritten by a second producer
/// is the two-writers failure this workspace records under the feasibility
/// window.
pub const SHARED_CAUSE_AXES: [&str; 3] = [CAUSAL_DRIVER_AXIS, FACTOR_AXIS, FAMILY_AXIS];

/// Whether `axis` is one of the three levels this module produces.
pub fn is_shared_cause_axis(axis: &str) -> bool {
    SHARED_CAUSE_AXES.contains(&axis)
}

/// Whether `kind` reads one of these axes against a denominator they do not
/// move.
///
/// True for the two equity-denominated bucket limits over a shared-cause axis.
/// False for `MaxConcentration`, whose denominator is the sum of the buckets
/// on the axis — see this module's header for why that sum means nothing over
/// an overlapping axis — and false for every limit that reads no axis at all.
/// Exposed so a configuration test can assert a set never names one of these
/// axes on a kind that cannot measure it, rather than a comment asserting
/// nobody will.
pub fn measures_an_overlapping_axis(kind: &LimitKind) -> bool {
    overlapping_axis_measured(kind).is_some()
}

/// The shared-cause axis `kind` measures against equity, if it measures one.
///
/// [`measures_an_overlapping_axis`] is defined in terms of this, so the
/// predicate and the axis a configuration test collects can never name
/// different sets of limit kinds. They did, and the gap was a real one rather
/// than a tidiness: the absence test proving the shipped set caps no family
/// exposure collected axes from [`LimitKind::MaxAxisWeight`] alone, so a
/// `MaxBucketExposure` over the family axis would have passed it — and
/// `MaxBucketExposure` has no early return on an absent axis. It takes its
/// `unwrap_or(zero)` arm, compares a zero nobody computed against its bound,
/// and never fires. That is the `MaxExpectedShortfall` defect under a new
/// name, reachable through the one kind the test could not see.
pub fn overlapping_axis_measured(kind: &LimitKind) -> Option<&str> {
    match kind {
        LimitKind::MaxAxisWeight { axis, .. } | LimitKind::MaxBucketExposure { axis, .. } => {
            is_shared_cause_axis(axis).then_some(axis.as_str())
        }
        _ => None,
    }
}

/// Whether `kind` divides one of these axes by a denominator the axis's own
/// overlap makes meaningless.
///
/// The companion refusal to [`measures_an_overlapping_axis`]: a
/// `MaxConcentration` naming a shared-cause axis is a limit that would fire on
/// a share of a total nobody computed.
pub fn divides_an_overlapping_axis(kind: &LimitKind) -> bool {
    match kind {
        LimitKind::MaxConcentration { axis, .. } => is_shared_cause_axis(axis),
        _ => false,
    }
}

/// What the producers of the three shared-cause levels found, before it is
/// charged against a book.
///
/// Built by whoever can read the sources — the kernel, which holds the causal
/// graph, the tape and the strategy registry — and applied to a [`RiskState`]
/// by [`Self::apply`]. Kept as a separate type rather than as methods on
/// `RiskState` so that the refusal path is the producer's own sentence: this
/// crate has no causal graph and could only invent a reason for somebody
/// else's silence, which is why `RiskState::with_liquidity_horizons` was
/// deleted rather than repaired.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SharedCauseExposure {
    /// axis → driver → instrument → the share of that instrument's notional
    /// the driver accounts for.
    ///
    /// The weight is carried as a [`Decimal`] because it multiplies money.
    /// The crossing from the producer's `f64` — a transmission, a beta —
    /// happens once, in [`Self::attribute`], where a value that cannot be
    /// carried is refused rather than rounded into silence.
    attributed: BTreeMap<String, BTreeMap<String, BTreeMap<String, Decimal>>>,
    /// axis → why its producer could not read its source at all.
    refusals: BTreeMap<String, String>,
}

impl SharedCauseExposure {
    /// Nothing attempted and nothing refused: every axis absent, which
    /// [`Self::apply`] leaves as no claim either way.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that a producer ran over `axis` and found whatever it found.
    ///
    /// Called before any attribution, and called even when there is nothing to
    /// attribute — that is the whole of its purpose. Without it a cycle in
    /// which no held position shares a named cause is indistinguishable from a
    /// cycle in which nothing looked, and the two are the difference between a
    /// control that is idle and a control that is not wired.
    pub fn declare(&mut self, axis: &str) -> Result<()> {
        self.check_axis(axis)?;
        if self.refusals.contains_key(axis) {
            return Err(Error::invalid(format!(
                "the {axis} level has already been refused, so it cannot also be declared read; \
                 a producer states one outcome per axis per state"
            )));
        }
        self.attributed.entry(axis.to_string()).or_default();
        Ok(())
    }

    /// Charge `instrument` to `driver` on `axis`, for `weight` of its notional.
    ///
    /// `weight` is how much of the instrument's exposure the driver accounts
    /// for: a causal edge's transmission, a factor's absolute loading, `1.0`
    /// for a membership that is simply true. It is not bounded above by one —
    /// a beta of 1.4 means what it says, and clamping it would understate the
    /// book precisely where the control matters.
    ///
    /// Refused, never corrected:
    ///
    /// * a weight that is not finite, or is not positive — a negative weight
    ///   would let one position net another's shared cause away, which is the
    ///   illusion this level exists to see through, and a zero one is not an
    ///   attribution at all, for the reason `RiskAggregates::apply_fill`
    ///   refuses a fill of zero notional;
    /// * a weight that is positive as an `f64` and rounds to nothing once
    ///   carried as a [`Decimal`] — the rule [`LimitKind::with_bound`] already
    ///   follows for the same crossing, because a figure that reads positive
    ///   and multiplies to zero drops a position out of a bucket with nothing
    ///   reading as wrong;
    /// * a blank axis, driver or instrument — a bucket under an empty name is
    ///   a real counter nobody chose;
    /// * an axis whose producer has already refused it.
    ///
    /// A producer that finds an instrument immaterial to a driver leaves it
    /// out, and the count [`Self::subjects`] reports then means something.
    pub fn attribute(
        &mut self,
        axis: &str,
        driver: &str,
        instrument: &str,
        weight: f64,
    ) -> Result<()> {
        self.check_axis(axis)?;
        if self.refusals.contains_key(axis) {
            return Err(Error::invalid(format!(
                "the {axis} level was refused by its producer, so {driver} cannot also be \
                 attributed under it; a refusal and an attribution are two claims about one fact"
            )));
        }
        if driver.trim().is_empty() {
            return Err(Error::invalid(format!(
                "an attribution on the {axis} level must name the driver it charges, or the \
                 bucket it creates is one nobody can read"
            )));
        }
        if instrument.trim().is_empty() {
            return Err(Error::invalid(format!(
                "an attribution of {driver} on the {axis} level must name an instrument"
            )));
        }
        if !weight.is_finite() || weight <= 0.0 {
            return Err(Error::invalid(format!(
                "{instrument} cannot be charged to {driver} on the {axis} level at a weight of \
                 {weight}; a weight is a finite positive share of the position's notional, and a \
                 negative one would let one position net another's shared cause away"
            )));
        }
        let carried = Decimal::from_f64(weight).ok_or_else(|| {
            Error::numeric(format!(
                "a weight of {weight} for {instrument} under {driver} on the {axis} level does \
                 not fit the notional it multiplies"
            ))
        })?;
        if !carried.is_positive() {
            return Err(Error::invalid(format!(
                "a weight of {weight} for {instrument} under {driver} on the {axis} level rounds \
                 to {carried} once carried as a notional, so the position would leave the bucket \
                 while the producer believed it had charged it; attribute a material weight or \
                 none"
            )));
        }
        // Summed rather than replaced: two edges from the same cause to the
        // same instrument are two routes by which that cause reaches it, and
        // keeping only the last would silently discard one. `insert` here
        // would make the bucket depend on the order the producer walked its
        // source in, which a replay must not.
        *self
            .attributed
            .entry(axis.to_string())
            .or_default()
            .entry(driver.to_string())
            .or_default()
            .entry(instrument.to_string())
            .or_insert(Decimal::ZERO) += carried;
        Ok(())
    }

    /// Record that the producer of `axis` set out to read its source and could
    /// not, with the refusal that stopped it.
    ///
    /// [`Self::apply`] files this under [`RiskState::unevaluated`], which
    /// `PreTradeChecker::check` turns into a refusal of every order while it
    /// stands. That is deliberately harsher than an empty axis: an empty axis
    /// is a measured statement that the book shares no named cause, and an
    /// unreadable source is no statement at all.
    pub fn refuse(&mut self, axis: &str, reason: impl Into<String>) -> Result<()> {
        self.check_axis(axis)?;
        let reason = reason.into();
        if reason.trim().is_empty() {
            return Err(Error::invalid(format!(
                "a refusal of the {axis} level must say what stopped it; \"the level did not \
                 run\" and \"the level did not run because the graph has absorbed no claim\" are \
                 different sentences to whoever has to fix it"
            )));
        }
        // A refusal supersedes whatever had been seated on the axis, and
        // does not merely sit beside it. A producer that discovers halfway
        // through its walk that it cannot vouch for the level must be able to
        // say so, and the half it had already charged is exactly the number
        // that must not reach a limit: a bucket built from some of the
        // drivers reads as a measurement of all of them. The reverse order is
        // refused rather than superseded — see [`Self::attribute`] — because
        // attributing *after* a refusal is a producer contradicting itself
        // rather than correcting itself.
        self.attributed.remove(axis);
        self.refusals.insert(axis.to_string(), reason);
        Ok(())
    }

    /// Every level refused, with the refusal that stopped the producer.
    ///
    /// For a producer whose own walk failed rather than one of its sources:
    /// it cannot vouch for the level it was on, and it certainly cannot vouch
    /// for the ones it had not reached, so it claims nothing about any of
    /// them and every order is refused while that stands.
    ///
    /// Takes the [`Error`] rather than a string, and is infallible for that
    /// reason: there is no axis name to get wrong and no empty reason to
    /// invent a substitute for. A producer reaching for this has already
    /// failed once, and a constructor that could fail a second time there
    /// would need an arm nobody could test.
    pub fn refusing_all(refusal: &Error) -> Self {
        Self {
            attributed: BTreeMap::new(),
            refusals: SHARED_CAUSE_AXES
                .iter()
                .map(|axis| ((*axis).to_string(), refusal.message().to_string()))
                .collect(),
        }
    }

    /// The axes a producer has declared read, in [`SHARED_CAUSE_AXES`] order.
    pub fn read_axes(&self) -> Vec<&str> {
        self.attributed.keys().map(String::as_str).collect()
    }

    /// The instruments a declared axis charged to at least one driver, so a
    /// caller can say how much of the book the level could see.
    ///
    /// A level that attributed three of a book's forty positions and one that
    /// attributed all forty are different facts, and only one of them is a
    /// risk number — the rule `stress_the_book` already follows for a position
    /// with no beta.
    pub fn subjects(&self, axis: &str) -> BTreeSet<&str> {
        self.attributed
            .get(axis)
            .map(|drivers| {
                drivers
                    .values()
                    .flat_map(|members| members.keys().map(String::as_str))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Charge every attribution against the book `state` describes.
    ///
    /// A driver's bucket is the sum, over the positions the book actually
    /// holds, of the position's absolute notional times the weight that driver
    /// was attributed at. A driver no held position carries produces no bucket
    /// rather than a zero one: a zero bucket breaches nothing and would only
    /// make the axis harder to read.
    ///
    /// **An axis already present in [`RiskState::axis_exposures`] is refused,
    /// not overwritten, and not merged.** Either would give the level two
    /// writers, and a control whose evidence has two writers is the shape this
    /// workspace records under the feasibility window and under
    /// `MaxExpectedShortfall`: it reads as protection while being unable to
    /// say which producer's number the veto fired on. The refusal blocks,
    /// which is the fail-closed direction.
    ///
    /// An axis whose arithmetic leaves the range a notional can carry is
    /// refused the same way, through [`charge`] — see there for why this is the
    /// one site in the module that cannot use `Decimal`'s own operators.
    pub fn apply(&self, mut state: RiskState) -> RiskState {
        for axis in SHARED_CAUSE_AXES {
            if let Some(reason) = self.refusals.get(axis) {
                state = state.with_unevaluated(axis, reason.as_str());
                continue;
            }
            let Some(drivers) = self.attributed.get(axis) else {
                // Never declared: this producer makes no claim about the
                // level, and an absent axis says exactly that.
                continue;
            };
            if state.axis_exposures.contains_key(axis) {
                state = state.with_unevaluated(
                    axis,
                    format!(
                        "the {axis} level was already charged against this state by another \
                         producer, so this one's {} driver(s) are not merged into it; a level \
                         with two writers cannot say whose number a veto fired on",
                        drivers.len()
                    ),
                );
                continue;
            }
            let Some(buckets) = charge(drivers, &state.position_notionals) else {
                state = state.with_unevaluated(
                    axis,
                    format!(
                        "charging the {axis} level against this book left the range a notional                          can carry, so no bucket on it was computed; a weight or a position on                          this level is malformed — repair it at its producer rather than                          trading on a level nobody measured"
                    ),
                );
                continue;
            };
            state.axis_exposures.insert(axis.to_string(), buckets);
        }
        state
    }

    fn check_axis(&self, axis: &str) -> Result<()> {
        if is_shared_cause_axis(axis) {
            return Ok(());
        }
        Err(Error::invalid(format!(
            "{axis:?} is not one of the shared-cause levels {SHARED_CAUSE_AXES:?}; this producer \
             may not write an axis the reference data vouches for, because a bucket silently \
             replaced by a second writer is a limit that fires on a number nobody computed"
        )))
    }
}

/// Charge one axis's drivers against the book, or `None` if the arithmetic left
/// the range a [`Decimal`] carries.
///
/// Checked rather than `*` and `+`, and this is the one site in this module
/// that needs to be. `Decimal`'s operators come from a macro that panics on
/// overflow, [`SharedCauseExposure::apply`] returns a `RiskState` rather than a
/// `Result`, and both operands here come from outside this type — a notional
/// the book reports and a weight a producer chose, neither bounded above.
/// `ToleranceBasis::evaluate` and `Wallet::reconcile` already take the checked
/// form on the same reasoning; this was the one arithmetic site on the order
/// path that did not, and a panic on the order path is not a refusal.
///
/// The caller files the failure under [`RiskState::unevaluated`], which blocks
/// every order — the same fail-closed direction a producer's own refusal takes,
/// because a bucket that could not be summed is no more a measurement than a
/// source that could not be read.
fn charge(
    drivers: &BTreeMap<String, BTreeMap<String, Decimal>>,
    positions: &BTreeMap<String, Decimal>,
) -> Option<BTreeMap<String, Decimal>> {
    let mut buckets: BTreeMap<String, Decimal> = BTreeMap::new();
    for (driver, members) in drivers {
        let mut charged = Decimal::ZERO;
        for (instrument, weight) in members {
            let Some(notional) = positions.get(instrument) else {
                continue;
            };
            charged = charged.checked_add(notional.abs().checked_mul(*weight)?)?;
        }
        if charged.is_positive() {
            buckets.insert(driver.clone(), charged);
        }
    }
    Some(buckets)
}
