//! Where each desk position is in its life, discover to attribute
//! (blueprint R12-ASSET-4071).
//!
//! [`qip_portfolio::LifeStage`] holds the checked edge table; until this
//! register its only use outside its own crate was a display string, so the
//! rule it exists for — nothing reaches `Attribute` without having settled —
//! bound no position the platform actually held. This is the kernel's
//! per-position record of that walk, advanced at the seams where each fact
//! becomes known:
//!
//! - **a fill** opening, resizing or closing a desk lot
//!   ([`PositionLives::on_fill`], called from the one desk booking seam
//!   `Platform::book_desk_fill`);
//! - **the venue's settlement calendar**, quoted for that fill in
//!   `Platform::register_settlement`, which schedules the instant an exited
//!   position settles ([`PositionLives::schedule_settlement`]);
//! - **LEARN**, which settles every exited position whose instant has passed
//!   and only then attributes it ([`PositionLives::settle_and_attribute`]).
//!
//! Settlement is scheduled at the fill and *happens* at the calendar's
//! instant, not at the fill. Marking `Settle` when the sale was booked would
//! make the guarantee vacuous: every exit would settle at its own fill, and a
//! T+1 sale whose ownership has not cleared would be credited the same cycle.
//!
//! Every transition goes through [`LifeStage::transition`]. A refused one is
//! returned as a problem for the cycle to carry and the life does not move —
//! it is never skipped forward or repaired, because a register that corrected
//! itself would hide exactly the caller bug the table is there to find.
//!
//! **Resume.** `Platform::resume_book` replays every journaled fill through
//! the same booking seam, so the register is rebuilt from the event log rather
//! than from a second record: open lives come back at `Hold`, exited ones at
//! `Exit` with the settlement instant the same calendar quotes for the same
//! fill. `Attribute` itself is not journaled, so a life attributed before a
//! restart comes back exited and is attributed again by the first LEARN after
//! it. That is safe in the direction that matters — it re-marks a stage and
//! credits nothing — and it never lets an unsettled life through.
//!
//! Scope: the desk book only. A cell's fills reach the risk aggregate through
//! `Platform::charge_cell_fills` and never a desk lot, so they have no life
//! here.

use qip_core::Decimal;
use qip_core::time::Timestamp;
use qip_portfolio::LifeStage;
use std::collections::{BTreeMap, VecDeque};

/// Exited lives awaiting settlement or attribution the register will hold.
///
/// A venue with no settlement calendar leaves its exits unscheduled forever,
/// so without a cap those would accumulate for the life of the process. Past
/// it the oldest unscheduled exit is dropped — never attributed — and the
/// drop is said as a problem: eviction can lose a record, it cannot credit
/// one.
pub const MAX_EXITED_LIVES: usize = 1_024;

/// Attributed lives kept for reading, most recent last.
pub const MAX_ATTRIBUTED_KEPT: usize = 64;

/// One position's life: its stage and, once exited, when it settles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PositionLife {
    stage: LifeStage,
    opened_at: Timestamp,
    exited_at: Option<Timestamp>,
    /// `None` until the venue's calendar has been quoted for the exit; an
    /// exit at a venue with no calendar stays `None` and never settles.
    settles_at: Option<Timestamp>,
    attributed_at: Option<Timestamp>,
}

impl PositionLife {
    pub fn stage(&self) -> LifeStage {
        self.stage
    }

    pub fn opened_at(&self) -> Timestamp {
        self.opened_at
    }

    pub fn exited_at(&self) -> Option<Timestamp> {
        self.exited_at
    }

    pub fn settles_at(&self) -> Option<Timestamp> {
        self.settles_at
    }

    pub fn attributed_at(&self) -> Option<Timestamp> {
        self.attributed_at
    }

    fn advance(&mut self, object: &str, next: LifeStage) -> Result<(), String> {
        self.stage = self.stage.transition(next).map_err(|error| {
            format!(
                "the position in {object} was not moved to {}: {}",
                next.as_str(),
                error.message()
            )
        })?;
        Ok(())
    }
}

/// What one LEARN pass did to the register.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LifePass {
    pub settled: usize,
    pub attributed: usize,
    /// Exited and not yet settled — either before the calendar's instant or
    /// at a venue that quoted none.
    pub awaiting_settlement: usize,
    pub open: usize,
    pub problems: Vec<String>,
}

impl LifePass {
    /// The LEARN detail line, or `None` when the register holds nothing and
    /// did nothing, so a cycle with no desk positions reads as it did before.
    pub fn describe(&self) -> Option<String> {
        (self.settled + self.attributed + self.awaiting_settlement + self.open > 0).then(|| {
            format!(
                "position lives: {} open, {} exited awaiting settlement, {} settled and {} \
                 attributed this cycle",
                self.open, self.awaiting_settlement, self.settled, self.attributed
            )
        })
    }
}

/// Every desk position not yet attributed, keyed by instrument and a
/// per-instrument generation — an instrument re-bought before its last
/// exit settled has two lives, and the second must not overwrite the
/// settlement the first still owes.
#[derive(Clone, Debug, Default)]
pub struct PositionLives {
    lives: BTreeMap<(String, u32), PositionLife>,
    attributed: VecDeque<(String, PositionLife)>,
}

impl PositionLives {
    /// The latest life of `object`: the open one if there is one, else the
    /// most recent exited or attributed one.
    pub fn stage_of(&self, object: &str) -> Option<LifeStage> {
        self.latest(object).map(|(_, life)| life.stage).or_else(|| {
            self.attributed
                .iter()
                .rev()
                .find(|(held, _)| held == object)
                .map(|(_, life)| life.stage)
        })
    }

    /// Lives not yet attributed, in key order.
    pub fn lives(&self) -> impl Iterator<Item = (&str, &PositionLife)> {
        self.lives
            .iter()
            .map(|((object, _), life)| (object.as_str(), life))
    }

    /// The most recently attributed lives, oldest first, bounded by
    /// [`MAX_ATTRIBUTED_KEPT`].
    pub fn attributed(&self) -> impl Iterator<Item = (&str, &PositionLife)> {
        self.attributed
            .iter()
            .map(|(object, life)| (object.as_str(), life))
    }

    fn latest(&self, object: &str) -> Option<(&(String, u32), &PositionLife)> {
        self.lives
            .range((object.to_string(), 0)..=(object.to_string(), u32::MAX))
            .next_back()
    }

    fn open_key(&self, object: &str) -> Option<(String, u32)> {
        self.latest(object)
            .filter(|(_, life)| life.exited_at.is_none())
            .map(|(key, _)| key.clone())
    }

    /// Advance the life of `object` for a fill that moved the desk's signed
    /// holding from `before` to `after`. Returns the problems the cycle must
    /// carry; an empty vector means every transition was admitted.
    ///
    /// Opening records `Discover`, `Evaluate`, `Acquire`, `Hold` together, at
    /// the fill. The kernel holds no per-instrument record of discovery: a
    /// desk order exists only because a proposal sized it from a thesis, and
    /// the order carries that proposal and its hypotheses, so the discovery
    /// and the evaluation are what the fill is evidence of rather than steps
    /// this seam could observe separately. They are still walked through the
    /// table, so an opening can never land a life anywhere but `Hold`.
    pub fn on_fill(
        &mut self,
        object: &str,
        before: Decimal,
        after: Decimal,
        at: Timestamp,
    ) -> Vec<String> {
        let mut problems = Vec::new();
        let was_open = !before.is_zero();
        let is_open = !after.is_zero();
        let flipped = was_open && is_open && before.signum() != after.signum();
        if was_open && (!is_open || flipped) {
            self.exit(object, at, &mut problems);
        } else if was_open && before != after {
            self.resize(object, &mut problems);
        }
        if is_open && (!was_open || flipped) {
            self.open(object, at, &mut problems);
        }
        problems
    }

    fn open(&mut self, object: &str, at: Timestamp, problems: &mut Vec<String>) {
        if self.open_key(object).is_some() {
            problems.push(format!(
                "a fill opened a position in {object} while the register already holds an open \
                 life for it; the book and the register disagree and the existing life is kept"
            ));
            return;
        }
        let generation = self
            .latest(object)
            .map_or(0, |((_, generation), _)| generation.saturating_add(1));
        let mut life = PositionLife {
            stage: LifeStage::Discover,
            opened_at: at,
            exited_at: None,
            settles_at: None,
            attributed_at: None,
        };
        for next in [LifeStage::Evaluate, LifeStage::Acquire, LifeStage::Hold] {
            if let Err(problem) = life.advance(object, next) {
                problems.push(problem);
                return;
            }
        }
        self.lives.insert((object.to_string(), generation), life);
    }

    fn resize(&mut self, object: &str, problems: &mut Vec<String>) {
        let Some(life) = self
            .open_key(object)
            .and_then(|key| self.lives.get_mut(&key))
        else {
            problems.push(format!(
                "a fill resized the position in {object}, which has no open life in the \
                 register; nothing is recorded for it rather than a life invented mid-way"
            ));
            return;
        };
        // The table admits a resize only out of `Monitor`: a size is changed
        // because somebody read the held position, not spontaneously. A desk
        // order is admitted only after the pre-trade check has read the
        // position it changes — its weight, its bucket, its tail — and that
        // read is the monitoring this fill is evidence of, so a life at
        // `Hold` walks through `Monitor` here rather than the table being
        // widened to let `Hold` skip it.
        let steps: &[LifeStage] = if life.stage == LifeStage::Hold {
            &[LifeStage::Monitor, LifeStage::Resize, LifeStage::Hold]
        } else {
            &[LifeStage::Resize, LifeStage::Hold]
        };
        for &next in steps {
            if let Err(problem) = life.advance(object, next) {
                problems.push(problem);
                return;
            }
        }
    }

    fn exit(&mut self, object: &str, at: Timestamp, problems: &mut Vec<String>) {
        let Some(key) = self.open_key(object) else {
            problems.push(format!(
                "a fill closed the position in {object}, which has no open life in the \
                 register; it will never be attributed"
            ));
            return;
        };
        if let Some(life) = self.lives.get_mut(&key) {
            match life.advance(object, LifeStage::Exit) {
                Ok(()) => life.exited_at = Some(at),
                Err(problem) => problems.push(problem),
            }
        }
        self.bound(problems);
    }

    /// Record when the latest exit of `object` settles, from the venue's
    /// calendar as quoted for the exiting fill. `due` is `None` when the
    /// venue has no calendar: the life is left unscheduled, which means it
    /// stays at `Exit` and is never attributed — the restrictive reading,
    /// never an instant settlement.
    ///
    /// Only an exit that has not been scheduled is touched, so a later fill
    /// at the same instrument cannot move a settlement already quoted.
    pub fn schedule_settlement(&mut self, object: &str, due: Option<Timestamp>) -> Option<String> {
        let (_, life) = self
            .lives
            .range_mut((object.to_string(), 0)..=(object.to_string(), u32::MAX))
            .rev()
            .find(|(_, life)| life.stage == LifeStage::Exit && life.settles_at.is_none())?;
        match due {
            Some(due) => {
                life.settles_at = Some(due);
                None
            }
            None => Some(format!(
                "the position in {object} exited at a venue with no settlement calendar; it \
                 stays at exit and is not attributed until a settlement date can be quoted"
            )),
        }
    }

    /// LEARN's step: settle every exited life whose instant is at or before
    /// `now`, then attribute every settled one. Only `Settle` is attributed —
    /// the transition table refuses `Exit` to `Attribute`, and this seam does
    /// not ask it to.
    pub fn settle_and_attribute(&mut self, now: Timestamp) -> LifePass {
        let mut pass = LifePass::default();
        for ((object, _), life) in &mut self.lives {
            if life.stage == LifeStage::Exit && life.settles_at.is_some_and(|due| due <= now) {
                match life.advance(object, LifeStage::Settle) {
                    Ok(()) => pass.settled += 1,
                    Err(problem) => pass.problems.push(problem),
                }
            }
        }
        let settled: Vec<(String, u32)> = self
            .lives
            .iter()
            .filter(|(_, life)| life.stage == LifeStage::Settle)
            .map(|(key, _)| key.clone())
            .collect();
        for key in settled {
            let Some(mut life) = self.lives.remove(&key) else {
                continue;
            };
            match life.advance(&key.0, LifeStage::Attribute) {
                Ok(()) => {
                    life.attributed_at = Some(now);
                    pass.attributed += 1;
                    if self.attributed.len() >= MAX_ATTRIBUTED_KEPT {
                        self.attributed.pop_front();
                    }
                    self.attributed.push_back((key.0, life));
                }
                Err(problem) => {
                    pass.problems.push(problem);
                    self.lives.insert(key, life);
                }
            }
        }
        for life in self.lives.values() {
            if life.stage == LifeStage::Exit {
                pass.awaiting_settlement += 1;
            } else {
                pass.open += 1;
            }
        }
        pass
    }

    /// Keep exited lives within [`MAX_EXITED_LIVES`], dropping the oldest
    /// unscheduled exit first and then the oldest exit at all. Never by
    /// attributing: a dropped life is lost, not credited.
    fn bound(&mut self, problems: &mut Vec<String>) {
        while self
            .lives
            .values()
            .filter(|life| life.exited_at.is_some())
            .count()
            > MAX_EXITED_LIVES
        {
            let oldest = |unscheduled_only: bool| {
                self.lives
                    .iter()
                    .filter(|(_, life)| life.exited_at.is_some())
                    .filter(|(_, life)| !unscheduled_only || life.settles_at.is_none())
                    .min_by_key(|(_, life)| life.exited_at.map(Timestamp::as_secs))
                    .map(|(key, _)| key.clone())
            };
            let Some(key) = oldest(true).or_else(|| oldest(false)) else {
                return;
            };
            self.lives.remove(&key);
            problems.push(format!(
                "the exited position in {} was dropped from the life register unattributed, \
                 which holds at most {MAX_EXITED_LIVES} exits awaiting settlement",
                key.0
            ));
        }
    }
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use qip_core::dec;

    fn at(secs: i64) -> Timestamp {
        Timestamp::from_secs(secs)
    }

    #[test]
    fn a_flip_through_zero_exits_one_life_and_opens_another() {
        let mut lives = PositionLives::default();
        assert!(lives.on_fill("X", dec!("0"), dec!("5"), at(1)).is_empty());
        assert!(lives.on_fill("X", dec!("5"), dec!("-3"), at(2)).is_empty());
        let stages: Vec<LifeStage> = lives.lives().map(|(_, life)| life.stage()).collect();
        assert_eq!(stages, vec![LifeStage::Exit, LifeStage::Hold]);
        // The settlement is owed by the exited life, not the new one.
        assert!(lives.schedule_settlement("X", Some(at(10))).is_none());
        let pass = lives.settle_and_attribute(at(10));
        assert_eq!((pass.settled, pass.attributed, pass.open), (1, 1, 1));
        assert_eq!(lives.stage_of("X"), Some(LifeStage::Hold));
    }

    #[test]
    fn an_exit_at_a_venue_with_no_calendar_is_never_attributed() {
        let mut lives = PositionLives::default();
        assert!(lives.on_fill("X", dec!("0"), dec!("5"), at(1)).is_empty());
        assert!(lives.on_fill("X", dec!("5"), dec!("0"), at(2)).is_empty());
        assert!(lives.schedule_settlement("X", None).is_some());
        let pass = lives.settle_and_attribute(at(i64::from(i32::MAX)));
        assert_eq!((pass.attributed, pass.awaiting_settlement), (0, 1));
        assert_eq!(lives.stage_of("X"), Some(LifeStage::Exit));
    }

    #[test]
    fn a_resize_on_an_instrument_with_no_open_life_is_a_problem_not_an_invented_life() {
        let mut lives = PositionLives::default();
        let problems = lives.on_fill("X", dec!("5"), dec!("7"), at(1));
        assert_eq!(problems.len(), 1);
        assert_eq!(lives.stage_of("X"), None);
    }
}
