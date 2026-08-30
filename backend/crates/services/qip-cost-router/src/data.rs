//! What the inputs cost, per decision that read them.
//!
//! A data licence is billed as a subscription and consumed as reads, and those
//! two facts are what make a per-decision data cost non-obvious. A source read
//! once a day and one read a million times cost exactly the same to licence and
//! are wildly different per decision: the first carries the whole subscription,
//! the second carries a millionth of it. Charging every decision the same flat
//! figure — or, worse, charging none of them and letting the subscription sit
//! in an infrastructure budget — is how a strategy that only works because its
//! data is free passes review.
//!
//! Read volume here is **observed**, not forecast. A model that amortises over
//! the reads a source is expected to get makes every new source look cheap on
//! the strength of a plan, and the plan is the thing most likely to be wrong.

use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_core::time::Duration;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One licensed source, its price, and how hard it was actually used.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DataSource {
    pub name: String,
    /// What the licence cost for `period`. Exact: it is an invoice.
    pub subscription_cost: Decimal,
    /// The period the subscription bought.
    pub period: Duration,
    /// Reads observed over that period.
    pub reads_in_period: u64,
}

impl DataSource {
    pub fn new(
        name: impl Into<String>,
        subscription_cost: Decimal,
        period: Duration,
        reads_in_period: u64,
    ) -> Result<Self> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(Error::invalid(
                "a data source with no name cannot be billed",
            ));
        }
        if subscription_cost < Decimal::ZERO {
            return Err(Error::invalid(format!(
                "the {name} licence cannot cost a negative amount"
            )));
        }
        if period.as_nanos() <= 0 {
            return Err(Error::invalid(format!(
                "the {name} licence covers no time, so nothing can be amortised over it"
            )));
        }
        Ok(Self {
            name,
            subscription_cost,
            period,
            reads_in_period,
        })
    }

    /// The amortised cost of one read. Exact.
    ///
    /// A source that was never read is an error rather than a zero. Its
    /// per-read cost is undefined, and answering zero would say the licence
    /// was free — which is the opposite of what an unread subscription is.
    pub fn cost_per_read(&self) -> Result<Decimal> {
        if self.reads_in_period == 0 {
            return Err(Error::invalid(format!(
                "{} was not read at all over the period; its per-read cost is undefined and the whole subscription is unamortised",
                self.name
            )));
        }
        let reads = Decimal::from_scaled(i128::from(self.reads_in_period), 0).ok_or_else(|| {
            Error::numeric(format!(
                "{} recorded {} reads, which does not fit an exact decimal",
                self.name, self.reads_in_period
            ))
        })?;
        self.subscription_cost.checked_div(reads).ok_or_else(|| {
            Error::numeric(format!(
                "dividing the {} licence by its reads overflowed",
                self.name
            ))
        })
    }
}

/// What one decision read, by source.
///
/// Counted rather than listed: a decision that consults the same source forty
/// times has amortised forty reads out of the subscription, and charging it for
/// one would leave thirty-nine paid for by decisions that did not make them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DataReads {
    reads: BTreeMap<String, u64>,
}

impl DataReads {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record `count` reads of a source. Repeats accumulate.
    pub fn record(mut self, source: impl Into<String>, count: u64) -> Self {
        let entry = self.reads.entry(source.into()).or_insert(0);
        *entry = entry.saturating_add(count);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.reads.is_empty()
    }

    pub fn sources(&self) -> usize {
        self.reads.len()
    }

    pub fn total_reads(&self) -> u64 {
        self.reads
            .values()
            .fold(0u64, |sum, n| sum.saturating_add(*n))
    }

    /// Every source and its read count, in a stable order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, u64)> {
        self.reads
            .iter()
            .map(|(name, count)| (name.as_str(), *count))
    }
}

/// The licensed sources the platform pays for, and what a read of each costs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DataCostModel {
    sources: BTreeMap<String, DataSource>,
}

impl DataCostModel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_source(mut self, source: DataSource) -> Self {
        self.sources.insert(source.name.clone(), source);
        self
    }

    pub fn len(&self) -> usize {
        self.sources.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    pub fn get(&self, name: &str) -> Option<&DataSource> {
        self.sources.get(name)
    }

    /// The amortised cost of one read of a named source.
    ///
    /// An unknown source is refused rather than charged nothing. A decision
    /// that read something the platform has no licence record for is either
    /// reading unlicensed data or has a stale source name, and both are worth
    /// failing over.
    pub fn cost_per_read(&self, source: &str) -> Result<Decimal> {
        self.sources
            .get(source)
            .ok_or_else(|| {
                Error::not_found(format!(
                    "no licence is recorded for '{source}', so the cost of reading it is unknown, not zero"
                ))
            })?
            .cost_per_read()
    }

    /// What one decision's reads cost, and the sentence that explains it.
    pub fn charge(&self, reads: &DataReads) -> Result<(Decimal, String)> {
        if reads.is_empty() {
            return Ok((
                Decimal::ZERO,
                "the decision read no licensed source".to_string(),
            ));
        }
        let mut total = Decimal::ZERO;
        let mut parts: Vec<String> = Vec::new();
        for (source, count) in reads.iter() {
            let per_read = self.cost_per_read(source)?;
            let times = Decimal::from_scaled(i128::from(count), 0).ok_or_else(|| {
                Error::numeric(format!(
                    "{count} reads of {source} does not fit an exact decimal"
                ))
            })?;
            let charged = per_read.checked_mul(times).ok_or_else(|| {
                Error::numeric(format!("charging {count} reads of {source} overflowed"))
            })?;
            total = total
                .checked_add(charged)
                .ok_or_else(|| Error::numeric("the data cost of this decision overflowed"))?;
            parts.push(format!("{count}×{source} at {per_read}/read"));
        }
        Ok((total, parts.join(", ")))
    }
}
