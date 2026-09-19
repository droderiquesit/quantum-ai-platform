//! Tax-lot accounting.
//!
//! A position is a stack of lots, each with its own acquisition price and date.
//! When a sale closes part of a position, which lot it closes determines the
//! realised gain and its holding period — so the choice is explicit
//! ([`LotMethod`]) rather than implied by an average.
//!
//! # Why the tax dimension lives here and not in `qip-capital`
//!
//! `qip_capital::ledger::TaxLot` also carries a jurisdiction and a
//! holding-period rule, and this module deliberately does **not** defer to it.
//! The two answer different questions. A `TaxLot` is one *contribution of
//! capital* — a user funding a strategy, with a basis and a currency. Nothing
//! ever sells it, so it has no lot-selection method and no closing side. A
//! [`Lot`] here is one *acquisition of an instrument* inside a
//! [`crate::position::Position`], and a closing fill consumes it under a
//! [`LotSelection`]. Neither type can answer the other's question, and the
//! boundary rules settle it anyway: `qip-portfolio` is a lib and
//! `qip-capital` is a service, so a dependency in that direction does not
//! exist.
//!
//! What this module must not become is a second answer to *when a holding
//! turns long-term*. `qip_capital::ledger::HoldingPeriodRules` is the
//! operator's table of those declarations, and a second table here — keyed on
//! a different `Jurisdiction` type, so the two could never even be reconciled
//! — would be exactly the "two independent claims about the same fact" the
//! platform's sixth principle names. So **nothing here stores a threshold and
//! nothing here has a default one**. A caller that wants holding-period
//! selection hands in a single [`HoldingPeriodTest`], and a lot whose
//! jurisdiction nobody declared reports [`HoldingTerm::Undetermined`] rather
//! than being filed under the more common answer.

use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};
use qip_financial::constraints::Jurisdiction;
use serde::{Deserialize, Serialize};

/// Which lot a closing trade consumes first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LotMethod {
    /// Oldest first. The default in most jurisdictions.
    #[default]
    FirstInFirstOut,
    /// Newest first.
    LastInFirstOut,
    /// Highest cost first, which minimises realised gains.
    HighestCost,
    /// Lowest cost first, which maximises them.
    LowestCost,
}

impl LotMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::FirstInFirstOut => "fifo",
            Self::LastInFirstOut => "lifo",
            Self::HighestCost => "highest_cost",
            Self::LowestCost => "lowest_cost",
        }
    }
}

/// How long a lot has been held, against the rule a jurisdiction declares.
///
/// Ordered `Undetermined`, `Short`, `Long` so a report keyed on this lists
/// the share nobody has a rule for first. That ordering is copied from
/// `qip_capital::ledger::HoldingPeriod` on purpose: the two enums label the
/// same three states for different subjects, and an operator reading a
/// contribution report beside a position report should not have to learn two
/// orders. What is *not* copied is the rule table — see this module's
/// documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum HoldingTerm {
    /// Nobody has declared a rule that applies to this lot, so the platform
    /// does not know. Never folded into [`Self::Short`]: a realised gain
    /// reported as short-term because nobody was asked is a tax position this
    /// repository would be taking on an operator's behalf.
    #[default]
    Undetermined,
    /// Held for less than the declared threshold.
    Short,
    /// Held for at least the declared threshold.
    Long,
}

impl HoldingTerm {
    /// Every state, in the order a report lists them.
    pub const ALL: [Self; 3] = [Self::Undetermined, Self::Short, Self::Long];

    /// The stable token a report or a journal entry carries.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Undetermined => "undetermined",
            Self::Short => "short",
            Self::Long => "long",
        }
    }
}

/// One jurisdiction's declaration of when a holding becomes long-term.
///
/// A single declaration rather than a table, and that is the whole design.
/// The table belongs to whoever collected the operator's declarations; this
/// type is the one row of it that applies to the position in hand, handed in
/// by the caller. There is no `Default`, no constant, and no constructor that
/// invents a span, because a threshold this repository chose would be a tax
/// position nobody consulted a jurisdiction about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoldingPeriodTest {
    jurisdiction: Jurisdiction,
    long_term_after: Duration,
}

impl HoldingPeriodTest {
    /// Record that a lot in `jurisdiction` is long-term once it has been held
    /// for `long_term_after`.
    ///
    /// Refuses a non-positive span rather than clamping it. A threshold of
    /// zero makes every lot long-term at the instant it is acquired, which
    /// distinguishes nothing while reading as a working rule — and a rule
    /// that classifies everything the same way is the shape of control this
    /// platform has shipped before and had to go back for.
    pub fn declare(jurisdiction: Jurisdiction, long_term_after: Duration) -> Result<Self> {
        if long_term_after <= Duration::ZERO {
            return Err(Error::invalid(format!(
                "the long-term threshold declared for {} is {} nanoseconds; declare a positive \
                 span, because a threshold of zero or less makes every lot long-term at the \
                 instant it is acquired and distinguishes nothing",
                jurisdiction.as_str(),
                long_term_after.as_nanos()
            )));
        }
        Ok(Self {
            jurisdiction,
            long_term_after,
        })
    }

    pub fn jurisdiction(&self) -> Jurisdiction {
        self.jurisdiction
    }

    pub fn long_term_after(&self) -> Duration {
        self.long_term_after
    }

    /// The term of a lot acquired at `acquired_at`, seen at `at`, when the
    /// lot sits in `jurisdiction`.
    ///
    /// [`HoldingTerm::Undetermined`] when the lot names no jurisdiction or
    /// names one this declaration is not about — the test refuses to answer
    /// for a jurisdiction it was not declared for rather than applying the
    /// one span it happens to hold. An `at` before `acquired_at` is
    /// [`HoldingTerm::Short`]: a negative age has reached no positive
    /// threshold, and answering `Long` about a moment before the lot existed
    /// is the more surprising of the two.
    pub fn term_of(
        &self,
        jurisdiction: Option<Jurisdiction>,
        acquired_at: Timestamp,
        at: Timestamp,
    ) -> HoldingTerm {
        if jurisdiction != Some(self.jurisdiction) {
            return HoldingTerm::Undetermined;
        }
        if at.since(acquired_at) >= self.long_term_after {
            HoldingTerm::Long
        } else {
            HoldingTerm::Short
        }
    }
}

/// How a closing fill picks the lots it consumes.
///
/// The holding-period arms carry their [`HoldingPeriodTest`] *inside* the
/// arm, so the arm cannot be named without the declaration it needs. That is
/// the structural half of the guarantee: there is no way to ask for
/// holding-period selection and then have it quietly fall back to
/// first-in-first-out because nobody supplied a threshold. A selection that
/// silently degrades to another selection is a control that reads as a tax
/// policy and is not one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LotSelection {
    /// Price and date only — the four methods that need no tax rule.
    Mechanical(LotMethod),
    /// Consume lots that are already long-term before short-term ones, so a
    /// close realises the gain that is usually taxed more lightly. Within a
    /// term, oldest first.
    LongTermFirst(HoldingPeriodTest),
    /// Consume short-term lots first — what a desk harvesting a short-term
    /// loss against short-term gains asks for. Within a term, oldest first.
    ShortTermFirst(HoldingPeriodTest),
}

impl Default for LotSelection {
    fn default() -> Self {
        Self::Mechanical(LotMethod::FirstInFirstOut)
    }
}

impl LotSelection {
    /// The declaration this selection consults, where it consults one.
    pub fn holding_period_test(&self) -> Option<HoldingPeriodTest> {
        match self {
            Self::Mechanical(_) => None,
            Self::LongTermFirst(test) | Self::ShortTermFirst(test) => Some(*test),
        }
    }

    /// The stable token a report carries.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mechanical(method) => method.as_str(),
            Self::LongTermFirst(_) => "long_term_first",
            Self::ShortTermFirst(_) => "short_term_first",
        }
    }
}

/// One acquisition.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Lot {
    /// Signed: positive for a long lot, negative for a short.
    pub quantity: Decimal,
    /// Price paid or received per unit, excluding costs.
    pub price: Decimal,
    /// Transaction costs attributed to the lot.
    pub costs: Decimal,
    pub acquired_at: Timestamp,
    /// Order that created the lot, for lineage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_id: Option<String>,
}

impl Lot {
    pub fn new(quantity: Decimal, price: Decimal, acquired_at: Timestamp) -> Self {
        Self {
            quantity,
            price,
            costs: Decimal::ZERO,
            acquired_at,
            order_id: None,
        }
    }

    pub fn with_costs(mut self, costs: Decimal) -> Self {
        self.costs = costs;
        self
    }

    pub fn with_order(mut self, order_id: impl Into<String>) -> Self {
        self.order_id = Some(order_id.into());
        self
    }

    /// Total outlay including costs. Negative for a short lot's proceeds.
    pub fn cost_basis(&self) -> Decimal {
        self.quantity * self.price + self.costs
    }

    /// Cost per unit including attributed costs.
    pub fn unit_cost(&self) -> Decimal {
        if self.quantity.is_zero() {
            return self.price;
        }
        self.cost_basis()
            .checked_div(self.quantity)
            .unwrap_or(self.price)
    }

    pub fn is_long(&self) -> bool {
        self.quantity.is_positive()
    }
}

/// A closed round trip.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RealisedTrade {
    /// Quantity closed, always positive.
    pub quantity: Decimal,
    pub open_price: Decimal,
    pub close_price: Decimal,
    /// Costs from both the opening and closing sides.
    pub costs: Decimal,
    pub opened_at: Timestamp,
    pub closed_at: Timestamp,
    /// True when the closed lot was long.
    pub was_long: bool,
    /// Whether the gain was short- or long-term, against the declaration the
    /// closing selection carried.
    ///
    /// [`HoldingTerm::Undetermined`] whenever no declaration applied — a
    /// mechanical selection, a position whose instrument names no single
    /// jurisdiction, or a test declared for a different one. Defaulted on
    /// deserialisation so a trade recorded before this field existed reads as
    /// "nobody determined it", which is what was true of it.
    #[serde(default)]
    pub term: HoldingTerm,
}

impl RealisedTrade {
    /// Profit net of costs.
    pub fn realised_pnl(&self) -> Decimal {
        let gross = if self.was_long {
            (self.close_price - self.open_price) * self.quantity
        } else {
            (self.open_price - self.close_price) * self.quantity
        };
        gross - self.costs
    }

    /// Return on the opening notional.
    pub fn return_pct(&self) -> f64 {
        let notional = (self.open_price * self.quantity).abs();
        if !notional.is_positive() {
            return 0.0;
        }
        self.realised_pnl().to_f64() / notional.to_f64()
    }

    pub fn holding_period(&self) -> qip_core::Duration {
        self.closed_at.since(self.opened_at)
    }

    pub fn is_win(&self) -> bool {
        self.realised_pnl().is_positive()
    }
}

/// Close lots under an explicit method.
///
/// The mechanical half of [`close_lots_under`]: no jurisdiction is in play,
/// so every trade it returns carries [`HoldingTerm::Undetermined`].
pub fn close_lots_with(
    lots: &mut Vec<Lot>,
    quantity: Decimal,
    close_price: Decimal,
    close_costs: Decimal,
    closed_at: Timestamp,
    method: LotMethod,
) -> Vec<RealisedTrade> {
    close_lots_under(
        lots,
        quantity,
        close_price,
        close_costs,
        closed_at,
        LotSelection::Mechanical(method),
        None,
    )
}

/// Close lots under a selection, which may consult a holding-period rule.
///
/// `jurisdiction` is the one the lots sit in — an attribute of the
/// instrument, so one value for the whole stack rather than one per lot.
///
/// When the selection names a [`HoldingPeriodTest`] that was declared for a
/// different jurisdiction, or for lots whose jurisdiction is unknown, the
/// ordering cannot be taken and the lots fall back to oldest-first. **That
/// fallback is never silent**: every [`RealisedTrade`] it returns carries
/// [`HoldingTerm::Undetermined`], so a caller who asked for a term-aware
/// close and got a stack of undetermined trades has been told in the output
/// that no rule applied. [`crate::position::Position::declare_selection`]
/// refuses the mismatch earlier still, at the seam where it is knowable.
pub fn close_lots_under(
    lots: &mut Vec<Lot>,
    quantity: Decimal,
    close_price: Decimal,
    close_costs: Decimal,
    closed_at: Timestamp,
    selection: LotSelection,
    jurisdiction: Option<Jurisdiction>,
) -> Vec<RealisedTrade> {
    let mut remaining = quantity.abs();
    if !remaining.is_positive() || lots.is_empty() {
        return Vec::new();
    }
    let total_to_close = remaining;

    // The term of each lot at the close instant, taken once so the sort below
    // and the trades built afterwards cannot disagree about it.
    let terms: Vec<HoldingTerm> = lots
        .iter()
        .map(|lot| {
            selection
                .holding_period_test()
                .map(|test| test.term_of(jurisdiction, lot.acquired_at, closed_at))
                .unwrap_or_default()
        })
        .collect();

    // Order the lots for consumption. Indices are used so the originals can be
    // mutated in place and the surviving order preserved.
    let mut order: Vec<usize> = (0..lots.len()).collect();
    match selection {
        LotSelection::Mechanical(LotMethod::FirstInFirstOut) => {
            order.sort_by_key(|i| lots[*i].acquired_at.as_nanos());
        }
        LotSelection::Mechanical(LotMethod::LastInFirstOut) => {
            order.sort_by_key(|i| std::cmp::Reverse(lots[*i].acquired_at.as_nanos()));
        }
        LotSelection::Mechanical(LotMethod::HighestCost) => {
            order.sort_by(|a, b| lots[*b].unit_cost().cmp(&lots[*a].unit_cost()));
        }
        LotSelection::Mechanical(LotMethod::LowestCost) => {
            order.sort_by(|a, b| lots[*a].unit_cost().cmp(&lots[*b].unit_cost()));
        }
        // Wanted term first, then the lots nobody could classify, then the
        // other term — and oldest first inside each group. An undetermined
        // lot sits in the middle rather than at the front because consuming
        // a lot whose term nobody established, ahead of one that demonstrably
        // has the term the caller asked for, would spend the wrong lot on a
        // guess.
        LotSelection::LongTermFirst(_) => {
            order.sort_by_key(|i| {
                let rank = match terms[*i] {
                    HoldingTerm::Long => 0,
                    HoldingTerm::Undetermined => 1,
                    HoldingTerm::Short => 2,
                };
                (rank, lots[*i].acquired_at.as_nanos())
            });
        }
        LotSelection::ShortTermFirst(_) => {
            order.sort_by_key(|i| {
                let rank = match terms[*i] {
                    HoldingTerm::Short => 0,
                    HoldingTerm::Undetermined => 1,
                    HoldingTerm::Long => 2,
                };
                (rank, lots[*i].acquired_at.as_nanos())
            });
        }
    }

    let mut trades = Vec::new();
    let mut consumed = vec![Decimal::ZERO; lots.len()];

    for index in order {
        if !remaining.is_positive() {
            break;
        }
        let available = lots[index].quantity.abs();
        if !available.is_positive() {
            continue;
        }
        let take = available.min(remaining);
        // Costs are apportioned across the closed quantity pro rata, so a
        // partial close carries its share and no more.
        let opening_costs = lots[index]
            .costs
            .checked_mul(take)
            .and_then(|scaled| scaled.checked_div(available))
            .unwrap_or(Decimal::ZERO);
        let closing_costs = close_costs
            .checked_mul(take)
            .and_then(|scaled| scaled.checked_div(total_to_close))
            .unwrap_or(Decimal::ZERO);

        trades.push(RealisedTrade {
            quantity: take,
            open_price: lots[index].price,
            close_price,
            costs: opening_costs + closing_costs,
            opened_at: lots[index].acquired_at,
            closed_at,
            was_long: lots[index].is_long(),
            term: terms[index],
        });

        consumed[index] = take;
        remaining -= take;
    }

    // Reduce or remove consumed lots, keeping the surviving order.
    let mut surviving = Vec::with_capacity(lots.len());
    for (index, lot) in lots.iter().enumerate() {
        let taken = consumed[index];
        if !taken.is_positive() {
            surviving.push(lot.clone());
            continue;
        }
        let available = lot.quantity.abs();
        if taken >= available {
            continue;
        }
        let remaining_fraction = (available - taken)
            .checked_div(available)
            .unwrap_or(Decimal::ZERO);
        let sign = if lot.is_long() {
            Decimal::ONE
        } else {
            Decimal::NEG_ONE
        };
        surviving.push(Lot {
            quantity: (available - taken) * sign,
            price: lot.price,
            costs: lot.costs * remaining_fraction,
            acquired_at: lot.acquired_at,
            order_id: lot.order_id.clone(),
        });
    }
    *lots = surviving;

    trades
}
