//! The producer of the shipping payload's slot 4 — blueprint §41.5's episodic
//! digest — as the centre's own memory can honestly state it.
//!
//! # Why this slot, and why now
//!
//! [`crate::central::whitelist`]'s audit of the nine unproduced slots grouped
//! belief priors, the episodic digest and the causal digest together as "the
//! kernel holds no belief engine, no episodic store, and no causal edge".
//! Belief priors still hold. The causal-edge third held until ADR 0054 gave
//! `WorldModel::claim_causal` a second, real caller
//! (`Platform::discover_temporal_precedence`) — the kernel can now hold a
//! causal edge, narrowly and rarely, which `whitelist.rs`'s own bullet
//! restates and dates; this module is still not that producer, and the
//! `causal_digest` slot is still unproduced regardless. The episodic third
//! does not hold either, and stopped holding when the LEARN stage began
//! moving a resolved thesis's episode into memory: `Platform::remember_resolved`
//! is called from `calibrate_resolved`, which `stage_learn` calls, so
//! `Platform::episodes` is written by the production cycle and not only by a
//! test. The store is real, its contents are the platform's own resolved
//! reasoning, and this module states what it holds.
//!
//! # What is asserted, and what it costs if it is wrong
//!
//! `EpisodicDigest` is a manifest, in the same sense
//! [`qip_contracts::policy::GrantManifest`] is one: a digest and a count, not
//! a delivery path. No cell retrieves an analogue from it. What it changes at
//! a cell is §6.2 row 3 — [`qip_contracts::degradation::DegradationState`]
//! reads the slot's freshness as [`qip_contracts::degradation::Capability`]'s
//! `EpisodicMemory` row, and a fresh row stops pausing strategies that depend
//! on situational recognition. So a digest asserted fresh on a memory that
//! had absorbed nothing would restart exactly the strategies the row exists
//! to stop. Three refusals keep that from happening, and each is structural
//! rather than a caller's discipline:
//!
//! * A memory with nothing **knowable** at the issue instant produces no
//!   digest at all. Not an empty digest, not a digest of the pending
//!   episodes: the slot ships unproduced and the cell narrows exactly as it
//!   does today.
//! * The instant stamped on the slot is the newest **knowable** episode's,
//!   never the issue instant. Freshness measures the fact and not the
//!   envelope, and slot 4's time to live is ten minutes, so a memory that has
//!   absorbed nothing for ten minutes reads stale at the cell and the pause
//!   returns. That is the common case — theses resolve over days — and it is
//!   the honest one.
//! * [`EpisodicIssue::slot`] is the only way to reach a
//!   [`qip_contracts::policy::Slot`] from here, and it carries the digest and
//!   its instant together or neither. A shipper cannot stamp `now` on a
//!   digest of a memory that stopped moving last week.
//!
//! # Bitemporality is the memory's, not this module's
//!
//! `EpisodicMemory::episodes` yields only what is knowable strictly before
//! the instant it is asked about, which is the same filter `recall` applies.
//! This module takes that iterator and never reaches past it. An episode
//! remembered at the very instant a digest is taken is therefore not in it,
//! and the next cycle's digest is the first that names it — a cycle's delay
//! on a ten-minute time to live, and the alternative is a digest that names
//! a resolution the platform had not yet finished writing.

use qip_ai::memory::Episode;
use qip_contracts::policy::{EpisodicDigest, Slot};
use qip_core::error::{Error, Result};
use qip_core::hash::sha256_hex;
use qip_core::time::Timestamp;
use qip_events::{EventBody, Topic};
use serde::{Deserialize, Serialize};

/// Why a cell's slot 4 carries what it carries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpisodicOutcome {
    /// Nothing in memory was knowable at the issue instant, so no digest was
    /// produced. `held` is what memory holds in total, which is not the same
    /// number: an episode remembered this instant is held and not yet
    /// knowable, and reporting only the knowable count would read as an empty
    /// memory to an operator watching one fill up.
    NothingKnowable { held: usize },
    /// A digest over the episodes knowable at the issue instant, current as
    /// of the newest of them.
    Produced {
        episodes: u64,
        newest_knowable: Timestamp,
    },
}

/// One cycle's slot 4, and why — the record the journal keeps.
///
/// One per issue rather than one per cell: the memory is the platform's, so
/// every cell in a cycle receives the same digest, and journaling it once per
/// cell would be seven records of one fact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpisodicIssue {
    pub issued_at: Timestamp,
    /// The digest and the instant it is current as of, together or not at
    /// all. Private because the pair is the invariant: a digest that could be
    /// stamped with an instant from anywhere else is the overclaim this
    /// module exists to refuse.
    produced: Option<(EpisodicDigest, Timestamp)>,
    pub outcome: EpisodicOutcome,
}

impl EpisodicIssue {
    /// Derive the digest from every episode knowable at `now`.
    ///
    /// `knowable` is `EpisodicMemory::episodes(now)` and `held` is
    /// `EpisodicMemory::len()`. Two arguments rather than the memory itself so
    /// that this crate's producer needs nothing of the store but the two
    /// facts, and so a test can drive the empty and the future cases the store
    /// will not produce.
    ///
    /// Refuses an episode knowable at or after `now`. The store's own filter
    /// makes that unreachable, and a producer that read one anyway would stamp
    /// a slot with an instant from the future — which
    /// `Slot::freshness` reads as a clock fault and narrows on, so nothing
    /// unsafe would ship, but the payload would carry a digest whose age no
    /// operator could account for. Refusing names the fault where it happened.
    pub fn derive<'a>(
        knowable: impl Iterator<Item = &'a Episode>,
        held: usize,
        now: Timestamp,
    ) -> Result<Self> {
        // Sorted on the pair rather than left in the store's order, because
        // the digest reaches a signed payload and a replay that reorders is
        // not a replay. Ids are unique in the store, so the pair is a total
        // order over what is here.
        let mut episodes: Vec<&Episode> = knowable.collect();
        episodes.sort_by(|left, right| {
            (left.known_at, left.episode_id.as_str())
                .cmp(&(right.known_at, right.episode_id.as_str()))
        });

        let Some(newest_knowable) = episodes.last().map(|episode| episode.known_at) else {
            return Ok(Self {
                issued_at: now,
                produced: None,
                outcome: EpisodicOutcome::NothingKnowable { held },
            });
        };
        if newest_knowable >= now {
            return Err(Error::invalid(format!(
                "an episode knowable at {newest_knowable:?} was offered to a digest taken at \
                 {now:?}; memory yields only what is knowable strictly earlier, so this is a \
                 caller reaching past that filter rather than a digest to stamp"
            )));
        }

        // The digest is over the serialised episodes rather than over a
        // grammar invented here: the payload's own slot digests are taken the
        // same way, and a second encoding of the same facts is a second thing
        // that can disagree about them.
        let bytes = serde_json::to_vec(&episodes).map_err(|error| {
            Error::invalid(format!(
                "the episodes knowable at {now:?} cannot be serialised, so no digest names them: \
                 {error}"
            ))
        })?;
        Ok(Self {
            issued_at: now,
            produced: Some((
                EpisodicDigest {
                    digest: sha256_hex(&bytes),
                    episodes: episodes.len() as u64,
                },
                newest_knowable,
            )),
            outcome: EpisodicOutcome::Produced {
                episodes: episodes.len() as u64,
                newest_knowable,
            },
        })
    }

    /// The payload slot this issue ships, produced or not.
    ///
    /// The only route from here into a payload. A produced slot carries the
    /// newest knowable episode's instant, so the cell's §6.2 row goes stale
    /// on the memory's silence rather than on the shipper's.
    pub fn slot(&self) -> Slot<EpisodicDigest> {
        match &self.produced {
            Some((digest, produced_at)) => Slot::produced(digest.clone(), *produced_at),
            None => Slot::unproduced(),
        }
    }

    /// The digest, where one was produced.
    pub fn digest(&self) -> Option<&EpisodicDigest> {
        self.produced.as_ref().map(|(digest, _)| digest)
    }

    /// The line an operator reads.
    pub fn describe(&self) -> String {
        match &self.outcome {
            EpisodicOutcome::NothingKnowable { held } => format!(
                "episodic digest: not shipped, memory holds {held} episode(s) and none is \
                 knowable yet; every cell reads the slot unavailable and pauses situational \
                 recognition"
            ),
            EpisodicOutcome::Produced {
                episodes,
                newest_knowable,
            } => format!(
                "episodic digest: {episodes} episode(s), newest knowable at {newest_knowable:?}, \
                 which is the instant the slot's freshness is measured from"
            ),
        }
    }
}

impl EventBody for EpisodicIssue {
    // What the centre distributed as policy, recorded whether or not it was
    // anything: a memory that never becomes knowable is exactly the fact an
    // operator asking why every cell still pauses needs to find.
    const TOPIC: Topic = Topic::PolicyDistributed;
    const SCHEMA_VERSION: u32 = 1;
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_ai::memory::{ClaimRecord, DecisionTaken, FindingsSummary, RegimeLabel};
    use qip_contracts::degradation::{Capability, Freshness, StrategyClass};
    use qip_contracts::policy::{PolicyItem, PolicyPayload};
    use qip_core::time::Duration;

    fn at(secs: i64) -> Timestamp {
        Timestamp::from_secs(secs)
    }

    fn episode(id: &str, known_at: Timestamp) -> Episode {
        Episode {
            episode_id: id.to_string(),
            instrument: "AAA".to_string(),
            regime: RegimeLabel {
                market: "calm".to_string(),
                volatility: "low".to_string(),
            },
            findings: FindingsSummary {
                runs: 1,
                findings: 1,
                coverage: 1.0,
                contested: false,
            },
            stances: Vec::new(),
            claim: ClaimRecord {
                class: "mean_reversion".to_string(),
                claim: "reverts".to_string(),
                direction: 1.0,
                confidence: 0.6,
            },
            horizon: Duration::from_hours(24),
            decision: DecisionTaken::Approved,
            outcome: None,
            at: known_at.saturating_sub(Duration::from_hours(24)),
            known_at,
        }
    }

    #[test]
    fn a_memory_with_nothing_knowable_produces_no_digest_and_says_what_it_holds() {
        // The refusal that keeps §6.2 row 3 fail-closed: a memory holding
        // episodes nobody may read yet must not be described as one that can
        // be recalled from, because a produced slot stops the cell pausing
        // situational-recognition strategies.
        let issue = EpisodicIssue::derive(std::iter::empty(), 3, at(1_000))
            .expect("an empty iterator is not an error");
        assert_eq!(issue.slot(), Slot::unproduced());
        assert_eq!(issue.digest(), None);
        assert_eq!(
            issue.outcome,
            EpisodicOutcome::NothingKnowable { held: 3 },
            "the operator line has to say memory holds three, not zero"
        );
        assert!(
            issue.describe().contains("holds 3 episode(s)"),
            "{}",
            issue.describe()
        );
    }

    #[test]
    fn a_produced_slot_is_stamped_with_the_newest_knowable_episode_and_not_the_issue_instant() {
        // The whole safety argument of this module. If the slot were stamped
        // `now`, a memory that stopped absorbing anything last week would
        // read fresh at every cell for as long as payloads kept being issued.
        let old = episode("ep-old", at(1_000));
        let newest = episode("ep-new", at(1_200));
        let issued_at = at(9_000);
        let issue = EpisodicIssue::derive([&old, &newest].into_iter(), 2, issued_at)
            .expect("both episodes are knowable");

        assert_eq!(issue.slot().produced_at(), Some(at(1_200)));
        assert_ne!(issue.slot().produced_at(), Some(issued_at));
        assert_eq!(issue.digest().map(|digest| digest.episodes), Some(2));

        // And what that costs the cell: 7,800 seconds past a 600-second time
        // to live is stale, so the row keeps pausing.
        assert_eq!(
            issue
                .slot()
                .freshness(PolicyItem::EpisodicDigest, issued_at),
            Freshness::Stale
        );
    }

    #[test]
    fn a_digest_names_the_episodes_and_changes_when_one_of_them_does() {
        // A digest that did not move when memory did would be a manifest of
        // nothing — the cell could not tell two memories apart, and the
        // reconciliation the manifest exists for would always agree.
        let first = episode("ep-a", at(1_000));
        let second = episode("ep-b", at(1_100));
        let one = EpisodicIssue::derive([&first].into_iter(), 1, at(2_000))
            .expect("one knowable episode");
        let two = EpisodicIssue::derive([&first, &second].into_iter(), 2, at(2_000))
            .expect("two knowable episodes");
        let one_digest = one.digest().expect("produced").digest.clone();
        let two_digest = two.digest().expect("produced").digest.clone();
        assert_ne!(one_digest, two_digest);

        // Premise, so the inequality above is about the contents and not
        // about the digest being empty or random: the same episodes offered
        // in the other order digest identically.
        let reordered = EpisodicIssue::derive([&second, &first].into_iter(), 2, at(2_000))
            .expect("two knowable episodes");
        assert_eq!(reordered.digest().expect("produced").digest, two_digest);
    }

    #[test]
    fn an_episode_knowable_at_or_after_the_issue_instant_is_refused() {
        // Unreachable through `EpisodicMemory::episodes`, which filters
        // strictly earlier. Refused rather than stamped, because a slot whose
        // age cannot be accounted for is one nobody can audit — and the
        // caller reaching past the store's own bitemporal filter is the bug
        // to name.
        let future = episode("ep-future", at(2_000));
        let error = EpisodicIssue::derive([&future].into_iter(), 1, at(2_000))
            .expect_err("knowable at the issue instant is not knowable yet");
        assert!(error.to_string().contains("strictly earlier"), "{error}");
    }

    #[test]
    fn a_fresh_digest_is_what_stops_a_cell_pausing_situational_recognition() {
        // What producing this slot actually changes at a cell, asserted
        // through the payload rather than described in a comment — and the
        // premise first, because a test that only asserted the fresh case
        // would pass on a payload that never pauses anything.
        let issued_at = at(10_000);
        let unproduced = PolicyPayload::unproduced(1, "cell-1", issued_at);
        assert!(
            unproduced
                .narrowing(issued_at)
                .pauses(StrategyClass::SituationalRecognition),
            "an unproduced slot must pause situational recognition, or this test proves nothing"
        );

        let recent = episode(
            "ep-recent",
            issued_at.saturating_sub(Duration::from_secs(60)),
        );
        let issue = EpisodicIssue::derive([&recent].into_iter(), 1, issued_at)
            .expect("one knowable episode a minute old");
        let mut payload = PolicyPayload::unproduced(2, "cell-1", issued_at);
        payload.episodic_digest = issue.slot();
        let narrowing = payload.narrowing(issued_at);
        assert_eq!(
            narrowing.freshness(Capability::EpisodicMemory),
            Freshness::Fresh
        );
        assert!(!narrowing.pauses(StrategyClass::SituationalRecognition));

        // Eleven minutes later the same payload's slot is past its ten-minute
        // time to live, and the pause returns without anything being
        // republished.
        let later = issued_at.saturating_add(Duration::from_secs(660));
        assert!(
            payload
                .narrowing(later)
                .pauses(StrategyClass::SituationalRecognition),
            "a digest older than its time to live must stop excusing the pause"
        );
    }
}
