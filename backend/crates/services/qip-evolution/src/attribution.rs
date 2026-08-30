//! Where the money actually came from, against where it was expected from.
//!
//! [`qip_contracts::NetEdge`] already decomposes an expectation into a gross
//! figure and nine deductions, and already refuses to call itself complete
//! while any of them is unconsidered. Attribution is the same decomposition
//! run backwards over what happened: the difference between what a trade was
//! supposed to earn and what it earned, split into the gross surprise and one
//! surprise per deduction kind.
//!
//! The arithmetic is exact. Every figure here is a [`Decimal`] and the parts
//! sum to the whole to the cent — [`Attribution::identity_holds`] asserts it,
//! and it is not an approximation that happens to be close. An attribution
//! with a floating-point residual is an attribution nobody trusts, and one
//! nobody trusts gets replaced by a story.
//!
//! What attribution is *for* is the next decision. A strategy whose gross edge
//! arrived in full and whose slippage was three times what was modelled did
//! not have a bad idea; it had a good idea and a wrong cost model, and those
//! two findings lead to opposite actions. Feeding the first into
//! [`crate::scoring`] and the second into [`crate::cost_model`] is the
//! division this module exists to make possible.

use qip_contracts::edge::{Deduction, DeductionKind, NetEdge};
use qip_contracts::signal::StrategyId;
use qip_core::Decimal;
use qip_core::error::Result;

use crate::scoring::Outcome;

/// One trade, as expected and as it turned out.
#[derive(Clone, Debug, PartialEq)]
pub struct RealisedTrade {
    pub strategy: StrategyId,
    /// The circumstance the trade happened in — a regime, a session, a venue.
    /// Attribution without one aggregates a calm week and a crisis into a
    /// number that describes neither.
    pub context: String,
    /// What the platform expected, decomposed. Must be complete.
    pub expected: NetEdge,
    /// The gross edge that actually materialised.
    pub realised_gross: Decimal,
    /// What was actually paid, by kind.
    pub realised_deductions: Vec<Deduction>,
}

/// The decomposition of one trade's outcome.
#[derive(Clone, Debug, PartialEq)]
pub struct Attribution {
    strategy: StrategyId,
    context: String,
    expected_net: Decimal,
    realised_net: Decimal,
    gross_surprise: Decimal,
    /// Realised minus expected, per kind. Positive means it cost more than it
    /// was supposed to.
    cost_surprise: Vec<(DeductionKind, Decimal)>,
}

impl Attribution {
    /// Decompose a realised trade.
    ///
    /// Refuses an expectation that did not consider every deduction kind. An
    /// attribution against an incomplete expectation would silently book the
    /// unconsidered costs as a gross shortfall, which is how a compute bill
    /// gets attributed to the alpha.
    pub fn of(trade: &RealisedTrade) -> Result<Self> {
        trade.expected.require_complete()?;

        let cost_surprise = DeductionKind::all()
            .into_iter()
            .map(|kind| {
                let expected = total_of(trade.expected.deductions(), kind);
                let realised = total_of(&trade.realised_deductions, kind);
                (kind, realised - expected)
            })
            .collect();

        let realised_total = trade
            .realised_deductions
            .iter()
            .fold(Decimal::ZERO, |sum, deduction| sum + deduction.amount);

        Ok(Self {
            strategy: trade.strategy.clone(),
            context: trade.context.clone(),
            expected_net: trade.expected.net(),
            realised_net: trade.realised_gross - realised_total,
            gross_surprise: trade.realised_gross - trade.expected.gross_edge(),
            cost_surprise,
        })
    }

    pub const fn strategy(&self) -> &StrategyId {
        &self.strategy
    }

    pub fn context(&self) -> &str {
        &self.context
    }

    pub const fn expected_net(&self) -> Decimal {
        self.expected_net
    }

    pub const fn realised_net(&self) -> Decimal {
        self.realised_net
    }

    /// Realised gross minus expected gross. This is the part that is about the
    /// idea being right.
    pub const fn gross_surprise(&self) -> Decimal {
        self.gross_surprise
    }

    /// Per-kind cost surprises, in the fixed order [`DeductionKind::all`]
    /// gives, so two attributions line up column for column.
    pub fn cost_surprise(&self) -> &[(DeductionKind, Decimal)] {
        &self.cost_surprise
    }

    /// What one kind cost above what was modelled.
    pub fn surprise_of(&self, kind: DeductionKind) -> Decimal {
        self.cost_surprise
            .iter()
            .find(|(held, _)| *held == kind)
            .map_or(Decimal::ZERO, |(_, amount)| *amount)
    }

    /// Everything paid above what was modelled, across every kind.
    pub fn total_cost_surprise(&self) -> Decimal {
        self.cost_surprise
            .iter()
            .fold(Decimal::ZERO, |sum, (_, amount)| sum + *amount)
    }

    /// The kind that overran its model by the most.
    ///
    /// The single most useful line in an attribution: it names what to fix.
    pub fn worst_overrun(&self) -> Option<(DeductionKind, Decimal)> {
        self.cost_surprise
            .iter()
            .filter(|(_, amount)| amount.is_positive())
            .copied()
            .reduce(|worst, next| if next.1 > worst.1 { next } else { worst })
    }

    /// Whether the parts sum to the whole, exactly.
    ///
    /// Expected net, plus what the idea earned above expectation, less what
    /// the costs overran by, is what was actually made. There is no residual
    /// term and no tolerance: with [`Decimal`] this is an identity, and a
    /// version of it that needed a tolerance would be hiding a mistake.
    pub fn identity_holds(&self) -> bool {
        self.expected_net + self.gross_surprise - self.total_cost_surprise() == self.realised_net
    }

    /// How much of the expected net actually arrived, in `[0, 1]`.
    ///
    /// A statistic and therefore an `f64`; the money it is derived from stays
    /// exact. Zero where nothing was expected and nothing arrived, because a
    /// trade that was expected to make nothing cannot have delivered a
    /// fraction of it.
    pub fn realisation(&self) -> f64 {
        let expected = self.expected_net.to_f64();
        if expected <= 0.0 {
            return f64::from(u8::from(self.realised_net.is_positive()));
        }
        (self.realised_net.to_f64() / expected).clamp(0.0, 1.0)
    }

    /// This attribution as something [`crate::scoring::Scoreboard`] can take.
    pub fn outcome(&self) -> Outcome {
        Outcome::new(self.strategy.as_str(), &self.context, self.realisation())
    }

    pub fn summarise(&self) -> String {
        let worst = self.worst_overrun().map_or_else(
            || "no overrun".to_string(),
            |(kind, amount)| format!("{} over by {amount}", kind.as_str()),
        );
        format!(
            "{} in {}: expected {} made {} (gross surprise {}, costs over by {}); {worst}",
            self.strategy,
            self.context,
            self.expected_net,
            self.realised_net,
            self.gross_surprise,
            self.total_cost_surprise()
        )
    }
}

/// Every attribution recorded, and the aggregates worth reading off them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AttributionLedger {
    entries: Vec<Attribution>,
}

impl AttributionLedger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, attribution: Attribution) {
        self.entries.push(attribution);
    }

    pub fn entries(&self) -> &[Attribution] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Total overrun per deduction kind, across every recorded trade.
    ///
    /// The input to a cost-model revision: a kind that overruns on one trade
    /// is noise, and a kind that overruns on four hundred is a model that is
    /// wrong.
    pub fn overrun_by_kind(&self) -> Vec<(DeductionKind, Decimal)> {
        DeductionKind::all()
            .into_iter()
            .map(|kind| {
                let total = self
                    .entries
                    .iter()
                    .fold(Decimal::ZERO, |sum, entry| sum + entry.surprise_of(kind));
                (kind, total)
            })
            .collect()
    }

    /// Outcomes for every recorded trade, ready for a scoreboard.
    pub fn outcomes(&self) -> Vec<Outcome> {
        self.entries.iter().map(Attribution::outcome).collect()
    }
}

fn total_of(deductions: &[Deduction], kind: DeductionKind) -> Decimal {
    deductions
        .iter()
        .filter(|deduction| deduction.kind == kind)
        .fold(Decimal::ZERO, |sum, deduction| sum + deduction.amount)
}
