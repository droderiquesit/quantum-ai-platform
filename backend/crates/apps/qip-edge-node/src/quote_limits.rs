//! Each venue's own message limits, as the deployment states them (EXEC-004).
//!
//! The failure this closes was a control no deployment could configure. The
//! cell has always held a message budget, and this binary never set it, so
//! every node ran `RateLimits::default()` — a ceiling whose own documentation
//! says it is not a claim about any venue — applied identically to every
//! venue it held, with a message-to-trade ratio that narrowed quoting and
//! never refused. A venue's real rate and ratio were therefore discovered when
//! the venue enforced them: a throttle or a disconnect at a moment nobody
//! chose, with resting orders the cell could no longer withdraw.
//!
//! A venue named here is held to both of its limits, and both refuse before
//! the gateway is called: the rate through the token bucket, and the ratio
//! through `RateLimits::refusing_at_ratio`. A venue not named here keeps the
//! cell's fallback ceiling and the monitor, and the node says so in its
//! production requirements rather than leaving the reader to assume a venue's
//! figure was ever stated.

use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::time::Duration;
use qip_edge::cell::CellConfig;
use qip_edge::quoting::RateLimits;
use std::collections::BTreeMap;

/// Each venue's message limits, as the deployment declares them:
/// `<venue>=<burst>:<per second>:<withdrawal reserve>:<narrowed reserve>:<messages per trade>:<window>:<ratio interval ms>`,
/// comma-separated.
pub const QUOTE_LIMITS_VARIABLE: &str = "QIP_VENUE_QUOTE_LIMITS";

/// The limits a deployment stated, per venue. Only [`VenueQuoteLimits::read`]
/// builds one, so a value of this type has already refused a venue the cell
/// may not trade and a set of numbers `RateLimits` would not hold.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VenueQuoteLimits {
    /// A `BTreeMap` because the banner lists it and the order reaches the
    /// start-up output an operator compares between two nodes.
    stated: BTreeMap<VenueId, RateLimits>,
}

impl VenueQuoteLimits {
    /// Read the declaration against the venues this cell was configured for.
    ///
    /// Unset or blank states nothing: every venue keeps the fallback, and
    /// that is announced. Anything else is either every entry valid or a
    /// refusal naming the entry and the form — a node that came up on half a
    /// declaration would hold one venue to its limit and silently not the
    /// other.
    pub fn read(value: Option<&str>, venues: &[VenueId]) -> Result<Self> {
        let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok(Self::default());
        };
        let mut stated = BTreeMap::new();
        for entry in value
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
        {
            let form = || {
                Error::invalid(format!(
                    "configuration: {QUOTE_LIMITS_VARIABLE} entry `{entry}` is not \
                     `<venue>=<burst>:<per second>:<withdrawal reserve>:<narrowed reserve>:\
                     <messages per trade>:<window>:<ratio interval ms>`; write \
                     `XLON=4096:2048:512:2048:64:512:1000` for a burst of 4096 refilling at 2048 \
                     a second, 512 kept for cancels, and 64 messages per trade refused within \
                     each second (512 is the window the narrowing monitor judges)"
                ))
            };
            let (name, numbers) = entry.split_once('=').ok_or_else(form)?;
            let venue = VenueId::new(name.trim());
            if !venues.contains(&venue) {
                return Err(Error::invalid(format!(
                    "configuration: {QUOTE_LIMITS_VARIABLE} states limits for {}, which is not a \
                     venue in QIP_VENUES; the limit would bind nothing while reading as one, so \
                     name the venue in QIP_VENUES or remove the entry",
                    venue.as_str()
                )));
            }
            let parsed = numbers
                .split(':')
                .map(|number| number.trim().parse::<u32>())
                .collect::<std::result::Result<Vec<u32>, _>>()
                .map_err(|_| form())?;
            let [
                burst,
                rate,
                reserve,
                narrowed,
                per_trade,
                window,
                interval_ms,
            ] = parsed[..]
            else {
                return Err(form());
            };
            let limits = RateLimits::new(burst, rate, reserve, narrowed, per_trade, window)
                .and_then(|limits| {
                    limits.refusing_at_ratio(Duration::from_millis(i64::from(interval_ms)))
                })
                .map_err(|error| {
                    Error::invalid(format!(
                        "configuration: {QUOTE_LIMITS_VARIABLE} entry `{entry}` is refused: {}",
                        error.message()
                    ))
                })?;
            if stated.insert(venue.clone(), limits).is_some() {
                return Err(Error::invalid(format!(
                    "configuration: {QUOTE_LIMITS_VARIABLE} states limits for {} twice; one of \
                     the two would be silently dropped, so state each venue once",
                    venue.as_str()
                )));
            }
        }
        Ok(Self { stated })
    }

    /// Hand the stated limits to the cell's configuration.
    ///
    /// The one place the two meet, in the library rather than in `main.rs`,
    /// so a test can drive a node whose limits came through this door.
    #[must_use]
    pub fn apply(&self, mut config: CellConfig) -> CellConfig {
        for (venue, limits) in &self.stated {
            config = config.with_venue_quote_limits(venue.clone(), *limits);
        }
        config
    }

    /// One line per venue with stated limits, for the start-up output.
    pub fn banner_lines(&self) -> Vec<String> {
        self.stated
            .iter()
            .map(|(venue, limits)| {
                format!(
                    "qip-edge-node: quote limits at {}: burst {} refilling at {} a second, {} \
                     kept for withdrawals, {} message(s) per trade refused within each {} ms",
                    venue.as_str(),
                    limits.capacity(),
                    limits.refill_per_second(),
                    limits.withdrawal_reserve(),
                    limits.messages_per_trade_bound(),
                    limits.ratio_interval().map_or(0, Duration::as_millis)
                )
            })
            .collect()
    }

    /// The venues of `venues` nobody stated limits for, in the order given.
    pub fn unstated<'a>(&self, venues: &'a [VenueId]) -> Vec<&'a VenueId> {
        venues
            .iter()
            .filter(|venue| !self.stated.contains_key(venue))
            .collect()
    }
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;

    fn venues() -> Vec<VenueId> {
        vec![VenueId::new("XLON"), VenueId::new("XPAR")]
    }

    #[test]
    fn an_unset_declaration_states_nothing_and_names_every_venue_as_unstated() -> Result<()> {
        let venues = venues();
        for absent in [None, Some(""), Some("   ")] {
            let limits = VenueQuoteLimits::read(absent, &venues)?;
            assert!(limits.banner_lines().is_empty());
            assert_eq!(
                limits.unstated(&venues).len(),
                2,
                "a venue nobody stated a limit for was not reported as running the fallback"
            );
            assert!(
                limits
                    .apply(CellConfig::new("cell", "region"))
                    .venue_quote_limits
                    .is_empty()
            );
        }
        Ok(())
    }

    #[test]
    fn a_stated_venue_is_held_to_its_own_rate_and_to_a_ratio_that_refuses() -> Result<()> {
        let venues = venues();
        let limits = VenueQuoteLimits::read(Some(" XPAR = 8:4:2:3:5:16:250 "), &venues)?;
        let config = limits.apply(CellConfig::new("cell", "region"));
        let stated = config
            .venue_quote_limits
            .get(&VenueId::new("XPAR"))
            .copied()
            .expect("the stated venue reached the cell configuration");
        assert_eq!(stated.capacity(), 8);
        assert_eq!(stated.refill_per_second(), 4);
        assert_eq!(stated.withdrawal_reserve(), 2);
        assert_eq!(stated.narrowed_reserve(), 3);
        assert_eq!(stated.messages_per_trade_bound(), 5);
        assert_eq!(stated.monitor_window(), 16);
        assert_eq!(
            stated.ratio_interval(),
            Some(Duration::from_millis(250)),
            "a stated venue's ratio only narrows, so the deployment's figure refuses nothing"
        );
        assert_eq!(
            limits.unstated(&venues),
            vec![&VenueId::new("XLON")],
            "the venue with no stated limits was not the one reported as on the fallback"
        );
        assert_eq!(limits.banner_lines().len(), 1);
        Ok(())
    }

    #[test]
    fn a_declaration_that_would_bind_nothing_or_half_of_what_was_written_is_refused() {
        let venues = venues();
        let refusal = |value: &str| {
            VenueQuoteLimits::read(Some(value), &venues)
                .expect_err("the declaration was accepted")
                .message()
                .to_string()
        };
        // Each refusal is matched on what it names, because a bare `is_err`
        // passes when the entry was refused for some other reason.
        assert!(
            refusal("XNYS=8:4:2:3:5:16:250").contains("not a venue in QIP_VENUES"),
            "limits for a venue the cell may not trade were not refused by name"
        );
        assert!(
            refusal("XLON=8:4:2:3:5:16").contains("is not `<venue>="),
            "six numbers were read as seven"
        );
        assert!(
            refusal("XLON=8:4:2:3:5:16:250:9").contains("is not `<venue>="),
            "eight numbers were read as seven"
        );
        assert!(
            refusal("XLON 8:4:2:3:5:16:250").contains("is not `<venue>="),
            "an entry with no venue was accepted"
        );
        assert!(
            refusal("XLON=8:4:2:3:five:16:250").contains("is not `<venue>="),
            "a word was read as a number"
        );
        assert!(
            refusal("XLON=8:4:2:3:5:16:250,XLON=9:4:2:3:5:16:250").contains("twice"),
            "a venue stated twice kept one figure and dropped the other"
        );
        assert!(
            refusal("XLON=0:4:0:0:5:16:250").contains("capacity of zero"),
            "a budget the cell's own constructor refuses was accepted here"
        );
        assert!(
            refusal("XLON=8:4:2:3:5:16:0").contains("interval of zero"),
            "a ratio interval that refuses nothing was accepted"
        );
        // One bad entry refuses the whole declaration rather than leaving
        // the good one in force beside a venue silently on the fallback.
        assert!(
            VenueQuoteLimits::read(Some("XLON=8:4:2:3:5:16:250,XPAR=nonsense"), &venues).is_err()
        );
    }
}
