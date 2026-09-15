//! The producer of the shipping payload's slot 3 — blueprint §41.5's belief
//! priors — as the centre's own reasoning can honestly state it.
//!
//! # Why this slot, and why now
//!
//! [`crate::central::whitelist`]'s audit grouped belief priors, the episodic
//! digest and the causal digest together as "the kernel holds no belief
//! engine, no episodic store, and no causal edge". The episodic third stopped
//! holding when LEARN began moving resolved theses into memory, and
//! [`super::episodic`] became slot 4's producer. The belief third held on a
//! narrower and correct argument, which that audit stated as a requirement
//! rather than as an impossibility: `BeliefState` records *when* a belief was
//! last formed and how many have been, never the beliefs themselves, so
//! "what would have to exist is a belief the centre holds **per subject a
//! cell trades**, retained past the cycle that formed it."
//!
//! That is `Platform::pending_episodes`. REASON drafts an [`Episode`] for
//! every hypothesis it forms — keyed by the instrument the claim is about,
//! carrying the effective confidence after review — and LEARN removes it only
//! when the claim resolves. Between those two instants the draft *is* the
//! centre's open belief about that instrument, keyed the way slot 3 is keyed
//! and retained past the cycle that formed it. This module states that set,
//! and nothing else.
//!
//! # What producing this slot does to a cell, said plainly
//!
//! It relaxes it. `PolicyItem::BeliefPriors` maps to
//! [`qip_contracts::degradation::Capability::BeliefState`], and
//! `DegradationState::sizing_multiplier` stops applying
//! `BELIEF_STALE_MULTIPLIER` — a halving — the moment the slot reads fresh.
//! So a slot produced on a belief nobody formed would double every receiving
//! cell's size against a signed payload it has no way to doubt. Four refusals
//! keep that from happening, and each is structural rather than a caller's
//! discipline:
//!
//! * **A process that has formed no belief produces nothing at all.** Not an
//!   empty map, not a map of whatever drafts happen to be pending: the slot
//!   ships unproduced and the cell narrows exactly as every deployed cell
//!   does today. The gate is `BeliefState::last_updated()`, which is the same
//!   fact `Platform::central_degradation` reads for the centre's own §6.2 row
//!   4, so the centre and the cell cannot disagree about whether a belief
//!   exists.
//! * **Only beliefs inside the slot's own freshness window are carried**, and
//!   the slot is stamped with the **oldest** of those — never `now`, never
//!   the newest. One instant is asserted over a whole set, and the only
//!   instant that overclaims no member of the set is its oldest member's.
//!   Stamping the newest would let one belief formed this second vouch for a
//!   map of beliefs from last week; stamping `now` would let a platform that
//!   stopped reasoning entirely keep every cell at full size for as long as
//!   payloads kept being issued. The window is
//!   `PolicyItem::BeliefPriors::time_to_live()` itself, so the set can never
//!   contain a belief older than the freshness its own stamp claims.
//! * **A belief the engine never absorbed is refused, not shipped.** A draft
//!   dated after `BeliefState::last_updated()` means the two records of one
//!   fact have drifted — every draft is written downstream of
//!   `ReasoningEngine::reason`, which absorbs at the same instant — and a
//!   slot built from the louder of two disagreeing claims is the failure this
//!   platform names in its own principles. Same for a draft dated after the
//!   issue instant: that is a clock fault, named where it happened.
//! * **[`BeliefIssue::slot`] is the only route from here into a
//!   [`qip_contracts::policy::Slot`]**, and it carries the map and its instant
//!   together or neither. A shipper cannot stamp its own clock on a belief
//!   state that stopped moving.
//!
//! # The f64/Decimal crossing, named where it happens
//!
//! A prior is a confidence, which is a statistic, so the map's values are
//! `f64` — the domain rule's own carve-out, and the type
//! [`qip_contracts::policy::BeliefPriors`] already fixes. No money crosses
//! this module. What the *freshness* of this slot moves at a cell is
//! `DegradationState::sizing_multiplier`, which is a `Decimal`, and the
//! crossing happens there rather than here: nothing in this file multiplies a
//! confidence by a quantity.
//!
//! # What no cell reads
//!
//! The values. `grep -rn 'belief_priors' backend/crates/edge
//! backend/crates/apps/qip-edge-node --include=*.rs | grep -v /tests/` finds
//! nothing, and that is the honest state: what reaches a cell's decision is
//! this slot's *freshness*, through `PolicyItem::capability`. The map is
//! carried because a freshness claim with nothing behind it is a claim about
//! nothing, and because slot 3's type names priors keyed by subject — but do
//! not describe a cell as acting on a prior. It does not.

use qip_ai::memory::Episode;
use qip_contracts::policy::{BeliefPriors, PolicyItem, Slot};
use qip_core::error::{Error, Result};
use qip_core::time::{Duration, Timestamp};
use qip_events::{EventBody, Topic};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Why a cell's slot 3 carries what it carries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BeliefOutcome {
    /// The reasoning engine has never turned evidence into a belief in this
    /// process, so no prior is shipped whatever is pending. `open` is how
    /// many drafts are held, which is deliberately reported even though none
    /// is shipped: a platform holding drafts it never formed a belief for is
    /// a different fault from a platform that has simply not reasoned yet.
    NeverFormed { open: usize },
    /// Beliefs exist but every one of them is older than the window slot 3's
    /// own time to live defines, so none can be stamped without overclaiming.
    NothingCurrent {
        open: usize,
        aged_out: usize,
        formed_at: Timestamp,
    },
    /// A map of the beliefs inside the window, current as of the oldest of
    /// them.
    Produced {
        subjects: u64,
        oldest_current: Timestamp,
        aged_out: usize,
    },
}

/// One cycle's slot 3, and why — the record the journal keeps.
///
/// One per issue rather than one per cell: the beliefs are the platform's, so
/// every cell in a cycle receives the same map, and journaling it per cell
/// would be seven records of one fact.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BeliefIssue {
    pub issued_at: Timestamp,
    /// The priors and the instant they are current as of, together or not at
    /// all. Private because the pair is the invariant: a map that could be
    /// stamped with an instant from anywhere else is the overclaim this
    /// module exists to refuse.
    produced: Option<(BeliefPriors, Timestamp)>,
    pub outcome: BeliefOutcome,
}

impl BeliefIssue {
    /// Derive the priors from the beliefs the centre holds open at `now`.
    ///
    /// `open` is `Platform::pending_episodes` — every hypothesis REASON has
    /// formed and LEARN has not yet resolved — and `formed_at` is
    /// `BeliefState::last_updated()`. Two arguments rather than the platform
    /// itself so that this module needs nothing of the kernel but the two
    /// facts, and so a test can drive the never-formed and the drifted cases
    /// the production path will not produce.
    ///
    /// Refuses rather than repairs in three places, each named in the module
    /// register above: a belief dated after `now`, a belief dated after the
    /// engine's own last absorption, and a confidence outside `[0, 1]` or not
    /// finite. The last is not pedantry — a `NaN` confidence serialises to
    /// nothing `serde_json` will emit, so it would fail at
    /// `PolicyPayload::signing_payload` and take the *whole* payload down,
    /// every slot with it, rather than the one belief that was wrong.
    pub fn derive<'a>(
        open: impl Iterator<Item = &'a Episode>,
        formed_at: Option<Timestamp>,
        now: Timestamp,
    ) -> Result<Self> {
        let open: Vec<&Episode> = open.collect();
        let Some(formed_at) = formed_at else {
            return Ok(Self {
                issued_at: now,
                produced: None,
                outcome: BeliefOutcome::NeverFormed { open: open.len() },
            });
        };
        if formed_at > now {
            return Err(Error::invalid(format!(
                "the reasoning engine reports its newest belief formed at {formed_at:?}, after \
                 the issue instant {now:?}; a belief cannot be formed in the future, so this is \
                 a clock fault rather than a prior to stamp"
            )));
        }

        // The window slot 3's own time to live defines. Read from
        // `PolicyItem` rather than restated, so a change to the contract's
        // cadence moves the producer with it instead of leaving two numbers
        // to disagree.
        let window: Duration = PolicyItem::BeliefPriors.time_to_live();
        let earliest_current = now.saturating_sub(window);

        let mut current: Vec<&Episode> = Vec::new();
        let mut aged_out = 0usize;
        for episode in &open {
            if episode.at > now {
                return Err(Error::invalid(format!(
                    "belief {} on {} is dated {:?}, after the issue instant {now:?}; a prior \
                     whose age cannot be accounted for must not be stamped",
                    episode.episode_id, episode.instrument, episode.at
                )));
            }
            if episode.at > formed_at {
                return Err(Error::invalid(format!(
                    "belief {} on {} is dated {:?} but the reasoning engine last absorbed \
                     evidence at {formed_at:?}; two records of one fact have drifted, and the \
                     newer is not evidence that the older is wrong",
                    episode.episode_id, episode.instrument, episode.at
                )));
            }
            let confidence = episode.claim.confidence;
            if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
                return Err(Error::invalid(format!(
                    "belief {} on {} carries confidence {confidence}, which is not a probability; \
                     a slot built from it would refuse to sign and take every other slot of the \
                     payload with it",
                    episode.episode_id, episode.instrument
                )));
            }
            if episode.at < earliest_current {
                aged_out += 1;
                continue;
            }
            current.push(episode);
        }

        let Some(oldest_current) = current.iter().map(|episode| episode.at).min() else {
            return Ok(Self {
                issued_at: now,
                produced: None,
                outcome: BeliefOutcome::NothingCurrent {
                    open: open.len(),
                    aged_out,
                    formed_at,
                },
            });
        };

        // Sorted on the pair rather than left in the platform's order,
        // because the map reaches a signed payload and a replay that reorders
        // is not a replay. Ids are unique among the open drafts, so the pair
        // is a total order over what is here, and inserting in ascending
        // order means the newest belief about a subject is the one that
        // survives — a prior is what the platform believes now, not what it
        // believed first.
        current.sort_by(|left, right| {
            (left.at, left.episode_id.as_str()).cmp(&(right.at, right.episode_id.as_str()))
        });
        let mut priors: BTreeMap<String, f64> = BTreeMap::new();
        for episode in &current {
            priors.insert(episode.instrument.clone(), episode.claim.confidence);
        }

        Ok(Self {
            issued_at: now,
            outcome: BeliefOutcome::Produced {
                subjects: priors.len() as u64,
                oldest_current,
                aged_out,
            },
            produced: Some((BeliefPriors { priors }, oldest_current)),
        })
    }

    /// The payload slot this issue ships, produced or not.
    ///
    /// The only route from here into a payload. A produced slot carries the
    /// oldest current belief's instant, so the cell's §6.2 row 4 goes stale on
    /// the platform's silence rather than on the shipper's.
    pub fn slot(&self) -> Slot<BeliefPriors> {
        match &self.produced {
            Some((priors, produced_at)) => Slot::produced(priors.clone(), *produced_at),
            None => Slot::unproduced(),
        }
    }

    /// The priors, where any were produced.
    pub fn priors(&self) -> Option<&BeliefPriors> {
        self.produced.as_ref().map(|(priors, _)| priors)
    }

    /// The line an operator reads.
    pub fn describe(&self) -> String {
        match &self.outcome {
            BeliefOutcome::NeverFormed { open } => format!(
                "belief priors: not shipped, the reasoning engine has formed no belief in this \
                 process and {open} draft(s) are open; every cell reads the slot unavailable and \
                 halves its size"
            ),
            BeliefOutcome::NothingCurrent {
                open,
                aged_out,
                formed_at,
            } => format!(
                "belief priors: not shipped, all {open} open belief(s) are older than the slot's \
                 {:?} window ({aged_out} aged out, newest absorption {formed_at:?}); every cell \
                 reads the slot unavailable and halves its size",
                PolicyItem::BeliefPriors.time_to_live()
            ),
            BeliefOutcome::Produced {
                subjects,
                oldest_current,
                aged_out,
            } => format!(
                "belief priors: {subjects} subject(s), oldest current belief at \
                 {oldest_current:?}, which is the instant the slot's freshness is measured from; \
                 {aged_out} older belief(s) left out"
            ),
        }
    }
}

impl EventBody for BeliefIssue {
    // What the centre distributed as policy, recorded whether or not it was
    // anything: a platform whose beliefs never reach a cell is exactly the
    // fact an operator asking why every region sizes at half has to find.
    const TOPIC: Topic = Topic::PolicyDistributed;
    const SCHEMA_VERSION: u32 = 1;
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_ai::memory::{ClaimRecord, DecisionTaken, FindingsSummary, RegimeLabel};
    use qip_contracts::degradation::{Capability, Freshness};
    use qip_contracts::policy::PolicyPayload;

    fn at(secs: i64) -> Timestamp {
        Timestamp::from_secs(secs)
    }

    fn belief(id: &str, instrument: &str, confidence: f64, formed: Timestamp) -> Episode {
        Episode {
            episode_id: id.to_string(),
            instrument: instrument.to_string(),
            regime: RegimeLabel {
                market: "calm".to_string(),
                volatility: "low".to_string(),
            },
            state: None,
            causal_context: Vec::new(),
            findings: FindingsSummary {
                runs: 1,
                findings: 1,
                coverage: 1.0,
                contested: false,
            },
            stances: Vec::new(),
            claim: ClaimRecord {
                class: "mean_reversion".to_string(),
                claim: "undervalued".to_string(),
                direction: 1.0,
                confidence,
            },
            horizon: Duration::from_hours(24),
            decision: DecisionTaken::Approved,
            outcome: None,
            // A draft is knowable from the instant it is formed; LEARN
            // restamps `known_at` at resolution, by which point it is no
            // longer open and no longer this module's business.
            at: formed,
            known_at: formed,
        }
    }

    #[test]
    fn a_process_that_has_formed_no_belief_ships_no_prior_however_many_drafts_are_open() {
        // The refusal that keeps §6.2 row 4 fail-closed. Drafts without a
        // formed belief cannot happen through the production path, and the
        // point of gating on the engine's own record rather than on the
        // drafts is that the centre's table and the cell's read one fact: a
        // centre reading row 4 unavailable must not ship a cell a row 4 that
        // is fresh.
        let drafts = [
            belief("ep-a", "AAA", 0.7, at(1_000)),
            belief("ep-b", "BBB", 0.4, at(1_100)),
        ];
        let issue = BeliefIssue::derive(drafts.iter(), None, at(1_200))
            .expect("a never-reasoned engine is not an error");
        assert_eq!(issue.slot(), Slot::unproduced());
        assert_eq!(issue.priors(), None);
        assert_eq!(
            issue.outcome,
            BeliefOutcome::NeverFormed { open: 2 },
            "the operator line has to say two drafts are held, not zero"
        );
        assert!(
            issue.describe().contains("2 draft(s) are open"),
            "{}",
            issue.describe()
        );
    }

    #[test]
    fn a_produced_slot_is_stamped_with_the_oldest_current_belief_and_not_the_issue_instant() {
        // The whole safety argument of this module. One instant is asserted
        // over a set, so it must be the set's oldest member: stamping the
        // newest would let a belief formed this second vouch for one formed
        // four minutes ago, and stamping `now` would keep every cell at full
        // size for as long as payloads were issued after the platform stopped
        // reasoning.
        let issued_at = at(10_000);
        let older = belief("ep-old", "AAA", 0.7, at(9_800));
        let newer = belief("ep-new", "BBB", 0.4, at(9_990));
        let drafts = [newer, older];
        let issue = BeliefIssue::derive(drafts.iter(), Some(at(9_990)), issued_at)
            .expect("both beliefs are inside the window");

        assert_eq!(issue.slot().produced_at(), Some(at(9_800)));
        assert_ne!(issue.slot().produced_at(), Some(issued_at));
        assert_ne!(
            issue.slot().produced_at(),
            Some(at(9_990)),
            "the newest belief must not vouch for the oldest"
        );
        let priors = issue.priors().expect("produced");
        assert_eq!(priors.priors.len(), 2);
        assert_eq!(priors.priors.get("AAA"), Some(&0.7));
        assert_eq!(priors.priors.get("BBB"), Some(&0.4));
    }

    #[test]
    fn every_belief_older_than_the_slots_own_window_is_left_out_and_counted() {
        // A set whose oldest member is beyond the window could never be
        // stamped honestly, so it is not shipped at all rather than shipped
        // trimmed to whatever survived — and the beliefs that *are* inside the
        // window still ship when some of them are not.
        let issued_at = at(10_000);
        let window = PolicyItem::BeliefPriors.time_to_live();
        assert_eq!(
            window,
            Duration::from_secs(300),
            "the premise: slot 3's window is five minutes"
        );

        let ancient = belief("ep-ancient", "AAA", 0.9, at(9_000));
        let current = belief("ep-current", "BBB", 0.3, at(9_900));
        let drafts = [ancient.clone(), current];
        let mixed = BeliefIssue::derive(drafts.iter(), Some(at(9_900)), issued_at)
            .expect("one belief is inside the window");
        let priors = mixed.priors().expect("the current belief ships");
        assert_eq!(
            priors.priors.keys().collect::<Vec<_>>(),
            vec!["BBB"],
            "a belief a thousand seconds old rode in on a three-hundred-second stamp"
        );
        assert_eq!(
            mixed.outcome,
            BeliefOutcome::Produced {
                subjects: 1,
                oldest_current: at(9_900),
                aged_out: 1,
            }
        );

        // And with nothing inside the window, nothing is produced at all.
        let only_ancient = [ancient];
        let none = BeliefIssue::derive(only_ancient.iter(), Some(at(9_000)), issued_at)
            .expect("an aged-out set is not an error");
        assert_eq!(none.slot(), Slot::unproduced());
        assert_eq!(
            none.outcome,
            BeliefOutcome::NothingCurrent {
                open: 1,
                aged_out: 1,
                formed_at: at(9_000),
            }
        );
    }

    #[test]
    fn a_belief_the_engine_never_absorbed_or_dated_after_the_issue_instant_is_refused() {
        // Two records of one fact that have drifted, and a clock that ran
        // backwards. Both are unreachable through the production path — every
        // draft is written downstream of `ReasoningEngine::reason`, which
        // absorbs at the same instant — and both are refused rather than
        // stamped, because a prior whose age cannot be accounted for is one
        // nobody can audit.
        let drifted = [belief("ep-drift", "AAA", 0.7, at(9_990))];
        let error = BeliefIssue::derive(drifted.iter(), Some(at(9_900)), at(10_000))
            .expect_err("a belief newer than the engine's own record is drift");
        assert!(error.to_string().contains("have drifted"), "{error}");

        let future = [belief("ep-future", "AAA", 0.7, at(10_001))];
        let error = BeliefIssue::derive(future.iter(), Some(at(10_001)), at(10_000))
            .expect_err("a belief dated after the issue instant is a clock fault");
        assert!(
            error.to_string().contains("after the issue instant"),
            "{error}"
        );

        let ahead = [belief("ep-ok", "AAA", 0.7, at(9_900))];
        let error = BeliefIssue::derive(ahead.iter(), Some(at(10_001)), at(10_000))
            .expect_err("an engine whose record is ahead of the clock is a clock fault");
        assert!(error.to_string().contains("clock fault"), "{error}");
    }

    #[test]
    fn a_confidence_that_is_not_a_probability_is_refused_rather_than_signed() {
        // `NaN` has no `serde_json` representation, so a slot carrying one
        // fails at `PolicyPayload::signing_payload` and takes the grant
        // manifest, the risk envelope and the whitelist down with it. Refused
        // here, where the one bad belief can be named.
        for bad in [f64::NAN, f64::INFINITY, 1.5, -0.1] {
            let drafts = [belief("ep-bad", "AAA", bad, at(9_900))];
            let error = BeliefIssue::derive(drafts.iter(), Some(at(9_900)), at(10_000))
                .expect_err("a confidence outside [0, 1] is not a probability");
            assert!(
                error.to_string().contains("is not a probability"),
                "{bad}: {error}"
            );
        }
    }

    #[test]
    fn the_newest_belief_about_a_subject_is_the_one_that_ships() {
        // Two open drafts on one instrument is the ordinary case — REASON
        // forms a belief per cycle and LEARN resolves them days later — and
        // the map holds one value per subject. Which one is not arbitrary: a
        // prior is what the platform believes now.
        let older = belief("ep-1", "AAA", 0.2, at(9_800));
        let newer = belief("ep-2", "AAA", 0.8, at(9_900));
        let drafts = [newer, older];
        let issue = BeliefIssue::derive(drafts.iter(), Some(at(9_900)), at(10_000))
            .expect("both beliefs are inside the window");
        let priors = issue.priors().expect("produced");
        assert_eq!(
            priors.priors.len(),
            1,
            "one subject, one prior: {:?}",
            priors.priors
        );
        assert_eq!(priors.priors.get("AAA"), Some(&0.8));
        // And the stamp is still the oldest of the two, because the older
        // draft is still an open belief this map speaks for.
        assert_eq!(issue.slot().produced_at(), Some(at(9_800)));
    }

    #[test]
    fn a_fresh_prior_is_what_stops_a_cell_halving_its_size_and_silence_restores_the_halving() {
        // What producing this slot actually changes at a cell, asserted
        // through the payload rather than described in a comment — and the
        // premise first, because a test that only asserted the fresh case
        // would pass on a payload that never narrows anything.
        let issued_at = at(10_000);
        let unproduced = PolicyPayload::unproduced(1, "cell-1", issued_at);
        let floor = unproduced.narrowing(issued_at).sizing_multiplier();
        assert_eq!(
            unproduced
                .narrowing(issued_at)
                .freshness(Capability::BeliefState),
            Freshness::Unavailable,
            "premise: an unproduced slot 3 must read unavailable, or this test proves nothing"
        );

        let drafts = [belief("ep-1", "AAA", 0.8, at(9_940))];
        let issue = BeliefIssue::derive(drafts.iter(), Some(at(9_940)), issued_at)
            .expect("one belief a minute old");
        let mut payload = PolicyPayload::unproduced(2, "cell-1", issued_at);
        payload.belief_priors = issue.slot();
        let narrowing = payload.narrowing(issued_at);
        assert_eq!(
            narrowing.freshness(Capability::BeliefState),
            Freshness::Fresh
        );
        assert_eq!(
            narrowing.sizing_multiplier(),
            floor
                .checked_mul(qip_core::Decimal::from_int(2))
                .expect("doubling a multiplier no larger than one is representable"),
            "a fresh belief row must stop the halving, which is the whole reason this slot is \
             worth producing and the whole reason it must not be produced on a guess"
        );

        // Six minutes later the same payload's slot is past its five-minute
        // time to live, and the halving returns without anything being
        // republished. The payload's own `valid_for` is 300s too, so this is
        // the belt as well as the braces — both age out, and a cell that
        // stopped hearing from the centre narrows either way.
        let later = issued_at.saturating_add(Duration::from_secs(360));
        assert_eq!(
            payload.narrowing(later).sizing_multiplier(),
            floor,
            "a prior older than its time to live must stop excusing full size"
        );
    }
}
