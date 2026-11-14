//! The settlement convention both halves of the Settlement Brain share
//! (ADR 0127, R12-CAPITAL-1100).
//!
//! `qip-capital-fabric`'s `SettlementCalendar` and `qip-edge`'s
//! `SettlementTerms` each once defined their own T+0/T+1/T+2 enum: two claims
//! about one fact, free to drift until the centre and a cell disagreed about
//! the day proceeds land. One definition here makes the term set structural.

use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// How many settlement days after the value date funds are usable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettlementConvention {
    /// Same settlement day, if the instruction makes the cut-off.
    T0,
    /// The next settlement day.
    T1,
    /// Two settlement days on: the equity and FX spot convention in most
    /// markets, and the one that quietly turns a Thursday into a Monday.
    T2,
}

impl SettlementConvention {
    /// Settlement days added to the value date.
    pub const fn days(self) -> u32 {
        match self {
            Self::T0 => 0,
            Self::T1 => 1,
            Self::T2 => 2,
        }
    }

    /// A stable label for logs and refusal messages.
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
