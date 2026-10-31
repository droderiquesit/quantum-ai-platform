//! Custody's freshness, read from the venue's own custody statements (ADR
//! 0242, R12-RES-7099).
//!
//! The §6.2 table priced a custody row and nothing read custody, so a cell
//! that could not confirm the venue still held its positions sized as if it
//! could. The reading here is the only one the ruling admits: the age of the
//! newest statement the venue itself sent, in statement periods. No operator
//! route sets it and no transfer gate feeds it.
//!
//! Pure and clock-free, like [`crate::pressure`]: the node records each
//! statement as it arrives and asks for the freshness at the pass instant,
//! then hands the cell the answer through `Cell::observe_custody`.

use qip_contracts::degradation::Freshness;
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};

/// Statement periods past which custody reads stale: one missed statement is
/// a late message, two is a pattern.
pub const STALE_AFTER_PERIODS: i64 = 2;

/// Statement periods past which custody reads unavailable: the custody being
/// read is older than any position opened since.
pub const UNAVAILABLE_AFTER_PERIODS: i64 = 4;

/// One custody statement as the venue sent it: which venue, and the instant
/// it states custody as of.
///
/// Defined here because R12-LEDGER-10185's custody statement has not landed;
/// when it does, the simulated venue's statement is recorded through
/// [`CustodyWatch::record`] and nothing else here changes (ADR 0242
/// decision 5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustodyStatement {
    pub venue: VenueId,
    pub as_of: Timestamp,
}

/// The newest custody statement a node has seen, judged against the period
/// the venue states at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustodyWatch {
    period: Duration,
    newest: Option<Timestamp>,
}

impl CustodyWatch {
    /// A watch for statements every `period`. Refused unless the period is
    /// positive: a zero period would read every statement as late forever.
    pub fn new(period: Duration) -> Result<Self> {
        if period <= Duration::ZERO {
            return Err(Error::invalid(
                "a custody statement period must be positive; state how often the venue states \
                 custody",
            ));
        }
        Ok(Self {
            period,
            newest: None,
        })
    }

    /// Record a statement. Only a newer one moves the reading: a statement
    /// arriving late, out of order, says nothing newer than what is held.
    pub fn record(&mut self, statement: &CustodyStatement) {
        if self.newest.is_none_or(|held| statement.as_of > held) {
            self.newest = Some(statement.as_of);
        }
    }

    /// Custody's freshness at `now`: unavailable with no statement since the
    /// watch was made (ADR 0242 decision 2), else by the newest statement's
    /// age — stale past [`STALE_AFTER_PERIODS`] periods, unavailable past
    /// [`UNAVAILABLE_AFTER_PERIODS`].
    pub fn freshness(&self, now: Timestamp) -> Freshness {
        let Some(newest) = self.newest else {
            return Freshness::Unavailable;
        };
        let age = now.since(newest).as_nanos();
        let period = self.period.as_nanos();
        if age > period.saturating_mul(UNAVAILABLE_AFTER_PERIODS) {
            Freshness::Unavailable
        } else if age > period.saturating_mul(STALE_AFTER_PERIODS) {
            Freshness::Stale
        } else {
            Freshness::Fresh
        }
    }
}
