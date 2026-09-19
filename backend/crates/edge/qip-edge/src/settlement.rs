//! Settlement terms per venue, and when a leg's proceeds are usable (§56.2
//! rule 21, §32.2).
//!
//! Blueprint §32.2 puts one sentence at the centre of this: "Reservation is
//! future-tense: availability is checked at the moment each leg needs it."
//! The cell's region ledger ([`crate::reservation`]) holds a cycle gross —
//! every leg's notional out of settled capital, taken before an `Intent`
//! exists — so no leg is funded from another leg's proceeds *in the ledger*.
//! But a cycle is a chain: leg two spends what leg one bought, at the venue
//! leg one bought it. Whether that asset is usable when leg two fires is not
//! a question the ledger can answer, because the ledger counts money and the
//! question is about time. §32.2's stage table answers it: proceeds are
//! **in-settlement** until the venue's calendar says otherwise, and
//! in-settlement capital is usable as funding "only via a bridge, at a priced
//! cost". This cell has no bridge — no margin line, no broker credit, no
//! cross-margin — so the only row of the bridge table it can take is the
//! last: wait for settlement. A cycle cannot wait. It is vetoed whole.
//!
//! What this module holds is the fact that decides that veto: for each venue,
//! how long after a fill the proceeds are usable. [`SettlementTerms::instant`]
//! is a venue that credits proceeds when it reports the fill — a paper
//! simulator, or a crypto exchange booking spot internally. A venue on a
//! settlement cycle carries the convention (T+0, T+1, T+2), a cut-off minute,
//! an availability minute and the days that settle, and projects the instant
//! proceeds land by walking settlement days rather than calendar days, so a
//! Friday T+1 lands on Monday and a Thursday T+2 lands on Monday. That walk
//! is the fact `qip-capital-fabric`'s `SettlementCalendar` computes for the
//! centre; the discipline is reproduced here rather than reused for the
//! reason `reservation.rs` gives — a cell that reached a capital service
//! could widen its own bound, and the acceptance suite refuses the edge.
//!
//! # What the cell does not know, stated
//!
//! A venue the cell holds no terms for is **not projected**. It is not read
//! as instant — that is the permissive guess — and not as T+2 — that is a
//! number nobody computed. The feasibility gate takes the same position on a
//! venue with no lot size (`feasibility.rs`, "what the gate knows, and from
//! where"), and for the same reason: the fact is the composition root's to
//! supply, and inventing it would be a rounding rule wearing a refusal's
//! clothes. The count of configured venues without terms is published on a
//! gauge every pass so that the silence is a number on a chart rather than a
//! gate that appears to be passing.

use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The gate literal a cycle is refused under when a leg would be funded by
/// proceeds that are still in settlement when it fires. A `pub const` so the
/// `qip_edge_refusals_total{gate}` label stays bounded by source rather than
/// by a runtime string.
pub const GATE_SETTLEMENT: &str = "settlement_reservation";

/// How many settlement days after the value date proceeds are usable.
///
/// The same three arms as the centre's convention, spelled the same, so a
/// value the centre ships per venue can be read here without translation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettlementConvention {
    /// The same settlement day, if the fill makes the cut-off.
    T0,
    /// The next settlement day.
    T1,
    /// Two settlement days on.
    T2,
}

impl SettlementConvention {
    pub const fn days(self) -> u32 {
        match self {
            Self::T0 => 0,
            Self::T1 => 1,
            Self::T2 => 2,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::T0 => "T+0",
            Self::T1 => "T+1",
            Self::T2 => "T+2",
        }
    }

    /// Parse the spelling [`Self::as_str`] produces. Refuses anything else
    /// rather than defaulting: a convention typed wrong is a venue whose
    /// proceeds land on a day nobody chose.
    pub fn parse(text: &str) -> Result<Self> {
        match text.trim() {
            "T+0" | "t+0" | "T0" | "t0" => Ok(Self::T0),
            "T+1" | "t+1" | "T1" | "t1" => Ok(Self::T1),
            "T+2" | "t+2" | "T2" | "t2" => Ok(Self::T2),
            other => Err(Error::invalid(format!(
                "settlement convention {other:?} is not one of T+0, T+1, T+2 or instant"
            ))),
        }
    }
}

/// When one venue's proceeds become usable as funding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettlementTerms {
    /// `None` is a venue that credits proceeds when it reports the fill.
    cycle: Option<SettlementCycle>,
}

/// A venue that settles on a calendar rather than on the fill.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct SettlementCycle {
    convention: SettlementConvention,
    /// A fill at or after this UTC minute of the day is dated to the next
    /// settlement day.
    cutoff_minute: u32,
    /// Proceeds are usable from this UTC minute of the settlement day.
    availability_minute: u32,
    /// Weekdays that settle, Monday = 0. Saturday and Sunday are absent from
    /// the weekday constructor, which is the one fact this module exists to
    /// stop a cycle forgetting.
    settlement_weekdays: BTreeSet<u32>,
    /// Non-settlement days, as day numbers since the epoch.
    holidays: BTreeSet<i64>,
}

/// Minutes in a day; a cut-off or an availability at or past it names no
/// instant.
const MINUTES_PER_DAY: u32 = 1440;

/// The furthest ahead a projection walks before refusing. Bounded rather
/// than a loop that trusts its input: terms with every weekday a holiday
/// would otherwise spin on the order path.
const MAXIMUM_ROLL_DAYS: u32 = 400;

impl SettlementTerms {
    /// A venue whose proceeds are usable the instant it reports the fill.
    pub const fn instant() -> Self {
        Self { cycle: None }
    }

    /// A venue settling Monday to Friday on the given convention, with a
    /// 16:00 UTC cut-off and proceeds usable at 09:00 UTC on the settlement
    /// day — 17:00 for T+0, which must hand over after its own cut-off.
    ///
    /// The same figures the centre's weekday calendar uses, on purpose: a
    /// cell and the centre projecting one fill to two different days would
    /// be two claims about one fact.
    pub fn weekday(convention: SettlementConvention) -> Result<Self> {
        let availability = match convention {
            SettlementConvention::T0 => 17 * 60,
            _ => 9 * 60,
        };
        Self::on_cycle(convention, 16 * 60, availability, [0, 1, 2, 3, 4])
    }

    /// A venue settling on a calendar.
    ///
    /// Refused when either minute names no instant in a day, when no weekday
    /// settles at all — terms under which no proceeds ever land are not
    /// terms, they are a venue that should not be configured — and when a
    /// T+0 venue would hand proceeds over before its own cut-off, which
    /// models settlement before instruction.
    pub fn on_cycle(
        convention: SettlementConvention,
        cutoff_minute: u32,
        availability_minute: u32,
        settlement_weekdays: impl IntoIterator<Item = u32>,
    ) -> Result<Self> {
        if cutoff_minute >= MINUTES_PER_DAY || availability_minute >= MINUTES_PER_DAY {
            return Err(Error::invalid(format!(
                "a settlement cut-off ({cutoff_minute}) and availability ({availability_minute}) \
                 are minutes within a day, so both must be below {MINUTES_PER_DAY}"
            )));
        }
        if convention == SettlementConvention::T0 && availability_minute < cutoff_minute {
            return Err(Error::invalid(format!(
                "T+0 terms cannot make proceeds usable at minute {availability_minute} against a \
                 cut-off at minute {cutoff_minute}; that is settlement before the fill"
            )));
        }
        let settlement_weekdays: BTreeSet<u32> = settlement_weekdays.into_iter().collect();
        if settlement_weekdays.is_empty() {
            return Err(Error::invalid(
                "settlement terms name no weekday that settles, so no proceeds would ever be \
                 usable; name the settling weekdays or configure the venue as instant",
            ));
        }
        if let Some(day) = settlement_weekdays.iter().find(|day| **day > 6) {
            return Err(Error::invalid(format!(
                "settlement weekday {day} is not a weekday; Monday is 0 and Sunday is 6"
            )));
        }
        Ok(Self {
            cycle: Some(SettlementCycle {
                convention,
                cutoff_minute,
                availability_minute,
                settlement_weekdays,
                holidays: BTreeSet::new(),
            }),
        })
    }

    /// Parse the composition root's spelling: `instant`, or a convention as
    /// [`SettlementConvention::as_str`] writes it, on the weekday calendar.
    pub fn parse(text: &str) -> Result<Self> {
        match text.trim() {
            "instant" | "INSTANT" | "Instant" => Ok(Self::instant()),
            other => Self::weekday(SettlementConvention::parse(other)?),
        }
    }

    /// Add a non-settlement day. A no-op for instant terms, because a venue
    /// that credits on the fill has no settlement day to skip.
    #[must_use]
    pub fn with_holiday(mut self, date: Timestamp) -> Self {
        if let Some(cycle) = self.cycle.as_mut() {
            cycle.holidays.insert(day_number(date));
        }
        self
    }

    pub const fn is_instant(&self) -> bool {
        self.cycle.is_none()
    }

    /// The convention, or `None` for instant terms.
    pub fn convention(&self) -> Option<SettlementConvention> {
        self.cycle.as_ref().map(|cycle| cycle.convention)
    }

    /// A stable label for refusal messages: `instant` or the convention.
    pub fn describe(&self) -> &'static str {
        match self.cycle.as_ref() {
            None => "instant",
            Some(cycle) => cycle.convention.as_str(),
        }
    }

    /// The instant proceeds of a fill at `filled_at` are usable as funding.
    ///
    /// Instant terms return `filled_at` itself. Terms on a cycle date the
    /// fill to today if today settles and the cut-off has not passed, else
    /// to the next settlement day, then advance the convention's number of
    /// *settlement* days, then apply the availability minute. Monotone in
    /// `filled_at`: a later fill never lands earlier, which is what lets a
    /// caller that does not know exactly when a leg will fill project from
    /// the earliest instant it could.
    pub fn available_at(&self, filled_at: Timestamp) -> Result<Timestamp> {
        let Some(cycle) = self.cycle.as_ref() else {
            return Ok(filled_at);
        };
        let today = filled_at.start_of_day();
        let (hour, minute, _, _) = filled_at.civil_time();
        let inside_cutoff = hour * 60 + minute < cycle.cutoff_minute;
        let value_date = if cycle.settles_on(today) && inside_cutoff {
            today
        } else {
            cycle.settlement_day_at_or_after(today.saturating_add(Duration::from_days(1)))?
        };
        let mut settlement_date = value_date;
        for _ in 0..cycle.convention.days() {
            settlement_date = cycle.settlement_day_at_or_after(
                settlement_date.saturating_add(Duration::from_days(1)),
            )?;
        }
        Ok(settlement_date.saturating_add(Duration::from_secs(
            i64::from(cycle.availability_minute) * 60,
        )))
    }
}

impl SettlementCycle {
    fn settles_on(&self, day_start: Timestamp) -> bool {
        self.settlement_weekdays.contains(&day_start.weekday())
            && !self.holidays.contains(&day_number(day_start))
    }

    fn settlement_day_at_or_after(&self, from: Timestamp) -> Result<Timestamp> {
        let mut cursor = from.start_of_day();
        for _ in 0..MAXIMUM_ROLL_DAYS {
            if self.settles_on(cursor) {
                return Ok(cursor);
            }
            cursor = cursor.saturating_add(Duration::from_days(1));
        }
        Err(Error::unavailable(format!(
            "no settlement day within {MAXIMUM_ROLL_DAYS} days of {}; the venue's terms have \
             no settlement days left in them",
            from.to_date_string()
        )))
    }
}

fn day_number(at: Timestamp) -> i64 {
    at.as_nanos().div_euclid(qip_core::time::NANOS_PER_DAY)
}

/// How one leg of a cycle is funded, projected before the cycle is held.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LegFunding {
    /// Settled capital or prefunded inventory: the first leg, or a leg at a
    /// different venue from the one before it. `qip-arbitrage`'s planner
    /// never emits a transfer as a leg — it realises one as inventory at the
    /// far venue — so a venue change between two legs is exactly the case
    /// where the second spends something the cell already holds there.
    Held,
    /// What the previous leg delivers at this same venue, usable from
    /// `usable_at` under the venue's terms.
    Proceeds { of_leg: usize, usable_at: Timestamp },
    /// What the previous leg delivers at a venue the cell holds no terms
    /// for. Not judged, and counted rather than guessed at; see the module
    /// doc.
    Unprojected { of_leg: usize },
}

/// A cycle's legs against the settlement timeline, in firing order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Projection {
    /// The instant every leg is taken to fire and every fill to be dated.
    pub fires_at: Timestamp,
    pub legs: Vec<LegFunding>,
}

/// One leg whose funding is not there when it fires.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unfunded {
    /// Zero-based position of the leg that cannot be funded.
    pub leg: usize,
    /// The leg whose proceeds it would spend.
    pub of_leg: usize,
    /// When those proceeds become usable.
    pub usable_at: Timestamp,
}

impl Projection {
    /// The first leg, in firing order, that would spend proceeds still in
    /// settlement at the instant it fires.
    pub fn first_unfunded(&self) -> Option<Unfunded> {
        self.legs
            .iter()
            .enumerate()
            .find_map(|(leg, funding)| match funding {
                LegFunding::Proceeds { of_leg, usable_at } if *usable_at > self.fires_at => {
                    Some(Unfunded {
                        leg,
                        of_leg: *of_leg,
                        usable_at: *usable_at,
                    })
                }
                _ => None,
            })
    }

    /// How many legs were funded by proceeds at a venue with no terms.
    pub fn unprojected(&self) -> usize {
        self.legs
            .iter()
            .filter(|funding| matches!(funding, LegFunding::Unprojected { .. }))
            .count()
    }
}

/// Project every leg of a cycle against its venues' terms.
///
/// `venues` is the legs' venues in firing order and `fires_at` the earliest
/// instant any of them fires. One instant for every leg, deliberately: the
/// cell has no execution shape in which a leg fires *later* than the leg it
/// depends on by a settlement day — all-at-once sends every leg now, and
/// §32.1's passive-first rests one and crosses the others at its fill, so
/// the dependent legs and the fill they depend on move together. Since
/// [`SettlementTerms::available_at`] is monotone, projecting from the
/// earliest instant is the conservative reading and never admits a cycle a
/// later fire would refuse.
///
/// `Err` is a calendar that could not be walked, which the caller refuses
/// under the same gate: a projection the cell could not make is a figure it
/// could not evaluate, and the edge's rule for those is to refuse rather
/// than abstain.
pub fn project(
    venues: &[VenueId],
    terms: &BTreeMap<String, SettlementTerms>,
    fires_at: Timestamp,
) -> Result<Projection> {
    let mut legs = Vec::with_capacity(venues.len());
    for (position, venue) in venues.iter().enumerate() {
        let funding = match position.checked_sub(1) {
            None => LegFunding::Held,
            Some(previous) if venues[previous] != *venue => LegFunding::Held,
            Some(previous) => match terms.get(venue.as_str()) {
                None => LegFunding::Unprojected { of_leg: previous },
                Some(terms) => LegFunding::Proceeds {
                    of_leg: previous,
                    usable_at: terms.available_at(fires_at)?,
                },
            },
        };
        legs.push(funding);
    }
    Ok(Projection { fires_at, legs })
}

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;

    /// 2025-10-09T08:53:20Z, a Thursday — the instant every edge fixture
    /// counts from, so a date asserted here is a date the crate tests share.
    fn thursday(secs: i64) -> Timestamp {
        Timestamp::from_secs(1_760_000_000 + secs)
    }

    fn friday_1500() -> Timestamp {
        Timestamp::from_secs(1_760_108_400)
    }

    fn friday_1630() -> Timestamp {
        Timestamp::from_secs(1_760_113_800)
    }

    fn v(name: &str) -> VenueId {
        VenueId::new(name)
    }

    #[test]
    fn instant_terms_make_proceeds_usable_at_the_fill_itself() -> Result<()> {
        let at = thursday(0);
        assert_eq!(SettlementTerms::instant().available_at(at)?, at);
        Ok(())
    }

    #[test]
    fn a_friday_t_plus_one_lands_on_monday_not_saturday() -> Result<()> {
        // The one fact this module exists for. A calendar-day count would
        // say Saturday 09:00; settlement days say Monday.
        let terms = SettlementTerms::weekday(SettlementConvention::T1)?;
        let usable = terms.available_at(friday_1500())?;
        assert_eq!(usable.to_rfc3339(), "2025-10-13T09:00:00.000Z");
        assert_eq!(usable.weekday(), 0, "Monday is 0");
        Ok(())
    }

    #[test]
    fn a_fill_after_the_cutoff_is_dated_to_the_next_settlement_day() -> Result<()> {
        // Friday 16:30 T+1: dated Monday, usable Tuesday. Friday 15:00 T+1
        // was Monday, so the cut-off alone moves the answer by a day.
        let terms = SettlementTerms::weekday(SettlementConvention::T1)?;
        let usable = terms.available_at(friday_1630())?;
        assert_eq!(usable.to_rfc3339(), "2025-10-14T09:00:00.000Z");
        Ok(())
    }

    #[test]
    fn a_holiday_is_skipped_not_counted() -> Result<()> {
        // Thursday T+2 is Monday; with Monday a holiday it is Tuesday.
        let monday = Timestamp::from_secs(1_760_313_600);
        assert_eq!(monday.weekday(), 0, "the premise: this is a Monday");
        let plain = SettlementTerms::weekday(SettlementConvention::T2)?;
        assert_eq!(
            plain.available_at(thursday(0))?.to_rfc3339(),
            "2025-10-13T09:00:00.000Z"
        );
        let holiday = plain.with_holiday(monday);
        assert_eq!(
            holiday.available_at(thursday(0))?.to_rfc3339(),
            "2025-10-14T09:00:00.000Z"
        );
        Ok(())
    }

    #[test]
    fn t_plus_zero_proceeds_are_never_usable_at_the_fill() -> Result<()> {
        // Same-day settlement is still not instant: the availability minute
        // is after the cut-off by construction, so a fill inside the cut-off
        // waits for it and a fill after the cut-off waits a day.
        let terms = SettlementTerms::weekday(SettlementConvention::T0)?;
        let inside = thursday(0);
        assert!(terms.available_at(inside)? > inside);
        assert_eq!(
            terms.available_at(inside)?.to_rfc3339(),
            "2025-10-09T17:00:00.000Z"
        );
        let after = friday_1630();
        assert_eq!(
            terms.available_at(after)?.to_rfc3339(),
            "2025-10-13T17:00:00.000Z"
        );
        Ok(())
    }

    #[test]
    fn availability_is_monotone_in_the_fill_instant() -> Result<()> {
        // The property `project` leans on: a later fill never lands
        // earlier, so projecting from the earliest instant a leg can fire
        // is the conservative reading.
        let terms = SettlementTerms::weekday(SettlementConvention::T2)?;
        let mut last = terms.available_at(thursday(0))?;
        for hour in 1..(24 * 14) {
            let next = terms.available_at(thursday(i64::from(hour) * 3_600))?;
            assert!(
                next >= last,
                "hour {hour} landed earlier than the hour before"
            );
            last = next;
        }
        Ok(())
    }

    #[test]
    fn terms_that_could_never_settle_are_refused_at_construction() {
        assert!(SettlementTerms::on_cycle(SettlementConvention::T1, 16 * 60, 9 * 60, []).is_err());
        assert!(
            SettlementTerms::on_cycle(SettlementConvention::T1, 1440, 9 * 60, [0]).is_err(),
            "a cut-off past the end of the day names no instant"
        );
        assert!(
            SettlementTerms::on_cycle(SettlementConvention::T0, 16 * 60, 9 * 60, [0]).is_err(),
            "T+0 handing over before its own cut-off is settlement before the fill"
        );
        assert!(SettlementTerms::on_cycle(SettlementConvention::T1, 16 * 60, 9 * 60, [7]).is_err());
        assert!(SettlementTerms::parse("T+3").is_err());
        assert!(SettlementTerms::parse("").is_err());
    }

    #[test]
    fn parse_reads_the_spellings_the_composition_root_writes() -> Result<()> {
        assert!(SettlementTerms::parse("instant")?.is_instant());
        assert_eq!(
            SettlementTerms::parse("T+2")?.convention(),
            Some(SettlementConvention::T2)
        );
        assert_eq!(
            SettlementTerms::parse(" t1 ")?.convention(),
            Some(SettlementConvention::T1)
        );
        Ok(())
    }

    #[test]
    fn the_first_leg_and_a_leg_after_a_venue_change_are_funded_from_what_is_held() -> Result<()> {
        let mut terms = BTreeMap::new();
        terms.insert(
            "A".to_string(),
            SettlementTerms::weekday(SettlementConvention::T2)?,
        );
        terms.insert(
            "B".to_string(),
            SettlementTerms::weekday(SettlementConvention::T2)?,
        );
        // A → B → B: the second leg follows a transfer the planner realised
        // as inventory at B, so only the third spends proceeds.
        let projection = project(&[v("A"), v("B"), v("B")], &terms, thursday(0))?;
        assert_eq!(projection.legs[0], LegFunding::Held);
        assert_eq!(projection.legs[1], LegFunding::Held);
        assert!(matches!(
            projection.legs[2],
            LegFunding::Proceeds { of_leg: 1, .. }
        ));
        let unfunded = projection
            .first_unfunded()
            .expect("the third leg spends T+2 proceeds the instant they are traded");
        assert_eq!((unfunded.leg, unfunded.of_leg), (2, 1));
        assert_eq!(unfunded.usable_at.to_rfc3339(), "2025-10-13T09:00:00.000Z");
        Ok(())
    }

    #[test]
    fn a_chain_at_an_instant_venue_is_funded_and_at_an_unknown_venue_is_counted() -> Result<()> {
        let mut terms = BTreeMap::new();
        terms.insert("A".to_string(), SettlementTerms::instant());
        let funded = project(&[v("A"), v("A"), v("A")], &terms, thursday(0))?;
        assert_eq!(funded.first_unfunded(), None);
        assert_eq!(funded.unprojected(), 0);
        assert!(matches!(
            funded.legs[1],
            LegFunding::Proceeds { of_leg: 0, usable_at } if usable_at == thursday(0)
        ));

        let unknown = project(&[v("Z"), v("Z"), v("Z")], &terms, thursday(0))?;
        assert_eq!(
            unknown.first_unfunded(),
            None,
            "an unknown venue is not judged"
        );
        assert_eq!(
            unknown.unprojected(),
            2,
            "and both dependent legs are counted"
        );
        Ok(())
    }
}
