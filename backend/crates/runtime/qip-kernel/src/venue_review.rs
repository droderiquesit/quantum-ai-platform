//! Blueprint §12.3's fourth row: "feasibility rejections cluster on one
//! venue" → "venue withdrawn", and what the LEARN stage may conclude from a
//! window of feasibility refusals.
//!
//! Until this module a feasibility refusal was counted by its gate and never
//! by its venue, on either plane: the desk's `qip_orders_refused_total`
//! carries a `control` and the edge's `qip_edge_refusals_total` a `gate`,
//! and neither says *where*. So the row could not be keyed. The kernel now
//! keeps one bounded window of refusals from both seams — the desk's own
//! order manager and the cells' reports — each naming the venue and the
//! constraint, and this module is the pure arithmetic over that window.
//!
//! What it may conclude is bounded on purpose. [`assess`] answers one
//! question — is there a venue that dominates the recent refusals — and
//! returns a finding, never a mutation. The consequence, withdrawing the
//! venue at both seams, is the kernel's, and it is a *subtraction*: a name
//! is added to a set the order manager refuses against and the whitelist
//! omits. Nothing here, and nothing that reads this, can add a venue. The
//! evidence can only ever say "stop using this one", and putting a venue
//! back is two operators' signatures (ADR 0062).
//!
//! The thresholds are ADR 0055's, by reference: the same minimum sample the
//! sizing discount and the rule review trust, and the same three-in-four
//! share, because the platform has one answer to "how much evidence makes
//! a pattern a finding" and a second, differently-sized answer would be a
//! number nobody could reconcile with the first.

use qip_core::time::Timestamp;
use qip_events::{EventBody, Topic};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// How many recent feasibility refusals the kernel keeps to judge a cluster
/// over.
///
/// A *rate* window, and the one place in the counterfactual machinery where
/// evicting the oldest entry is the right discipline: the question is "what
/// share of recent refusals name this venue", and an old refusal that fell
/// off the end is exactly what "recent" means. The declined and filled
/// queues are queues of *work*, where dropping the oldest would silently
/// choose which veto goes unexamined; this is a sample, where the oldest
/// leaving is the sample staying current.
pub(crate) const FEASIBILITY_WINDOW: usize = 256;

/// Minimum refusals in the window before any venue can be found to
/// dominate it.
///
/// Ten, and by reference rather than a fresh number:
/// [`crate::platform::COUNTERFACTUAL_SIZING_MIN_SAMPLE`], which is
/// `qip_learning_engine::self_model::MINIMUM_SAMPLE`. Eight refusals of
/// which six name one venue is a bad afternoon at one venue; the argument
/// for why ten observations is where a pattern stops being noise is ADR
/// 0055's and is not restated here.
pub const VENUE_WITHDRAWAL_MIN_SAMPLE: usize = crate::platform::COUNTERFACTUAL_SIZING_MIN_SAMPLE;

/// The share of the window one venue must account for before it is a
/// cluster — three in four, mirroring
/// [`crate::platform::COUNTERFACTUAL_SIZING_UNFAVOURABLE_FRACTION`].
///
/// A bare majority would withdraw the busier of two venues on a day both
/// misbehaved; three in four says the refusals are *about this venue* and
/// not about the desk's sizing.
pub const VENUE_WITHDRAWAL_SHARE: f64 =
    crate::platform::COUNTERFACTUAL_SIZING_UNFAVOURABLE_FRACTION;

/// Which plane refused.
///
/// Carried so a withdrawal record can say whether the desk, the cells, or
/// both found the venue infeasible — a cluster the desk alone sees may be
/// the desk's own grid being wrong, which is a different fault from a venue
/// that refuses everyone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeasibilitySeam {
    /// The desk's own order manager, on the central path.
    Desk,
    /// A regional cell, carried on its report.
    Edge,
}

/// One feasibility refusal, as the window keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FeasibilityRefusal {
    /// The venue the order was bound for — the desk broker's name, or the
    /// venue a cell's intent named. Bounded by configuration at both seams.
    pub venue: String,
    /// The `feasibility_*` gate literal that refused. Bounded by the source
    /// constants of the two feasibility modules.
    pub constraint: String,
    pub seam: FeasibilitySeam,
    pub at: Timestamp,
}

/// A venue that dominates the window: the finding [`assess`] returns.
///
/// A record of arithmetic over the window, carrying what a reader needs to
/// check it — the sample, the share, the modal constraint and which seams
/// contributed. It names a venue to *withdraw*; there is no finding in this
/// module that names one to add.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VenueCluster {
    pub venue: String,
    /// The gate this venue was most often refused under.
    pub constraint: String,
    /// Refusals in the window, all venues — the denominator.
    pub sample: usize,
    /// This venue's refusals in the window — the numerator.
    pub count: usize,
    /// `count` over `sample`.
    pub share: f64,
    /// The seams whose refusals of this venue are in the window, in order.
    pub seams: Vec<FeasibilitySeam>,
}

/// Find the venue, if any, that dominates the window and is not already
/// withdrawn.
///
/// Pure. The sample is the whole window, **including a withdrawn venue's
/// entries**: a withdrawn venue's later infeasible orders keep landing here
/// (the desk's feasibility gate runs before the withdrawal check, so the
/// window's denominator stays honest), and excluding them would make the
/// runner-up a cluster of whatever remained — ten refusals, eight at a
/// venue just withdrawn, two at another, and the other would be "100% of
/// the rest" and withdrawn next cycle, and so on through every venue the
/// desk has. The share is computed over everything, so a venue is withdrawn
/// only when it dominates *all* recent refusals.
///
/// Where two venues tie at or above the bar — impossible above one half,
/// possible only at exactly the bar with a share of one half, which the
/// three-in-four bar rules out — the `BTreeMap` order decides, so a replay
/// decides the same.
pub fn assess(window: &[FeasibilityRefusal], withdrawn: &BTreeSet<String>) -> Option<VenueCluster> {
    let sample = window.len();
    if sample < VENUE_WITHDRAWAL_MIN_SAMPLE {
        return None;
    }
    let mut by_venue: BTreeMap<&str, usize> = BTreeMap::new();
    for refusal in window {
        *by_venue.entry(refusal.venue.as_str()).or_insert(0) += 1;
    }
    let (venue, count) = by_venue
        .iter()
        .filter(|(venue, _)| !withdrawn.contains(**venue))
        .max_by_key(|(_, count)| **count)
        .map(|(venue, count)| (*venue, *count))?;
    // usize → f64: a ratio of counts, in the statistics lane.
    let share = count as f64 / sample as f64;
    if share < VENUE_WITHDRAWAL_SHARE {
        return None;
    }
    let mut by_constraint: BTreeMap<&str, usize> = BTreeMap::new();
    let mut seams = Vec::new();
    for refusal in window.iter().filter(|refusal| refusal.venue == venue) {
        *by_constraint
            .entry(refusal.constraint.as_str())
            .or_insert(0) += 1;
        if !seams.contains(&refusal.seam) {
            seams.push(refusal.seam);
        }
    }
    let constraint = by_constraint
        .iter()
        .max_by_key(|(_, count)| **count)
        .map(|(constraint, _)| (*constraint).to_string())?;
    Some(VenueCluster {
        venue: venue.to_string(),
        constraint,
        sample,
        count,
        share,
        seams,
    })
}

/// The record of a venue withdrawn on feasibility evidence — blueprint
/// §12.3's fourth-row consequence, journaled under `venue.withdrawn`
/// *before* the venue is withdrawn at either seam, so a withdrawal the log
/// refused is a withdrawal that did not happen.
///
/// A subtraction on the record: the venue is added to the set the desk's
/// order manager refuses against and the cells' whitelist omits, and
/// nothing reads this record to admit a venue anywhere. Putting the venue
/// back is two operators' signatures under `venue.reinstated` (ADR 0062).
/// The idempotency key is the venue and the cycle, so one review that
/// journals and then fails to withdraw cannot journal twice on retry, and a
/// later cycle that finds the same cluster after a reinstatement is a new
/// withdrawal with its own record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VenueWithdrawal {
    pub venue: String,
    /// The gate the venue was most often refused under.
    pub constraint: String,
    /// Refusals in the window, all venues.
    pub sample: usize,
    /// This venue's refusals in the window.
    pub count: usize,
    /// `count` over `sample`.
    pub share: f64,
    /// The seams whose refusals contributed.
    pub seams: Vec<FeasibilitySeam>,
    pub cycle: u64,
    pub at: Timestamp,
}

impl VenueWithdrawal {
    pub fn of(cluster: &VenueCluster, cycle: u64, at: Timestamp) -> Self {
        Self {
            venue: cluster.venue.clone(),
            constraint: cluster.constraint.clone(),
            sample: cluster.sample,
            count: cluster.count,
            share: cluster.share,
            seams: cluster.seams.clone(),
            cycle,
            at,
        }
    }
}

impl EventBody for VenueWithdrawal {
    const TOPIC: Topic = Topic::VenueWithdrawn;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("venue-withdrawal:{}:{}", self.venue, self.cycle))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn refusal(venue: &str, constraint: &str, seam: FeasibilitySeam) -> FeasibilityRefusal {
        FeasibilityRefusal {
            venue: venue.to_string(),
            constraint: constraint.to_string(),
            seam,
            at: at(),
        }
    }

    /// `count` refusals at `venue`, all under the lot gate, from the desk.
    fn refusals(venue: &str, count: usize) -> Vec<FeasibilityRefusal> {
        (0..count)
            .map(|_| refusal(venue, "feasibility_lot", FeasibilitySeam::Desk))
            .collect()
    }

    #[test]
    fn fewer_than_the_minimum_sample_withdraws_nothing_however_concentrated() {
        // Nine refusals, every one at the same venue: a share of one, on a
        // sample one short of the bar. A finding here would be the same
        // mistake as recalibrating a limit on nine observations — and the
        // consequence is stopping every desk order at that venue, so the
        // sample bar is the whole control. The tenth entry makes it a
        // finding, which is the admitting half that proves the bar is a
        // bar and not a function that refuses everything.
        let nine = refusals("simulated-venue", VENUE_WITHDRAWAL_MIN_SAMPLE - 1);
        assert_eq!(nine.len(), 9, "the premise is nine");
        assert_eq!(
            assess(&nine, &BTreeSet::new()),
            None,
            "a fully concentrated window below the sample bar was withdrawn"
        );

        let ten = refusals("simulated-venue", VENUE_WITHDRAWAL_MIN_SAMPLE);
        let found = assess(&ten, &BTreeSet::new()).expect("ten refusals at one venue cluster");
        assert_eq!(found.venue, "simulated-venue");
        assert_eq!(found.sample, 10);
        assert!((found.share - 1.0).abs() < f64::EPSILON);
        assert_eq!(found.constraint, "feasibility_lot");
        assert_eq!(found.seams, vec![FeasibilitySeam::Desk]);
    }

    #[test]
    fn the_desks_four_gate_literals_are_the_contracts_own() {
        // The desk's feasibility module does not depend on `qip-contracts`
        // and declares its four literals itself; the centre admits a cell's
        // refusal by the contracts' eight. This kernel sees both, so it is
        // where a desk literal drifting from the shared vocabulary — a
        // refusal the desk counts under one name and the centre would file
        // under `other` — fails a test rather than a dashboard.
        use qip_contracts::feasibility as shared;
        use qip_execution_engine::feasibility as desk;
        assert_eq!(desk::GATE_MINIMUM_QUANTITY, shared::GATE_MINIMUM_QUANTITY);
        assert_eq!(desk::GATE_MINIMUM_NOTIONAL, shared::GATE_MINIMUM_NOTIONAL);
        assert_eq!(desk::GATE_LOT, shared::GATE_LOT);
        assert_eq!(desk::GATE_TICK, shared::GATE_TICK);
        for gate in shared::DESK_GATES {
            assert!(
                shared::EDGE_GATES.contains(&gate),
                "the desk refuses under {gate}, which the centre would not admit from a cell"
            );
        }
    }

    #[test]
    fn a_venue_below_the_share_bar_is_not_withdrawn() {
        // Twelve refusals: eight at one venue is two thirds, below the
        // three-in-four bar, and stays; nine of twelve is exactly the bar
        // and clears it. Both directions of the boundary, so a `>=` that
        // became `>` fails on the second and one that became a lower bar
        // fails on the first.
        let mut below = refusals("alpha", 8);
        below.extend(refusals("beta", 4));
        assert_eq!(below.len(), 12);
        assert_eq!(
            assess(&below, &BTreeSet::new()),
            None,
            "two thirds of the window was read as a cluster"
        );

        let mut at_bar = refusals("alpha", 9);
        at_bar.extend(refusals("beta", 3));
        assert_eq!(at_bar.len(), 12);
        let found = assess(&at_bar, &BTreeSet::new()).expect("nine of twelve is the bar");
        assert_eq!(found.venue, "alpha");
        assert!((found.share - 0.75).abs() < f64::EPSILON);
    }

    #[test]
    fn withdrawing_one_venue_does_not_make_the_runner_up_a_cluster_of_the_remainder() {
        // The cascade this refuses: ten refusals, eight at alpha and two at
        // beta. Alpha is withdrawn. If the next review excluded alpha's
        // entries from the denominator, beta would be "two of two" — a
        // share of one on a sample the bar was never applied to — and be
        // withdrawn on the next cycle, and the desk's last venue after
        // that. The share is over the whole window, so beta is two of ten
        // and stays.
        let mut window = refusals("alpha", 8);
        window.extend(refusals("beta", 2));
        let first = assess(&window, &BTreeSet::new()).expect("alpha dominates");
        assert_eq!(first.venue, "alpha", "the premise failed");
        assert!((first.share - 0.8).abs() < f64::EPSILON);

        let withdrawn = BTreeSet::from(["alpha".to_string()]);
        assert_eq!(
            assess(&window, &withdrawn),
            None,
            "the runner-up was read as a cluster of what remained"
        );

        // And beta genuinely dominating the whole window is still found,
        // so the guard above is the denominator and not a refusal of every
        // second venue.
        window.extend(refusals("beta", 30));
        let second = assess(&window, &withdrawn).expect("beta dominates the whole window");
        assert_eq!(second.venue, "beta");
        assert_eq!(second.sample, 40);
        assert!((second.share - 0.8).abs() < f64::EPSILON);
    }

    #[test]
    fn an_already_withdrawn_venue_is_not_withdrawn_twice() {
        // A withdrawn venue's later refusals keep landing in the window —
        // the desk's feasibility gate runs before its withdrawal check, on
        // purpose, so the denominator stays honest — and every review would
        // find the same cluster again. Without the exclusion each cycle
        // would journal a fresh withdrawal of a venue already withdrawn, and
        // a reinstatement's two signatures would be undone by the very next
        // LEARN pass.
        let window = refusals("alpha", 12);
        assert!(
            assess(&window, &BTreeSet::new()).is_some(),
            "the premise failed: alpha does not dominate"
        );
        let withdrawn = BTreeSet::from(["alpha".to_string()]);
        assert_eq!(
            assess(&window, &withdrawn),
            None,
            "a venue already withdrawn was found again"
        );
    }
}
