//! Consumer groups: membership, partition assignment and generation-fenced
//! commits (ADR 0100 §1; FABRIC-7033, FABRIC-033).
//!
//! The broker already kept each group's committed offset per partition, in
//! its own store, so a consumer that restarts resumes from the checkpoint
//! (FABRIC-016, [`super::broker::Broker::commit_offset`]). What it did not
//! keep was *who* was in a group. `groups/join` assigned a caller every
//! partition its grant covered and answered with the broker's leader epoch
//! as the generation, so two members of one group both owned every partition,
//! both fetched it, and both committed it — the double read a group exists
//! to prevent, with nothing on either side to say it was happening.
//!
//! # What a member holds
//!
//! A member is a `(group, stream, member id)` with an owner — the identity
//! whose token joined it — a lease, and the partitions it was assigned. The
//! [`GroupCoordinator`] holds those in memory:
//!
//! * **Assignment is a function of membership.** Every change to the member
//!   set of a group — a join, a member whose lease ran out, a member whose
//!   grant scope changed — bumps the group's generation and recomputes the
//!   whole assignment, deterministically: each partition, in order, goes to
//!   the eligible member holding the fewest so far, ties broken by member id.
//!   Two members of a group are never assigned one partition.
//! * **A commit is fenced by generation.** `groups/commit` carries the member
//!   id and the generation it joined at. A commit from a generation that is no
//!   longer current, from a member that no longer exists, from an identity
//!   that does not own the member, or for a partition the member does not
//!   hold, is refused naming what to do — rejoin. An old member that was
//!   paused past its lease and wakes up therefore cannot move a checkpoint its
//!   successor now owns.
//! * **The generation includes the leader epoch.** The wire generation is the
//!   broker's leader epoch in the high 32 bits and the group's own counter in
//!   the low 32. Membership is not persisted — a restarted broker starts every
//!   group empty — and the epoch bump on restart is what makes every
//!   generation handed out by the previous incarnation stale, rather than a
//!   counter that restarted at one and happened to match.
//! * **A member id is owned by the identity that joined it.** A join that
//!   names no member id joins as the identity itself (static membership), so
//!   a restarted consumer resumes its own membership. A second
//!   identity naming the same member id is refused rather than allowed to
//!   commit under it; this is the group equivalent of the operator-name rule
//!   in [`super::service`].
//!
//! Membership is bounded: [`MAXIMUM_MEMBERS`] per group, and member ids are
//! bounded in length and alphabet, because each one is held in memory for as
//! long as its lease and appears in every refusal about it.
//!
//! # What this is not
//!
//! Not a rebalance protocol with revocation. A member whose partition moved
//! learns it on its next commit (refused: rejoin) or join (answered: the new
//! assignment); between the two it may still *fetch* the partition, because
//! a fetch is read-only and carries no member id. That is the same contract
//! Kafka's generation fencing gives — commits are fenced, reads are not —
//! and it is why a consumer that needs exactly-once processing keeps its own
//! offsets with its own effects, as `qip-ledgerd` does (ADR 0100 §4), and
//! treats the group checkpoint as the broker's view of its lag.

use std::collections::{BTreeMap, BTreeSet};

use qip_core::error::{Error, Result};

/// The most members one group may hold at once.
pub const MAXIMUM_MEMBERS: usize = 256;

/// The longest member id accepted.
pub const MAXIMUM_MEMBER_ID_LENGTH: usize = 128;

/// How long a member stays in its group without a join or a commit, when the
/// composition root does not choose: long enough for a consumer idle on an
/// empty partition to re-join well inside it, short enough that a dead
/// member's partitions move within a minute.
pub const DEFAULT_SESSION_TIMEOUT_NS: i64 = 30_000_000_000;

/// What a member was given on joining.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assignment {
    pub member_id: String,
    /// The wire generation: leader epoch high, group counter low.
    pub generation: u64,
    /// Ascending.
    pub partitions: Vec<u32>,
}

#[derive(Clone, Debug)]
struct Member {
    owner: String,
    eligible: BTreeSet<u32>,
    assigned: BTreeSet<u32>,
    last_seen_ns: i64,
}

#[derive(Clone, Debug, Default)]
struct Group {
    counter: u32,
    members: BTreeMap<String, Member>,
}

/// Every group's live membership on this broker. See the module
/// documentation.
#[derive(Clone, Debug)]
pub struct GroupCoordinator {
    leader_epoch: u64,
    session_timeout_ns: i64,
    groups: BTreeMap<(String, String), Group>,
}

fn check_member_id(member_id: &str) -> Result<()> {
    if member_id.trim().is_empty()
        || member_id.len() > MAXIMUM_MEMBER_ID_LENGTH
        || !member_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':' | '/' | '#'))
    {
        return Err(Error::invalid(format!(
            "a member id must be 1 to {MAXIMUM_MEMBER_ID_LENGTH} characters of ASCII letters, \
             digits and - _ . : / #; omit it to join as the identity itself"
        )));
    }
    Ok(())
}

impl GroupCoordinator {
    /// A coordinator for a broker at `leader_epoch`, expiring members not
    /// seen for `session_timeout_ns`. Refuses a non-positive timeout, which
    /// would expire every member at the instant it joined.
    pub fn new(leader_epoch: u64, session_timeout_ns: i64) -> Result<Self> {
        if session_timeout_ns <= 0 {
            return Err(Error::invalid(
                "a consumer-group session timeout must be positive; zero would expire every \
                 member as it joined",
            ));
        }
        if leader_epoch > u64::from(u32::MAX) {
            return Err(Error::numeric(format!(
                "leader epoch {leader_epoch} does not fit the high half of a group generation"
            )));
        }
        Ok(Self {
            leader_epoch,
            session_timeout_ns,
            groups: BTreeMap::new(),
        })
    }

    fn wire_generation(&self, counter: u32) -> u64 {
        (self.leader_epoch << 32) | u64::from(counter)
    }

    /// Join (or re-join, which renews the lease) `member_id` — or, when it
    /// is `None`, the member named after `identity` — to `group` on
    /// `stream`, eligible for
    /// `eligible` partitions (what its grant covers), and answer the
    /// assignment it now holds.
    pub fn join(
        &mut self,
        now_ns: i64,
        identity: &str,
        group: &str,
        stream: &str,
        member_id: Option<&str>,
        eligible: &[u32],
    ) -> Result<Assignment> {
        if group.trim().is_empty() {
            return Err(Error::invalid("a consumer group must have a non-empty id"));
        }
        let timeout = self.session_timeout_ns;
        let state = self
            .groups
            .entry((group.to_string(), stream.to_string()))
            .or_default();
        let mut changed = expire(state, now_ns, timeout);

        // Static membership: a join that names no member is the identity's
        // own member, so a consumer that restarts — or a second consumer of
        // the same process reading another partition — takes over the
        // membership it already had instead of joining beside its own
        // corpse and being assigned nothing until the corpse's lease ran
        // out. Several members under one identity name themselves.
        let member_id = member_id.unwrap_or(identity).to_string();
        check_member_id(&member_id)?;
        let eligible: BTreeSet<u32> = eligible.iter().copied().collect();
        match state.members.get_mut(&member_id) {
            Some(member) if member.owner != identity => {
                return Err(Error::denied(format!(
                    "member '{member_id}' of group '{group}' belongs to another identity; join \
                     under a member id of your own, or omit it to join as yourself"
                )));
            }
            Some(member) => {
                member.last_seen_ns = now_ns;
                if member.eligible != eligible {
                    member.eligible = eligible;
                    changed = true;
                }
            }
            None => {
                if state.members.len() >= MAXIMUM_MEMBERS {
                    return Err(Error::denied(format!(
                        "group '{group}' on '{stream}' already holds {MAXIMUM_MEMBERS} members; \
                         a member must leave or expire before another can join"
                    )));
                }
                state.members.insert(
                    member_id.clone(),
                    Member {
                        owner: identity.to_string(),
                        eligible,
                        assigned: BTreeSet::new(),
                        last_seen_ns: now_ns,
                    },
                );
                changed = true;
            }
        }
        if changed {
            state.counter = state.counter.checked_add(1).ok_or_else(|| {
                Error::numeric(format!(
                    "group '{group}' has changed membership {} times on this broker \
                     incarnation; restart the broker to begin a new leader epoch",
                    u32::MAX
                ))
            })?;
            assign(state);
        }
        let counter = state.counter;
        let partitions = state
            .members
            .get(&member_id)
            .map(|m| m.assigned.iter().copied().collect())
            .unwrap_or_default();
        Ok(Assignment {
            member_id,
            generation: self.wire_generation(counter),
            partitions,
        })
    }

    /// Admit a commit by `member_id` at `generation` for `partition`,
    /// renewing the member's lease; refuse it — naming the remedy — if the
    /// member, its owner, its generation or its assignment does not match.
    #[allow(clippy::too_many_arguments)]
    pub fn admit_commit(
        &mut self,
        now_ns: i64,
        identity: &str,
        group: &str,
        stream: &str,
        member_id: &str,
        generation: u64,
        partition: u32,
    ) -> Result<()> {
        let timeout = self.session_timeout_ns;
        let leader_epoch = self.leader_epoch;
        let Some(state) = self
            .groups
            .get_mut(&(group.to_string(), stream.to_string()))
        else {
            return Err(Error::denied(format!(
                "group '{group}' has no members on '{stream}' at this broker; join before \
                 committing"
            )));
        };
        if expire(state, now_ns, timeout) {
            state.counter = state.counter.checked_add(1).ok_or_else(|| {
                Error::numeric(format!("group '{group}' generation counter is exhausted"))
            })?;
            assign(state);
        }
        let current = (leader_epoch << 32) | u64::from(state.counter);
        let Some(member) = state.members.get_mut(member_id) else {
            return Err(Error::denied(format!(
                "member '{member_id}' is not in group '{group}' (its lease ran out, or the \
                 broker restarted); rejoin, then commit"
            )));
        };
        if member.owner != identity {
            return Err(Error::denied(format!(
                "member '{member_id}' of group '{group}' belongs to another identity; a commit \
                 is accepted only from the identity that joined the member"
            )));
        }
        if generation != current {
            return Err(Error::denied(format!(
                "member '{member_id}' committed at generation {generation} but group '{group}' \
                 is at {current}; its membership changed, so rejoin to learn the partitions it \
                 now holds"
            )));
        }
        if !member.assigned.contains(&partition) {
            return Err(Error::denied(format!(
                "member '{member_id}' of group '{group}' is not assigned partition {partition} \
                 of '{stream}' at generation {current}; another member owns it"
            )));
        }
        member.last_seen_ns = now_ns;
        Ok(())
    }

    /// The members of `group` on `stream` and what each holds, for an
    /// operator and for tests.
    pub fn members(&self, group: &str, stream: &str) -> BTreeMap<String, Vec<u32>> {
        self.groups
            .get(&(group.to_string(), stream.to_string()))
            .map(|g| {
                g.members
                    .iter()
                    .map(|(id, m)| (id.clone(), m.assigned.iter().copied().collect()))
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Drop every member not seen within the session timeout; whether any was.
fn expire(state: &mut Group, now_ns: i64, timeout: i64) -> bool {
    let before = state.members.len();
    state
        .members
        .retain(|_, m| now_ns.saturating_sub(m.last_seen_ns) <= timeout);
    state.members.len() != before
}

/// Recompute every member's assignment: each partition any member is
/// eligible for, ascending, goes to the eligible member holding fewest so
/// far, ties to the smaller member id.
fn assign(state: &mut Group) {
    let partitions: BTreeSet<u32> = state
        .members
        .values()
        .flat_map(|m| m.eligible.iter().copied())
        .collect();
    for member in state.members.values_mut() {
        member.assigned.clear();
    }
    for partition in partitions {
        let chosen = state
            .members
            .iter()
            .filter(|(_, m)| m.eligible.contains(&partition))
            .min_by_key(|(id, m)| (m.assigned.len(), (*id).clone()))
            .map(|(id, _)| id.clone());
        if let Some(id) = chosen
            && let Some(member) = state.members.get_mut(&id)
        {
            member.assigned.insert(partition);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: i64 = 1_000_000_000;

    fn coordinator() -> GroupCoordinator {
        GroupCoordinator::new(3, 10 * SECOND).expect("valid")
    }

    /// FABRIC-7033 / FABRIC-033: two members of one group split the
    /// partitions and never share one. Before this module both were
    /// assigned every partition. Mutation: in `assign`, insert the partition
    /// into every eligible member instead of the chosen one — the overlap
    /// assertion fails.
    #[test]
    fn two_members_of_one_group_split_the_partitions_and_never_share_one() {
        let mut c = coordinator();
        let all = [0, 1, 2, 3];
        let first = c
            .join(0, "ledger", "g", "outcomes", Some("a"), &all)
            .expect("joins");
        // Premise: alone, the first member holds everything.
        assert_eq!(first.partitions, vec![0, 1, 2, 3]);
        let second = c
            .join(SECOND, "ledger", "g", "outcomes", Some("b"), &all)
            .expect("joins");
        assert!(second.generation > first.generation, "membership changed");
        let first = c
            .join(SECOND, "ledger", "g", "outcomes", Some("a"), &all)
            .expect("rejoins");
        assert_eq!(
            first.generation, second.generation,
            "a renewal changes nothing"
        );
        assert_eq!(first.partitions.len(), 2);
        assert_eq!(second.partitions.len(), 2);
        let a: BTreeSet<u32> = first.partitions.iter().copied().collect();
        let b: BTreeSet<u32> = second.partitions.iter().copied().collect();
        assert!(a.is_disjoint(&b), "{a:?} and {b:?} overlap");
        assert_eq!(a.union(&b).count(), 4);
    }

    /// A commit from a stale generation, a foreign identity, or for a
    /// partition the member no longer holds is refused, and the current one
    /// is admitted. Mutation: delete the `generation != current` check — the
    /// stale commit is admitted and this fails.
    #[test]
    fn a_commit_from_a_superseded_generation_or_a_foreign_identity_is_refused() {
        let mut c = coordinator();
        let all = [0, 1];
        let old = c
            .join(0, "ledger", "g", "s", Some("a"), &all)
            .expect("joins");
        c.admit_commit(0, "ledger", "g", "s", "a", old.generation, 1)
            .expect("premise: the sole member commits partition 1");
        let _b = c
            .join(SECOND, "ledger", "g", "s", Some("b"), &all)
            .expect("joins");
        let stale = c
            .admit_commit(SECOND, "ledger", "g", "s", "a", old.generation, 0)
            .expect_err("the generation moved");
        assert!(stale.to_string().contains("rejoin"), "{stale}");
        let now = c
            .join(SECOND, "ledger", "g", "s", Some("a"), &all)
            .expect("rejoins");
        let lost = if now.partitions.contains(&0) { 1 } else { 0 };
        c.admit_commit(
            SECOND,
            "ledger",
            "g",
            "s",
            "a",
            now.generation,
            now.partitions[0],
        )
        .expect("the current generation commits its own partition");
        assert!(
            c.admit_commit(SECOND, "ledger", "g", "s", "a", now.generation, lost)
                .is_err(),
            "a partition the member does not hold is refused"
        );
        let foreign = c
            .join(SECOND, "intruder", "g", "s", Some("a"), &all)
            .expect_err("another identity cannot take over member a");
        assert!(
            foreign.to_string().contains("another identity"),
            "{foreign}"
        );
    }

    /// A member that stops renewing is expired and its partitions move; the
    /// old member's commit is refused. Mutation: make `expire` retain every
    /// member — the survivor never takes partition 1 and this fails.
    #[test]
    fn a_member_whose_lease_runs_out_loses_its_partitions_to_the_survivor() {
        let mut c = coordinator();
        let all = [0, 1];
        let a = c
            .join(0, "ledger", "g", "s", Some("a"), &all)
            .expect("joins");
        let b = c
            .join(0, "ledger", "g", "s", Some("b"), &all)
            .expect("joins");
        assert_eq!(a.generation, b.generation - 1);
        // Premise: they share the work before the lease runs out.
        assert_eq!(c.members("g", "s").get("a").map(Vec::len), Some(1));
        let later = 11 * SECOND;
        let survivor = c
            .join(later, "ledger", "g", "s", Some("b"), &all)
            .expect("renews");
        assert_eq!(survivor.partitions, vec![0, 1]);
        assert!(
            c.admit_commit(later, "ledger", "g", "s", "a", b.generation, 0)
                .is_err()
        );
    }

    /// A consumer that restarts without naming a member id resumes its own
    /// membership rather than joining beside its expired self and being
    /// assigned nothing. Mutation: mint a fresh id per unnamed join — the
    /// restarted consumer is a second member and holds one partition, not
    /// both, and this fails.
    #[test]
    fn an_unnamed_join_is_the_identity_itself_so_a_restart_keeps_its_partitions() {
        let mut c = coordinator();
        let first = c.join(0, "ledger", "g", "s", None, &[0, 1]).expect("joins");
        assert_eq!(first.member_id, "ledger");
        let again = c
            .join(SECOND, "ledger", "g", "s", None, &[0, 1])
            .expect("rejoins");
        assert_eq!(again.partitions, vec![0, 1]);
        assert_eq!(again.generation, first.generation);
        assert_eq!(c.members("g", "s").len(), 1);
    }

    /// The broker restarted: every generation the old incarnation handed out
    /// is stale even though the group counter restarts. Mutation: drop the
    /// leader epoch from `wire_generation` — the two generations are equal
    /// and this fails.
    #[test]
    fn a_generation_from_a_previous_broker_incarnation_is_never_current() {
        let mut before = GroupCoordinator::new(3, 10 * SECOND).expect("valid");
        let mut after = GroupCoordinator::new(4, 10 * SECOND).expect("valid");
        let old = before
            .join(0, "ledger", "g", "s", Some("a"), &[0])
            .expect("joins");
        let new = after
            .join(0, "ledger", "g", "s", Some("a"), &[0])
            .expect("joins");
        assert_ne!(old.generation, new.generation);
        assert!(
            after
                .admit_commit(0, "ledger", "g", "s", "a", old.generation, 0)
                .is_err()
        );
    }
}
