//! Pricing a candidate against the book it would actually trade on.
//!
//! This is the stage that decides whether an arbitrage exists. Everything
//! before it worked from a rate — one number per conversion, quoted at no size,
//! which is what makes a path look profitable in a scan and lose money in
//! production. Here every leg is walked through real depth at the real size,
//! and the answer is the quantity that comes back at the end of the cycle
//! rather than a multiplier anyone quoted.
//!
//! Two habits keep it honest:
//!
//! * **The touch and the sweep are recorded separately.** Crossing to the best
//!   price is the spread; walking past it is the slippage. They are different
//!   costs and a path can be killed by either, so collapsing them into one
//!   number would hide which one killed it.
//! * **Running out of depth is a fact, not a rounding.** A leg the book cannot
//!   fill is reported as short. It is never extrapolated to the size that was
//!   asked for, because the extrapolated price is exactly the price that does
//!   not exist.
//!
//! What comes out is gross of every conversion cost. Fees are not netted off
//! here even though each edge knows its own: they are charged once, by name, in
//! [`crate::netedge`], and a cost taken off in two places is a cost nobody can
//! find. The rate the search worked from does include them — a search that
//! chased cycles which only pay before fees would waste every stage after it —
//! which is why [`crate::graph::ConversionEdge::effective_rate`] and the
//! quantities here deliberately differ.

use crate::arith::{div, mul};
use crate::graph::{ArbitrageGraph, EdgeKind, Node, PathKind, SyntheticComponent};
use crate::liquidity::LiquiditySource;
use crate::search::PathCandidate;
use qip_contracts::message::BookSide;
use qip_contracts::venue::{VenueClass, VenueId};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, ObjectId, Timestamp};
use serde::{Deserialize, Serialize};

/// One order the path would have to send.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PathLeg {
    /// Which conversion in the cycle this leg realises.
    pub conversion: usize,
    /// The graph edge the conversion came from.
    pub edge: usize,
    pub object: ObjectId,
    pub venue: VenueId,
    pub venue_class: VenueClass,
    /// The side of the book consumed.
    pub side: BookSide,
    /// The instrument this leg's price and notional are denominated in.
    ///
    /// Recorded because a cycle can quote its legs in more than one currency,
    /// and a residual exposure summed blindly across them is a number with no
    /// units.
    pub quote_object: ObjectId,
    /// What would actually be traded, after depth and after what the path can
    /// afford to pay for it.
    pub quantity: Decimal,
    /// What the path asked for.
    pub requested_quantity: Decimal,
    /// What the book could have supplied at any price.
    pub available_quantity: Decimal,
    /// Volume-weighted price from walking the book at `requested_quantity`.
    pub executable_price: Decimal,
    /// Best price on this side.
    pub touch_price: Decimal,
    /// Size resting at the touch — how much of this leg is reversible cheaply.
    pub touch_quantity: Decimal,
    pub mid_price: Decimal,
}

impl PathLeg {
    /// Traded value, in the quote currency of this leg's book.
    pub fn notional(&self) -> Result<Decimal> {
        mul(self.quantity, self.executable_price, "leg notional")
    }

    /// Whether the book held everything the path asked for.
    pub fn has_depth(&self) -> bool {
        self.available_quantity >= self.requested_quantity
    }

    /// Cost of crossing from the mid to the touch, as a fraction of notional.
    ///
    /// Side-aware and floored at zero. A touch on the wrong side of the mid is
    /// a crossed or stale book, not a rebate for trading, and letting it come
    /// through as a negative cost would make a broken feed look profitable.
    pub fn spread_fraction(&self) -> Result<Decimal> {
        let adverse = match self.side {
            BookSide::Bid => self.mid_price - self.touch_price,
            BookSide::Ask => self.touch_price - self.mid_price,
        };
        if self.mid_price <= Decimal::ZERO {
            return Err(Error::invalid(format!(
                "{} has no usable mid price",
                self.object.as_str()
            )));
        }
        div(
            adverse.max(Decimal::ZERO),
            self.mid_price,
            "spread fraction",
        )
    }

    /// Cost of walking past the touch to fill the size, as a fraction of
    /// notional. Measured from the touch, so it does not double-count the
    /// spread.
    pub fn slippage_fraction(&self) -> Result<Decimal> {
        let adverse = match self.side {
            BookSide::Bid => self.touch_price - self.executable_price,
            BookSide::Ask => self.executable_price - self.touch_price,
        };
        if self.touch_price <= Decimal::ZERO {
            return Err(Error::invalid(format!(
                "{} has no usable touch price",
                self.object.as_str()
            )));
        }
        div(
            adverse.max(Decimal::ZERO),
            self.touch_price,
            "slippage fraction",
        )
    }
}

/// One conversion of the cycle, priced.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PricedConversion {
    pub edge: usize,
    pub kind: String,
    pub from: Node,
    pub to: Node,
    /// Units of `from.object` going in.
    pub input: Decimal,
    /// Units of `to.object` coming out, priced off the book and gross of the
    /// conversion's own cost.
    pub output: Decimal,
    /// What the quoted rate said would come out, on the same gross basis. The
    /// gap between this and `output` is what mid pricing would have missed.
    pub indicative_output: Decimal,
    pub cost_fraction: Decimal,
    pub legs: Vec<PathLeg>,
    /// Whether every leg found the depth it asked for.
    pub fully_available: bool,
    /// Whether this conversion either completes or does not happen.
    ///
    /// False for anything settled on a public chain, where one leg can land and
    /// its counterpart revert. The leg planner sizes prefunded inventory
    /// against exactly this.
    pub settles_atomically: bool,
}

impl PricedConversion {
    /// Total traded value across this conversion's legs.
    pub fn notional(&self) -> Result<Decimal> {
        let mut total = Decimal::ZERO;
        for leg in &self.legs {
            total += leg.notional()?;
        }
        Ok(total)
    }
}

/// A candidate priced end to end against the book.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PathPricing {
    pub kind: PathKind,
    pub start: Node,
    pub start_quantity: Decimal,
    /// What comes back after every leg has been walked through real depth.
    pub end_quantity: Decimal,
    /// What would have come back at the quoted rates. Kept so a rejection can
    /// say *how much* of the apparent edge was mid pricing.
    pub indicative_end_quantity: Decimal,
    pub conversions: Vec<PricedConversion>,
    /// The oldest input any part of this pricing rests on.
    pub oldest_input_at: Timestamp,
    /// The thinnest evidence any part of this pricing rests on.
    pub fewest_observations: u32,
    /// Whether every conversion settles atomically.
    pub all_atomic: bool,
}

impl PathPricing {
    /// What the path makes, in units of the instrument it started from.
    ///
    /// Negative is the common and useful answer.
    pub fn gross_edge(&self) -> Decimal {
        self.end_quantity - self.start_quantity
    }

    /// What the path would have made if the quoted rates had been fillable.
    pub fn indicative_gross_edge(&self) -> Decimal {
        self.indicative_end_quantity - self.start_quantity
    }

    /// Whether every leg found the depth it asked for.
    ///
    /// A path that did not is not a smaller opportunity, it is a different one:
    /// the legs no longer net out, and what is left is a position.
    pub fn is_fully_available(&self) -> bool {
        self.conversions.iter().all(|c| c.fully_available)
    }

    /// Every order the plan would have to send, in cycle order.
    pub fn legs(&self) -> impl Iterator<Item = &PathLeg> {
        self.conversions.iter().flat_map(|c| c.legs.iter())
    }

    /// Whether the path pays once the book has had its say.
    pub fn is_profitable_on_book(&self) -> bool {
        self.end_quantity > self.start_quantity
    }
}

/// Walk a candidate through the book at a stated size.
///
/// The size is in units of the instrument the cycle starts from; edge is not
/// linear in size and a path priced without one is not a number anyone can act
/// on, which is why there is no defaulted overload.
pub fn price_path(
    graph: &ArbitrageGraph,
    source: &dyn LiquiditySource,
    candidate: &PathCandidate,
    start_quantity: Decimal,
) -> Result<PathPricing> {
    if candidate.edges.is_empty() {
        return Err(Error::invalid("an empty cycle cannot be priced"));
    }
    if start_quantity <= Decimal::ZERO {
        return Err(Error::invalid("a path must be priced at a positive size"));
    }

    let first = graph.edge(candidate.edges[0]).ok_or_else(|| {
        Error::not_found("the candidate names a conversion that is not in the graph")
    })?;
    let start = first.from.clone();

    let mut carried = start_quantity;
    let mut indicative_carried = start_quantity;
    let mut conversions: Vec<PricedConversion> = Vec::new();
    let mut oldest = Timestamp::MAX;
    let mut fewest = u32::MAX;

    for (position, edge_index) in candidate.edges.iter().enumerate() {
        let edge = graph
            .edge(*edge_index)
            .ok_or_else(|| Error::not_found(format!("no conversion at index {edge_index}")))?;
        if position > 0 && conversions.last().is_some_and(|c| c.to != edge.from) {
            return Err(Error::invalid(format!(
                "the candidate is not a path: {} does not continue from the previous conversion",
                edge.label()
            )));
        }
        if carried <= Decimal::ZERO {
            return Err(Error::invalid(format!(
                "conversion {} has nothing left to convert",
                edge.label()
            )));
        }

        oldest = oldest.min(edge.observed_at);
        fewest = fewest.min(edge.observations);

        let mut conversion = match &edge.kind {
            EdgeKind::Transfer => price_transfer(graph, edge, carried, *edge_index)?,
            EdgeKind::Trade { market, side } => price_trade(
                graph,
                source,
                edge,
                market,
                *side,
                carried,
                position,
                *edge_index,
            )?,
            EdgeKind::Synthetic {
                synthetic_object,
                components,
            } => price_synthetic(
                graph,
                source,
                edge,
                synthetic_object,
                components,
                carried,
                position,
                *edge_index,
            )?,
        };

        for leg in &conversion.legs {
            if let Some(at) = source.as_of(&leg.venue, &leg.object) {
                oldest = oldest.min(at);
            }
            fewest = fewest.min(source.observations(&leg.venue, &leg.object));
        }

        conversion.indicative_output = mul(
            indicative_carried,
            edge.indicative_rate,
            "indicative output",
        )?;
        indicative_carried = conversion.indicative_output;
        carried = conversion.output;
        conversions.push(conversion);
    }

    let closes = conversions.last().is_some_and(|last| last.to == start);
    if !closes {
        return Err(Error::invalid(
            "the candidate does not return to the instrument it started from",
        ));
    }

    let all_atomic = conversions.iter().all(|c| c.settles_atomically);
    Ok(PathPricing {
        kind: candidate.kind,
        start,
        start_quantity,
        end_quantity: carried,
        indicative_end_quantity: indicative_carried,
        conversions,
        oldest_input_at: if oldest == Timestamp::MAX {
            Timestamp::EPOCH
        } else {
            oldest
        },
        fewest_observations: if fewest == u32::MAX { 0 } else { fewest },
        all_atomic,
    })
}

fn venue_class(graph: &ArbitrageGraph, venue: &VenueId) -> Result<VenueClass> {
    graph
        .venue_facts(venue)
        .map(|facts| facts.class)
        .ok_or_else(|| {
            Error::not_found(format!(
                "venue {} has no recorded class, so its settlement assumptions are unknown",
                venue.as_str()
            ))
        })
}

fn price_transfer(
    graph: &ArbitrageGraph,
    edge: &crate::graph::ConversionEdge,
    input: Decimal,
    edge_index: usize,
) -> Result<PricedConversion> {
    let from_class = venue_class(graph, &edge.from.venue)?;
    let to_class = venue_class(graph, &edge.to.venue)?;
    // Gross of the transfer's own cost, which the fee deduction charges once.
    let output = input;
    Ok(PricedConversion {
        edge: edge_index,
        kind: edge.kind.as_str().to_string(),
        from: edge.from.clone(),
        to: edge.to.clone(),
        input,
        output,
        indicative_output: output,
        cost_fraction: edge.cost_fraction,
        legs: Vec::new(),
        // A transfer has no counterparty to disappoint: whatever arrives,
        // arrives as the same instrument.
        fully_available: true,
        settles_atomically: from_class.settles_atomically() && to_class.settles_atomically(),
    })
}

#[allow(clippy::too_many_arguments)]
fn price_trade(
    graph: &ArbitrageGraph,
    source: &dyn LiquiditySource,
    edge: &crate::graph::ConversionEdge,
    market: &ObjectId,
    side: BookSide,
    input: Decimal,
    conversion: usize,
    edge_index: usize,
) -> Result<PricedConversion> {
    let venue = &edge.from.venue;
    let class = venue_class(graph, venue)?;
    // Consuming bids sells the base and receives the other end of the edge;
    // consuming offers spends it. Either way the price is quoted in whichever
    // instrument is not being counted in base units.
    let quote_object = match side {
        BookSide::Bid => edge.to.object.clone(),
        BookSide::Ask => edge.from.object.clone(),
    };

    // Selling asks for the base directly; buying has to guess how much base the
    // cash will reach before it can ask, and the quoted rate is the only guess
    // available. What it actually reaches is settled below, against the sweep.
    let requested = match side {
        BookSide::Bid => input,
        BookSide::Ask => mul(input, edge.indicative_rate, "requested base quantity")?,
    };

    let (executable_price, available) = source
        .sweep_cost(venue, market, side, requested)
        .ok_or_else(|| {
            Error::unavailable(format!(
                "no depth for {} at {} on the {} side",
                market.as_str(),
                venue.as_str(),
                side.as_str()
            ))
        })?;
    let (touch_price, touch_quantity) = source.touch(venue, market, side).ok_or_else(|| {
        Error::unavailable(format!(
            "no touch for {} at {}",
            market.as_str(),
            venue.as_str()
        ))
    })?;
    let mid_price = source.mid(venue, market).ok_or_else(|| {
        Error::unavailable(format!(
            "no two-sided market for {} at {}, so its spread cannot be measured",
            market.as_str(),
            venue.as_str()
        ))
    })?;

    let (quantity, output) = match side {
        BookSide::Bid => {
            let filled = available.min(requested);
            let proceeds = mul(filled, executable_price, "sale proceeds")?;
            (filled, proceeds)
        }
        BookSide::Ask => {
            // Cash is finite: a sweep that came back more expensive than the
            // quote buys less, not the same amount on credit.
            let affordable = div(input, executable_price, "affordable quantity")?;
            let acquired = available.min(requested).min(affordable);
            (acquired, acquired)
        }
    };

    let leg = PathLeg {
        conversion,
        edge: edge_index,
        object: market.clone(),
        venue: venue.clone(),
        venue_class: class,
        side,
        quote_object,
        quantity,
        requested_quantity: requested,
        available_quantity: available,
        executable_price,
        touch_price,
        touch_quantity,
        mid_price,
    };
    let fully_available = leg.has_depth();

    Ok(PricedConversion {
        edge: edge_index,
        kind: edge.kind.as_str().to_string(),
        from: edge.from.clone(),
        to: edge.to.clone(),
        input,
        output,
        indicative_output: Decimal::ZERO,
        cost_fraction: edge.cost_fraction,
        legs: vec![leg],
        fully_available,
        settles_atomically: class.settles_atomically(),
    })
}

/// Price a synthetic in whichever direction the edge runs.
///
/// Assembling and unwinding are the same walk with the book sides flipped, so
/// they share one implementation: two would drift, and the direction that drifts
/// is always the one nobody tested.
#[allow(clippy::too_many_arguments)]
fn price_synthetic(
    graph: &ArbitrageGraph,
    source: &dyn LiquiditySource,
    edge: &crate::graph::ConversionEdge,
    synthetic_object: &ObjectId,
    components: &[SyntheticComponent],
    input: Decimal,
    conversion: usize,
    edge_index: usize,
) -> Result<PricedConversion> {
    let assembling = &edge.to.object == synthetic_object;
    // The end of the edge that is not the synthetic is what pays for it, and is
    // therefore what every component leg is priced in.
    let cash_object = if assembling {
        edge.from.object.clone()
    } else {
        edge.to.object.clone()
    };

    // Unwinding starts from a holding of the synthetic; assembling starts from
    // cash and can only reach as many units as the quoted rate suggests.
    let target_units = if assembling {
        mul(input, edge.indicative_rate, "synthetic units sought")?
    } else {
        input
    };
    if target_units <= Decimal::ZERO {
        return Err(Error::invalid(format!(
            "synthetic {} was asked for a non-positive number of units",
            edge.label()
        )));
    }

    struct Quoted {
        component: SyntheticComponent,
        side: BookSide,
        class: VenueClass,
        requested: Decimal,
        available: Decimal,
        executable_price: Decimal,
        touch_price: Decimal,
        touch_quantity: Decimal,
        mid_price: Decimal,
    }

    let mut quotes: Vec<Quoted> = Vec::with_capacity(components.len());
    let mut reachable_units = target_units;
    for component in components {
        let side = if assembling {
            component.unwind_side.opposite()
        } else {
            component.unwind_side
        };
        let class = venue_class(graph, &component.venue)?;
        let requested = mul(target_units, component.units_per_unit, "component quantity")?;
        let (executable_price, available) = source
            .sweep_cost(&component.venue, &component.object, side, requested)
            .ok_or_else(|| {
                Error::unavailable(format!(
                    "no depth for component {} at {}",
                    component.object.as_str(),
                    component.venue.as_str()
                ))
            })?;
        let (touch_price, touch_quantity) = source
            .touch(&component.venue, &component.object, side)
            .ok_or_else(|| {
                Error::unavailable(format!(
                    "no touch for component {}",
                    component.object.as_str()
                ))
            })?;
        let mid_price = source
            .mid(&component.venue, &component.object)
            .ok_or_else(|| {
                Error::unavailable(format!(
                    "no two-sided market for component {}",
                    component.object.as_str()
                ))
            })?;

        // The synthetic is only as deep as its thinnest constituent. A basket
        // priced on the depth of its liquid names is the standard way to
        // discover, mid-trade, that one leg cannot be done at all.
        let units_here = div(
            available.min(requested),
            component.units_per_unit,
            "component depth in synthetic units",
        )?;
        reachable_units = reachable_units.min(units_here);

        quotes.push(Quoted {
            component: component.clone(),
            side,
            class,
            requested,
            available,
            executable_price,
            touch_price,
            touch_quantity,
            mid_price,
        });
    }

    // Cash flow at the depth-limited size: selling a constituent brings cash
    // in, buying one takes it out.
    let mut cash_flow = Decimal::ZERO;
    for quote in &quotes {
        let quantity = mul(
            reachable_units,
            quote.component.units_per_unit,
            "component quantity",
        )?;
        let value = mul(quantity, quote.executable_price, "component value")?;
        cash_flow += match quote.side {
            BookSide::Bid => value,
            BookSide::Ask => -value,
        };
    }

    let achieved_units = if assembling {
        let outlay = -cash_flow;
        if outlay <= Decimal::ZERO {
            return Err(Error::numeric(format!(
                "assembling {} costs nothing, which means its components are mispriced",
                synthetic_object.as_str()
            )));
        }
        if outlay > input {
            // Cash ran out before depth did. Scaling back keeps the volume
            // weighted prices from the larger sweep, which overstates the cost
            // of the smaller one — the error is in the cautious direction.
            mul(
                reachable_units,
                div(input, outlay, "affordable fraction")?,
                "affordable units",
            )?
        } else {
            reachable_units
        }
    } else {
        reachable_units
    };

    let mut legs = Vec::with_capacity(quotes.len());
    let mut realised_flow = Decimal::ZERO;
    for quote in &quotes {
        let quantity = mul(
            achieved_units,
            quote.component.units_per_unit,
            "component quantity",
        )?;
        let value = mul(quantity, quote.executable_price, "component value")?;
        realised_flow += match quote.side {
            BookSide::Bid => value,
            BookSide::Ask => -value,
        };
        legs.push(PathLeg {
            conversion,
            edge: edge_index,
            object: quote.component.object.clone(),
            venue: quote.component.venue.clone(),
            venue_class: quote.class,
            side: quote.side,
            quote_object: cash_object.clone(),
            quantity,
            requested_quantity: quote.requested,
            available_quantity: quote.available,
            executable_price: quote.executable_price,
            touch_price: quote.touch_price,
            touch_quantity: quote.touch_quantity,
            mid_price: quote.mid_price,
        });
    }

    let output = if assembling {
        achieved_units
    } else {
        if realised_flow <= Decimal::ZERO {
            return Err(Error::numeric(format!(
                "unwinding {} raises nothing, which means its components are mispriced",
                synthetic_object.as_str()
            )));
        }
        realised_flow
    };

    let fully_available = legs.iter().all(PathLeg::has_depth);
    let settles_atomically = legs.iter().all(|leg| leg.venue_class.settles_atomically());

    Ok(PricedConversion {
        edge: edge_index,
        kind: edge.kind.as_str().to_string(),
        from: edge.from.clone(),
        to: edge.to.clone(),
        input,
        output,
        indicative_output: Decimal::ZERO,
        cost_fraction: edge.cost_fraction,
        legs,
        fully_available,
        settles_atomically,
    })
}
