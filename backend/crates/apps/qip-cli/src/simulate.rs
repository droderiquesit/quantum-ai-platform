//! `qip simulate --set <path> [--shard k/n]` — replay a recorded tick set
//! with the platform's decisions executed against it, one shard at a time
//! (R12-TICK-9036).
//!
//! The market digital twin is [`qip_simulation_engine::tick_replay`]: a
//! rebuilt book per instrument, queue position, latency, fees and impact.
//! Until this command it was reached only from tests, so "run as sharded
//! replay jobs" described nothing an operator could start. This is the
//! entry point a batch job — a Cloud Run job, a shell loop, or `n`
//! terminals — runs once per shard.
//!
//! # Why the shard key is the instrument, hashed with SHA-256
//!
//! A replay session keys every book and every sequence stream by
//! instrument (`{stream}#{instrument}` in `ReplaySession::step`), so
//! instruments are independent of one another and the instrument is the
//! widest unit a set can be cut along without changing any fill. The
//! bucket is the SHA-256 of the instrument id, not `DefaultHasher`, whose
//! output is randomised per process: with it, two shard processes would
//! disagree about where an instrument lives, one instrument would replay
//! twice and another never, and the union would look complete because
//! every process exited zero.
//!
//! # What it refuses
//!
//! Two inputs couple instruments, and splitting either would change the
//! answer rather than parallelise it. Both are refused for `n > 1`, naming
//! the input, rather than replayed approximately:
//!
//! - **a venue message budget** — the throttle window counts every order
//!   the venue admitted, across instruments, so a shard would see a
//!   quieter venue than history did and credit fills the venue refused;
//! - **a multi-leg plan whose legs land in two shards** — the plan's later
//!   legs, its decay window and its unwinds all read books another
//!   process holds.
//!
//! A malformed `k/n` (zero, `k > n`, not two integers) is refused rather
//! than read as the whole set: a job told `--shard 0/4` that replayed
//! everything would quietly do four times the work and double-count.
//!
//! Paper only: a replay is a simulated outcome over recorded history, and
//! nothing here opens a socket.

use qip_core::error::{Error, Result};
use qip_core::hash::sha256;
use qip_simulation_engine::tick_replay::{
    ExecutionModel, MultiLegPlan, ReplayOrder, ReplayRun, ReplaySession, ReplayTick,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;

/// The most shards a set may be cut into. A bound, so a typo of
/// `--shard 1/40000` is refused rather than producing tens of thousands of
/// empty jobs each reporting success.
pub const MAX_SHARDS: u32 = 4096;

/// A recorded tick set and the decisions to replay against it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReplaySet {
    pub model: ExecutionModel,
    pub ticks: Vec<ReplayTick>,
    #[serde(default)]
    pub orders: Vec<ReplayOrder>,
    #[serde(default)]
    pub plans: Vec<MultiLegPlan>,
}

impl ReplaySet {
    /// Read a set from a JSON file, refusing a missing or malformed one by
    /// name rather than replaying an empty history.
    pub fn read(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|error| {
            Error::invalid(format!(
                "the replay set at {} could not be read: {error}",
                path.display()
            ))
        })?;
        serde_json::from_str(&text).map_err(|error| {
            Error::schema(format!(
                "the replay set at {} is not a replay set (model, ticks, orders, plans): {error}",
                path.display()
            ))
        })
    }
}

/// One shard of `count`, numbered from 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Shard {
    index: u32,
    count: u32,
}

impl Shard {
    /// The whole set, as one shard.
    pub const WHOLE: Self = Self { index: 1, count: 1 };

    /// Shard `index` of `count`, refusing zero, `index > count` and a count
    /// past [`MAX_SHARDS`].
    pub fn new(index: u32, count: u32) -> Result<Self> {
        if count == 0 || count > MAX_SHARDS {
            return Err(Error::invalid(format!(
                "a replay is cut into between 1 and {MAX_SHARDS} shards, not {count}"
            )));
        }
        if index == 0 || index > count {
            return Err(Error::invalid(format!(
                "shard {index}/{count} does not exist; shards are numbered 1 to {count}"
            )));
        }
        Ok(Self { index, count })
    }

    /// Parse `k/n`.
    pub fn parse(text: &str) -> Result<Self> {
        let refuse = || {
            Error::invalid(format!(
                "--shard takes k/n, two positive integers with k no greater than n, not {text:?}"
            ))
        };
        let (index, count) = text.split_once('/').ok_or_else(refuse)?;
        let index = index.parse::<u32>().map_err(|_| refuse())?;
        let count = count.parse::<u32>().map_err(|_| refuse())?;
        Self::new(index, count)
    }

    pub const fn index(self) -> u32 {
        self.index
    }

    pub const fn count(self) -> u32 {
        self.count
    }

    /// Whether `instrument` belongs to this shard.
    pub fn holds(self, instrument: &str) -> bool {
        bucket(instrument, self.count) == self.index
    }
}

/// The shard, numbered from 1, that `instrument` belongs to out of `count`.
///
/// The first eight bytes of the SHA-256 of the id, big-endian, modulo the
/// count: identical in every process and on every host, which is the whole
/// of what a shard key must be. Zero — no shard — for a count of zero,
/// which [`Shard::new`] never lets through.
fn bucket(instrument: &str, count: u32) -> u32 {
    let digest = sha256(instrument.as_bytes());
    let mut head = [0u8; 8];
    head.copy_from_slice(&digest[..8]);
    u64::from_be_bytes(head)
        .checked_rem(u64::from(count))
        .and_then(|remainder| u32::try_from(remainder).ok())
        .map_or(0, |remainder| remainder + 1)
}

/// The part of `set` that `shard` replays: the ticks and orders on its
/// instruments, and the plans whose every leg is on them.
pub fn split(set: &ReplaySet, shard: Shard) -> Result<ReplaySet> {
    if shard.count > 1 {
        if set.model.message_budget.is_some() {
            return Err(Error::invalid(
                "this replay set models a venue message budget, which counts orders across every \
                 instrument; a shard would replay a quieter venue than history. Replay it whole \
                 (--shard 1/1) or remove the budget",
            ));
        }
        for plan in &set.plans {
            let shards: BTreeSet<u32> = plan
                .legs
                .iter()
                .map(|leg| bucket(&leg.instrument, shard.count))
                .collect();
            if shards.len() > 1 {
                return Err(Error::invalid(format!(
                    "plan {} has legs in shards {:?} of {}; its later legs read books another \
                     shard holds. Replay it with fewer shards or whole",
                    plan.id, shards, shard.count
                )));
            }
        }
    }
    Ok(ReplaySet {
        model: set.model,
        ticks: set
            .ticks
            .iter()
            .filter(|tick| shard.holds(tick.message.object_id.as_str()))
            .cloned()
            .collect(),
        orders: set
            .orders
            .iter()
            .filter(|order| shard.holds(&order.instrument))
            .cloned()
            .collect(),
        plans: set
            .plans
            .iter()
            .filter(|plan| plan.legs.iter().all(|leg| shard.holds(&leg.instrument)))
            .cloned()
            .collect(),
    })
}

/// What one shard's replay produced, with the instruments it covered.
#[derive(Clone, Debug, PartialEq)]
pub struct ShardRun {
    pub shard: Shard,
    pub instruments: BTreeSet<String>,
    pub run: ReplayRun,
}

/// Replay `shard` of `set` through a [`ReplaySession`] to the end.
pub fn run(set: &ReplaySet, shard: Shard) -> Result<ShardRun> {
    let part = split(set, shard)?;
    let mut instruments: BTreeSet<String> = part
        .ticks
        .iter()
        .map(|tick| tick.message.object_id.as_str().to_string())
        .collect();
    instruments.extend(part.orders.iter().map(|order| order.instrument.clone()));
    instruments.extend(
        part.plans
            .iter()
            .flat_map(|plan| plan.legs.iter().map(|leg| leg.instrument.clone())),
    );
    let run = ReplaySession::new(part.model, part.ticks, part.orders, part.plans)?.run_to_end()?;
    Ok(ShardRun {
        shard,
        instruments,
        run,
    })
}

/// The lines `qip simulate` prints for one shard.
pub fn lines(shard_run: &ShardRun) -> Vec<String> {
    let run = &shard_run.run;
    let mut out = vec![
        format!(
            "shard:       {}/{}",
            shard_run.shard.index, shard_run.shard.count
        ),
        format!(
            "instruments: {} ({})",
            shard_run.instruments.len(),
            shard_run
                .instruments
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ),
        format!("steps:       {}", run.steps),
        format!(
            "unreliable:  {} span(s), {} step(s) replayed inside one",
            run.spans.len(),
            run.unreliable_steps.len()
        ),
        format!("input:       {}", run.input_digest),
        format!("digest:      {}", run.digest),
    ];
    for execution in &run.executions {
        out.push(format!(
            "execution {} {} {:?} requested {} filled {} status {:?}",
            execution.order_id,
            execution.instrument,
            execution.side,
            execution.requested,
            execution.filled,
            execution.status
        ));
    }
    for plan in &run.plans {
        out.push(format!(
            "plan {} completed {} failed_leg {:?} unwind_pnl {}",
            plan.plan_id, plan.completed, plan.failed_leg, plan.unwind_pnl
        ));
    }
    out.push("simulated over recorded history; paper only, nothing was sent".to_string());
    out
}
