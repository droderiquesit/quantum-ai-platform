//! LEARN's outcome attribution with real P&L, keyed by who produced it
//! (blueprint §22, §22.1; R12-MODEL-6003, R12-MODEL-6021).
//!
//! # The failure this replaces
//!
//! `Platform::attribute` built every position as if it opened and closed at
//! its own fill price: `opening_price = closing_price = fill.price`,
//! `realised_pnl`, `spread_cost` and `impact_cost` zero. The decomposition
//! closed — it closed on commissions, because commissions were the only
//! non-zero input — so LEARN graded every thesis on what it cost to trade and
//! never on what it earned. And it grouped by hypothesis alone, so nothing
//! could say which model, source, capital policy, rule or path to credit.
//!
//! # What a position is now
//!
//! One position per fill, marked to the latest observed close of its
//! instrument:
//!
//! * the fill's signed quantity is opened at the order's **arrival price**
//!   (the reference the decision was taken at) and held to the **mark**;
//! * the slippage from arrival to the fill price is the implementation cost,
//!   split into the half-spread quoted *when the order was released* (the
//!   spread component) and the remainder (impact, which may be negative — a
//!   fill better than arrival is price improvement, not a cost);
//! * commission is the fill's own cost.
//!
//! So a fill's total is `signed_qty × (mark − fill_price) − commission`, and
//! over a book that bought and later sold the same quantity the per-fill
//! totals sum to the realised P&L exactly. Two independent claims are then
//! compared: the per-fill sum, and the **book's** cash-plus-holdings total
//! (`−Σ signed_qty × price − Σ commission + Σ_instrument position × mark`).
//! They agree by algebra and differ only by per-product rounding; a gap
//! larger than one unit per fill is a refusal naming both figures (principle
//! 6), not a quietly preferred number.
//!
//! A fill whose instrument has no observed bar is marked at its own price —
//! no move is claimed — and counted as **unmarked** in the stage detail, so
//! "flat" and "never priced" read differently.
//!
//! # Keys, stamped at release
//!
//! [`AttributionLineage`] records, when REASON forms a hypothesis, the
//! forecasters behind it and its evidence origins; and, when an order is
//! accepted, the strategy, capital policy, risk rule set and execution path
//! it went out under. The keys are written at the instant they were true —
//! reading them at LEARN, from whatever policy is in force then, would credit
//! today's policy with yesterday's fills.
//!
//! Advisory: nothing here sizes or sends.

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::time::Timestamp;
use qip_events::{EventBody, Topic};
use qip_learning_engine::attribution::{Attribution, Attributor, PositionPeriod};
use qip_learning_engine::keyed_attribution::{AttributionKeys, Dimension, KeyedAttribution};
use qip_reasoning_engine::hypothesis::Hypothesis;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

/// The producer every attribution record carries.
pub const ATTRIBUTION_ORIGIN: &str = "kernel/attribution";

/// Hypotheses whose lineage is remembered, oldest evicted first.
pub const LINEAGE_HYPOTHESES: usize = 4_096;

/// Orders whose keys are remembered, oldest evicted first. A fill whose
/// order has been evicted is attributed under `unattributed` and counted.
pub const LINEAGE_ORDERS: usize = 8_192;

/// Who and what stood behind one hypothesis, captured at formation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HypothesisLineage {
    /// `detector:<class>`, `analyst:<agent>` and `model:<reference>`.
    pub forecasters: Vec<String>,
    /// The origins of the evidence it was formed on.
    pub evidence_sources: Vec<String>,
}

/// What an accepted order went out under, captured at acceptance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReleaseContext {
    pub strategy: String,
    pub capital_policy: String,
    pub risk_rule: String,
    pub execution_path: String,
    /// The instrument's latest quoted spread at release, in basis points.
    pub quoted_spread_bps: Option<f64>,
    /// The three quantities the order passed through on its way out, where it
    /// came from a constructed proposal leg; `None` for an order submitted
    /// directly, which has no proposal to be narrowed from.
    pub quantities: Option<ReleaseQuantities>,
}

/// The three order quantities ADR 0148 decision 5 stamps at release, each read
/// off the stage that produced it. Unsigned: the order's side carries the
/// direction.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReleaseQuantities {
    /// What the thesis implied at the budget before any narrowing multiplier.
    pub reference: Decimal,
    /// `ProposalLeg.quantity`: after the multipliers and caps.
    pub sized: Decimal,
    /// The order's quantity at release, after lot rounding.
    pub released: Decimal,
}

impl ReleaseQuantities {
    /// Refused unless every quantity is positive: a zero is a leg that is not
    /// an order, and a negative one is a side applied twice.
    pub fn new(reference: Decimal, sized: Decimal, released: Decimal) -> Result<Self> {
        if !reference.is_positive() || !sized.is_positive() || !released.is_positive() {
            return Err(Error::invalid(
                "the reference, sized and released quantities of an order are all positive; the side carries the direction",
            ));
        }
        Ok(Self {
            reference,
            sized,
            released,
        })
    }
}

/// How the held move of one order's fills splits across the stages that set
/// its size (ADR 0148 decision 5). `move = mark - arrival`, each term signed
/// by the order's side, and the four sum to `side * filled * move`.
#[derive(Clone, Debug, PartialEq)]
pub struct HeldMoveSplit {
    /// `reference * move`: the thesis, at the unnarrowed budget.
    pub prediction: Decimal,
    /// `(sized - reference) * move`: what the narrowing multipliers and caps cost.
    pub sizing: Decimal,
    /// `(released - sized) * move`: what lot rounding and the gate took after
    /// sizing. A **measured** zero when nothing stood between them.
    pub allocation: Decimal,
    /// `-(released - filled) * move`: the part of the order that did not fill.
    pub unfilled: Decimal,
    /// `side * filled * move`, computed directly and not as the sum above.
    pub total: Decimal,
    /// What it cost to get the fills, arrival to fill plus commission, as a
    /// negative figure: the execution component. It is the term the per-fill
    /// attribution already measures, carried here so the five components add
    /// to the order's whole outcome.
    pub execution: Decimal,
}

impl HeldMoveSplit {
    /// Split the held move of `filled` units (over every fill of the order)
    /// that moved from `arrival` to `mark`.
    ///
    /// The `unfilled` term is the balance against the directly computed total
    /// rather than `-(released - filled) * move`, because `Decimal` rounds each
    /// product to nine places and four rounded products need not telescope. The
    /// balance differs from the stated definition by rounding alone, and the
    /// test pins that; what it buys is that the four terms sum to the fill's
    /// own held move to the last digit, so no residual is hidden in a fifth
    /// line.
    pub fn of(
        quantities: &ReleaseQuantities,
        side: i64,
        filled: Decimal,
        arrival: Decimal,
        mark: Decimal,
        execution: Decimal,
    ) -> Result<Self> {
        if side != 1 && side != -1 {
            return Err(Error::invalid(format!("a side is +1 or -1, not {side}")));
        }
        if !filled.is_positive() || filled > quantities.released {
            return Err(Error::invalid(format!(
                "{filled} filled against {} released; a fill is positive and no larger than its order",
                quantities.released
            )));
        }
        let signed_move = Decimal::from_int(side) * (mark - arrival);
        let prediction = quantities.reference * signed_move;
        let sizing = (quantities.sized - quantities.reference) * signed_move;
        let allocation = (quantities.released - quantities.sized) * signed_move;
        let total = filled * signed_move;
        let unfilled = total - prediction - sizing - allocation;
        Ok(Self {
            prediction,
            sizing,
            allocation,
            unfilled,
            total,
            execution,
        })
    }

    /// The four held-move terms added up; equal to [`Self::total`] by
    /// construction.
    pub fn sum(&self) -> Decimal {
        self.prediction + self.sizing + self.allocation + self.unfilled
    }

    /// The order's whole outcome: the held move's four terms and the cost of
    /// executing. The per-fill attribution measures the same figure on its own
    /// (`PositionPnl::total`), which is what makes this a decomposition and not
    /// a second opinion.
    pub fn outcome(&self) -> Decimal {
        self.sum() + self.execution
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Stamped {
    keys: AttributionKeys,
    quoted_spread_bps: Option<f64>,
    quantities: Option<ReleaseQuantities>,
}

/// The keys each position will be attributed under, written when they were
/// true. Bounded on both axes.
#[derive(Clone, Debug, Default)]
pub struct AttributionLineage {
    hypotheses: BTreeMap<String, HypothesisLineage>,
    hypothesis_order: VecDeque<String>,
    orders: BTreeMap<String, Stamped>,
    order_order: VecDeque<String>,
    /// How many fills the last journaled record covered, so a cycle with no
    /// new fill writes nothing.
    journaled_fills: Option<usize>,
    /// The most recent pass, for the API and the tests.
    last: Option<AttributionPass>,
}

impl AttributionLineage {
    pub fn new() -> Self {
        Self::default()
    }

    /// The forecaster ids a hypothesis credits, in the self-model's naming:
    /// the detector of its class, each analyst whose run contributed
    /// (contributors are `run-<agent>-<sequence>`, matched against the
    /// roster so a run id is never mistaken for an agent), and each model
    /// reference it depended on.
    pub fn forecasters_of(hypothesis: &Hypothesis, roster: &[String]) -> Vec<String> {
        let mut forecasters = vec![format!("detector:{}", hypothesis.class)];
        for contributor in &hypothesis.contributors {
            let Some(rest) = contributor.strip_prefix("run-") else {
                continue;
            };
            if let Some(agent) = roster.iter().find(|id| {
                rest.strip_prefix(id.as_str())
                    .and_then(|tail| tail.strip_prefix('-'))
                    .is_some_and(|sequence| sequence.parse::<u64>().is_ok())
            }) {
                forecasters.push(format!("analyst:{agent}"));
            }
        }
        for model in &hypothesis.models {
            forecasters.push(format!("model:{model}"));
        }
        forecasters.sort();
        forecasters.dedup();
        forecasters
    }

    /// Remember who stood behind `hypothesis`.
    pub fn note_hypothesis(&mut self, hypothesis: &Hypothesis, roster: &[String]) {
        let id = hypothesis.hypothesis_id.as_str().to_string();
        let mut sources: Vec<String> = hypothesis
            .evidence
            .iter()
            .map(|evidence| evidence.origin.clone())
            .filter(|origin| !origin.trim().is_empty())
            .collect();
        sources.sort();
        sources.dedup();
        let lineage = HypothesisLineage {
            forecasters: Self::forecasters_of(hypothesis, roster),
            evidence_sources: sources,
        };
        if self.hypotheses.insert(id.clone(), lineage).is_none() {
            self.hypothesis_order.push_back(id);
            while self.hypothesis_order.len() > LINEAGE_HYPOTHESES {
                if let Some(oldest) = self.hypothesis_order.pop_front() {
                    self.hypotheses.remove(&oldest);
                }
            }
        }
    }

    /// The most recent LEARN pass's attribution, if any fill has been
    /// attributed.
    pub fn last(&self) -> Option<&AttributionPass> {
        self.last.as_ref()
    }

    /// Keep `pass` as the most recent.
    pub fn remember(&mut self, pass: AttributionPass) {
        self.last = Some(pass);
    }

    pub fn lineage(&self, hypothesis: &str) -> Option<&HypothesisLineage> {
        self.hypotheses.get(hypothesis)
    }

    /// Stamp an accepted order with the seven keys. The model and source
    /// keys come from the lineage of each hypothesis it expresses; a
    /// hypothesis with no remembered lineage contributes `unattributed`
    /// rather than nothing.
    pub fn note_release(
        &mut self,
        order_id: &str,
        hypotheses: &[String],
        context: ReleaseContext,
    ) -> Result<()> {
        let mut models = Vec::new();
        let mut sources = Vec::new();
        for hypothesis in hypotheses {
            match self.hypotheses.get(hypothesis) {
                Some(lineage) => {
                    models.extend(lineage.forecasters.iter().cloned());
                    sources.extend(lineage.evidence_sources.iter().cloned());
                }
                None => {
                    models.push(qip_learning_engine::keyed_attribution::UNATTRIBUTED.to_string());
                    sources.push(qip_learning_engine::keyed_attribution::UNATTRIBUTED.to_string());
                }
            }
        }
        let keys = AttributionKeys::new(
            context.strategy,
            models,
            hypotheses.to_vec(),
            sources,
            context.capital_policy,
            context.risk_rule,
            context.execution_path,
        )?;
        let stamped = Stamped {
            keys,
            quoted_spread_bps: context
                .quoted_spread_bps
                .filter(|s| s.is_finite() && *s >= 0.0),
            quantities: context.quantities,
        };
        if self.orders.insert(order_id.to_string(), stamped).is_none() {
            self.order_order.push_back(order_id.to_string());
            while self.order_order.len() > LINEAGE_ORDERS {
                if let Some(oldest) = self.order_order.pop_front() {
                    self.orders.remove(&oldest);
                }
            }
        }
        Ok(())
    }

    /// The keys stamped on every remembered order, by order id.
    pub fn keys(&self) -> BTreeMap<String, AttributionKeys> {
        self.orders
            .iter()
            .map(|(id, stamped)| (id.clone(), stamped.keys.clone()))
            .collect()
    }

    /// The quantities an order was stamped with at release, if it came from a
    /// constructed leg and has not been evicted.
    pub fn release_quantities(&self, order_id: &str) -> Option<ReleaseQuantities> {
        self.orders
            .get(order_id)
            .and_then(|stamped| stamped.quantities)
    }

    fn quoted_spread_bps(&self, order_id: &str) -> Option<f64> {
        self.orders
            .get(order_id)
            .and_then(|stamped| stamped.quoted_spread_bps)
    }
}

/// One fill, with what LEARN knows about it now.
#[derive(Clone, Debug, PartialEq)]
pub struct FillMark {
    pub order_id: String,
    pub instrument: String,
    pub at: Timestamp,
    /// `+1` for a buy, `-1` for a sell.
    pub side: i64,
    /// Always positive.
    pub quantity: Decimal,
    pub price: Decimal,
    /// The order's arrival (decision) price.
    pub arrival: Decimal,
    pub commission: Decimal,
    /// The latest observed close of the instrument, if any bar was seen.
    pub mark: Option<Decimal>,
    pub hypotheses: Vec<String>,
    /// The instrument's market beta, where the overlap supports one.
    pub beta: Option<f64>,
}

/// One LEARN pass's attribution.
#[derive(Clone, Debug, PartialEq)]
pub struct AttributionPass {
    pub attribution: Attribution,
    pub keyed: KeyedAttribution,
    /// Fills marked at their own price because no bar was observed.
    pub unmarked: usize,
    /// The book's cash-plus-holdings total, the independent claim.
    pub book_total: Decimal,
    /// Each stamped order's held move split by the stage that set its size.
    /// An order with no stamped quantities is absent, not zero.
    pub held_moves: BTreeMap<String, HeldMoveSplit>,
}

/// Build one position per fill. See the module doc for the arithmetic.
pub fn periods(
    fills: &[FillMark],
    lineage: &AttributionLineage,
    factor_return: Option<f64>,
) -> Result<Vec<PositionPeriod>> {
    fills
        .iter()
        .map(|fill| {
            if fill.side != 1 && fill.side != -1 {
                return Err(Error::invalid(format!(
                    "fill on order {} has side {}; a side is +1 or -1",
                    fill.order_id, fill.side
                )));
            }
            if !fill.quantity.is_positive() {
                return Err(Error::invalid(format!(
                    "fill on order {} has a non-positive quantity {}",
                    fill.order_id, fill.quantity
                )));
            }
            let signed = fill.quantity * Decimal::from_int(fill.side);
            let mark = fill.mark.unwrap_or(fill.price);
            // Slippage from arrival to fill, as a cost (positive = paid).
            let slippage = signed * (fill.price - fill.arrival);
            // The half-spread quoted at release, on the arrival notional.
            // Statistic-to-money crossing: basis points are f64 and the
            // result is money from here on.
            let spread = match lineage.quoted_spread_bps(&fill.order_id) {
                Some(bps) => Decimal::from_f64(bps / 2.0 / 10_000.0)
                    .map(|fraction| fill.quantity * fill.arrival * fraction)
                    .ok_or_else(|| {
                        Error::numeric(format!(
                            "the spread {bps} bp quoted for order {} does not convert",
                            fill.order_id
                        ))
                    })?,
                None => Decimal::ZERO,
            };
            let impact = slippage - spread;
            let (factor_returns, factor_betas) = match (fill.beta, factor_return) {
                (Some(beta), Some(value)) => (
                    BTreeMap::from([(qip_risk::market_factor::MARKET_FACTOR.to_string(), value)]),
                    BTreeMap::from([(qip_risk::market_factor::MARKET_FACTOR.to_string(), beta)]),
                ),
                _ => (BTreeMap::new(), BTreeMap::new()),
            };
            Ok(PositionPeriod {
                object_id: fill.order_id.clone(),
                hypotheses: fill.hypotheses.clone(),
                opening_quantity: Decimal::ZERO,
                opening_price: fill.arrival,
                closing_quantity: signed,
                closing_price: mark,
                decision_price: fill.arrival,
                traded_quantity: signed,
                traded_price: fill.arrival,
                commission: fill.commission,
                spread_cost: spread,
                impact_cost: impact,
                income: Decimal::ZERO,
                financing: Decimal::ZERO,
                realised_pnl: Decimal::ZERO,
                factor_returns,
                factor_betas,
                contract_multiplier: Decimal::from_int(1),
            })
        })
        .collect()
}

/// The book's own total: cash paid and received, less commission, plus each
/// instrument's net position at its mark. Computed without reference to the
/// per-fill positions, so it can disagree with them.
pub fn book_total(fills: &[FillMark]) -> Decimal {
    let mut cash = Decimal::ZERO;
    let mut positions: BTreeMap<&str, (Decimal, Decimal)> = BTreeMap::new();
    for fill in fills {
        let signed = fill.quantity * Decimal::from_int(fill.side);
        cash -= signed * fill.price;
        cash -= fill.commission;
        let entry = positions
            .entry(fill.instrument.as_str())
            .or_insert((Decimal::ZERO, fill.mark.unwrap_or(fill.price)));
        entry.0 += signed;
    }
    let holdings: Decimal = positions
        .values()
        .map(|(quantity, mark)| *quantity * *mark)
        .sum();
    cash + holdings
}

/// Attribute every fill and join the result to its keys.
pub fn run(
    attributor: &Attributor,
    fills: &[FillMark],
    lineage: &AttributionLineage,
    factor_return: Option<f64>,
    equity: Decimal,
    now: Timestamp,
) -> Result<AttributionPass> {
    let periods = periods(fills, lineage, factor_return)?;
    let per_fill: Decimal = periods
        .iter()
        .map(|p| {
            p.closing_quantity * p.closing_price
                - p.traded_quantity * p.traded_price
                - p.commission
                - p.spread_cost
                - p.impact_cost
        })
        .sum();
    let book = book_total(fills);
    let tolerance = Decimal::from_raw(i128::try_from(fills.len()).unwrap_or(i128::MAX).max(1));
    if (book - per_fill).abs() > tolerance {
        return Err(Error::numeric(format!(
            "the book's cash-and-holdings P&L is {book} and the fills attribute {per_fill}; two \
             accounts of the same fills disagree by more than rounding, so neither is reported"
        )));
    }
    let attribution = attributor.attribute(&periods, per_fill, equity, now, now)?;
    let keyed = KeyedAttribution::join(&attribution, &lineage.keys())?;
    let unmarked = fills.iter().filter(|fill| fill.mark.is_none()).count();
    // Per order, over all of its fills: the split's `unfilled` term is about
    // the order, and a fill-by-fill split would call every partial fill's
    // remainder unfilled.
    let mut filled: BTreeMap<&str, (Decimal, Decimal, &FillMark)> = BTreeMap::new();
    for fill in fills {
        let entry =
            filled
                .entry(fill.order_id.as_str())
                .or_insert((Decimal::ZERO, Decimal::ZERO, fill));
        entry.0 += fill.quantity;
        // Slippage from arrival to fill, paid, and the commission: the same
        // two costs `periods` charges, as a negative contribution.
        entry.1 -= Decimal::from_int(fill.side) * fill.quantity * (fill.price - fill.arrival)
            + fill.commission;
    }
    let mut held_moves = BTreeMap::new();
    for (order_id, (quantity, execution, fill)) in filled {
        if let Some(quantities) = lineage.release_quantities(order_id) {
            held_moves.insert(
                order_id.to_string(),
                HeldMoveSplit::of(
                    &quantities,
                    fill.side,
                    quantity,
                    fill.arrival,
                    fill.mark.unwrap_or(fill.price),
                    execution,
                )?,
            );
        }
    }
    Ok(AttributionPass {
        attribution,
        keyed,
        unmarked,
        book_total: book,
        held_moves,
    })
}

/// The pass, as the log keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AttributionRecorded {
    pub cycle: u64,
    pub at: Timestamp,
    pub fills: usize,
    pub total_pnl: Decimal,
    pub book_total: Decimal,
    pub unmarked: usize,
    pub unkeyed: usize,
    /// Component totals: market move, commission, spread, impact, …
    pub by_source: BTreeMap<String, Decimal>,
    /// Every §22 dimension's exact grouping.
    pub by_dimension: BTreeMap<String, BTreeMap<String, Decimal>>,
}

/// What every attribution record's idempotency key starts with.
pub const IDEMPOTENCY_PREFIX: &str = "attribution:";

impl EventBody for AttributionRecorded {
    const TOPIC: Topic = Topic::OutcomeObserved;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("{IDEMPOTENCY_PREFIX}{}", self.cycle))
    }
}

impl AttributionRecorded {
    /// Build the record, or `None` when the fill count has not moved since
    /// the last one — a cycle that traded nothing restates nothing.
    pub fn from_pass(
        pass: &AttributionPass,
        lineage: &mut AttributionLineage,
        fills: usize,
        cycle: u64,
        at: Timestamp,
    ) -> Result<Option<Self>> {
        if lineage.journaled_fills == Some(fills) {
            return Ok(None);
        }
        let by_dimension = pass
            .keyed
            .all()?
            .into_iter()
            .map(|(dimension, grouping)| (dimension.as_str().to_string(), grouping))
            .collect();
        lineage.journaled_fills = Some(fills);
        Ok(Some(Self {
            cycle,
            at,
            fills,
            total_pnl: pass.attribution.total_pnl,
            book_total: pass.book_total,
            unmarked: pass.unmarked,
            unkeyed: pass.keyed.unkeyed(),
            by_source: pass.attribution.by_source.clone(),
            by_dimension,
        }))
    }
}

/// The per-forecaster figure R12-MODEL-6021 asks for.
pub fn by_forecaster(pass: &AttributionPass) -> Result<BTreeMap<String, Decimal>> {
    pass.keyed.by(Dimension::Model)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic_in_result_fn)]

    use super::*;

    fn dec(text: &str) -> Decimal {
        Decimal::parse(text).expect("a decimal")
    }

    fn fill(order: &str, side: i64, qty: &str, price: &str, mark: Option<&str>) -> FillMark {
        FillMark {
            order_id: order.to_string(),
            instrument: "obj-AAA".to_string(),
            at: Timestamp::from_secs(0),
            side,
            quantity: dec(qty),
            price: dec(price),
            arrival: dec("100"),
            commission: dec("1"),
            mark: mark.map(dec),
            hypotheses: vec!["hyp-1".to_string()],
            beta: None,
        }
    }

    fn lineage_with_spread(order: &str, bps: f64) -> Result<AttributionLineage> {
        let mut lineage = AttributionLineage::new();
        lineage.note_release(
            order,
            &["hyp-1".to_string()],
            ReleaseContext {
                strategy: "desk:platform".to_string(),
                capital_policy: "survival(hedge=0)".to_string(),
                risk_rule: "kernel-test".to_string(),
                execution_path: "desk/sim/market".to_string(),
                quoted_spread_bps: Some(bps),
                quantities: None,
            },
        )?;
        Ok(lineage)
    }

    /// The defect R12-MODEL-6003's audit named: P&L was commission-only.
    /// A buy at 100.10 marked at 110 earns its move. Mutation: restoring
    /// `closing_price = fill.price` makes the total −1 (commission only).
    #[test]
    fn a_fill_marked_above_its_price_attributes_the_move_not_only_the_commission() -> Result<()> {
        let lineage = lineage_with_spread("o1", 10.0)?;
        let fills = vec![fill("o1", 1, "10", "100.10", Some("110"))];
        let pass = run(
            &Attributor::new(),
            &fills,
            &lineage,
            None,
            dec("100000"),
            Timestamp::from_secs(0),
        )?;
        // 10 × (110 − 100.10) − 1 = 98.
        assert_eq!(pass.attribution.total_pnl, dec("98"));
        assert_eq!(pass.book_total, dec("98"));
        let position = &pass.attribution.positions[0];
        // Half of 10 bp on 10 × 100 = 0.5 of spread; the rest of the 1.00
        // slippage is impact.
        assert_eq!(
            position.component(qip_learning_engine::Source::Spread),
            dec("-0.5")
        );
        assert_eq!(
            position.component(qip_learning_engine::Source::MarketImpact),
            dec("-0.5")
        );
        assert_eq!(
            position.component(qip_learning_engine::Source::Commission),
            dec("-1")
        );
        Ok(())
    }

    /// A round trip realises exactly the price difference. Mutation:
    /// dropping the sign on a sell turns the round trip into a doubled
    /// position and the realised figure into a mark-to-market one.
    #[test]
    fn a_round_trip_attributes_exactly_its_realised_pnl_whatever_the_mark() -> Result<()> {
        let lineage = AttributionLineage::new();
        let fills = vec![
            fill("buy", 1, "5", "100", Some("130")),
            fill("sell", -1, "5", "120", Some("130")),
        ];
        let pass = run(
            &Attributor::new(),
            &fills,
            &lineage,
            None,
            dec("100000"),
            Timestamp::from_secs(0),
        )?;
        // (120 − 100) × 5 − 2 commissions = 98, independent of the mark.
        assert_eq!(pass.attribution.total_pnl, dec("98"));
        assert_eq!(pass.keyed.unkeyed(), 2, "neither order was stamped");
        Ok(())
    }

    #[test]
    fn an_unmarked_fill_is_counted_rather_than_claimed_flat() -> Result<()> {
        let lineage = AttributionLineage::new();
        let fills = vec![fill("o", 1, "1", "100", None)];
        let pass = run(
            &Attributor::new(),
            &fills,
            &lineage,
            None,
            dec("1000"),
            Timestamp::from_secs(0),
        )?;
        assert_eq!(pass.unmarked, 1);
        Ok(())
    }
}
