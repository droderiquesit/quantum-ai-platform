//! Panels, and the difference between "nothing happened" and "we are blind".
//!
//! This is the one idea in the console that matters more than the layout. A
//! table with no rows renders identically whether the platform is flat or
//! whether nothing is reaching the platform at all, and on a trading console
//! those two readings are opposites: the first says there is no risk, the
//! second says the risk is unmeasured. An operator who reads the second as the
//! first will believe a position is closed when it is open.
//!
//! So a panel is never a bare `Vec`. It is a [`Panel<T>`] carrying its own
//! [`Freshness`], and the three states are kept apart all the way into the
//! markup:
//!
//! * [`Freshness::Current`] with rows — the platform reported, and this is
//!   what it reported.
//! * [`Freshness::Current`] with no rows — the platform reported, and it
//!   genuinely has nothing. This is the only state that means zero.
//! * [`Freshness::Stale`] — something reported once, too long ago to be
//!   believed. The rows are shown *marked as stale*, never as current.
//! * [`Freshness::Absent`] — nothing has reported, and the panel says why.
//!   An absent panel carries no rows at all, which is enforced by the
//!   constructor rather than by convention: [`Panel::absent`] has nowhere to
//!   put one.
//!
//! [`Panel::default`] is [`Freshness::Absent`] on purpose. A panel nobody has
//! filled in has not observed zero, and defaulting the other way is exactly
//! the failure this module exists to prevent.

use serde::{Deserialize, Serialize};

/// Whether a panel's contents can be believed, and as of when.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Freshness {
    /// Reported, and recent enough to act on.
    Current {
        /// When the report the panel is built from was made.
        as_of: String,
    },
    /// Reported once, older than the freshness bound for this panel.
    Stale {
        /// When the last report was made.
        as_of: String,
        /// How long ago that was, for display.
        age: String,
        /// The bound the age exceeded, so the reader can judge the margin.
        bound: String,
    },
    /// Nothing has reported. `reason` says what is not reporting and why,
    /// because "no data" without a cause is indistinguishable from a bug in
    /// the console.
    Absent {
        /// What is missing, in a sentence an operator can act on.
        reason: String,
    },
}

impl Freshness {
    /// A short label for the panel header.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Current { .. } => "current",
            Self::Stale { .. } => "STALE",
            Self::Absent { .. } => "no data",
        }
    }

    /// The value of the `data-state` attribute the panel renders with.
    ///
    /// Machine-readable as well as visible: a test can assert the distinction
    /// survives into the markup, which is the only place it protects anyone.
    pub fn state_attribute(&self, has_rows: bool) -> &'static str {
        match self {
            Self::Current { .. } if has_rows => "current",
            // Reported, and genuinely empty. The only state that means zero.
            Self::Current { .. } => "empty-reported",
            Self::Stale { .. } => "stale",
            Self::Absent { .. } => "absent",
        }
    }
}

/// Rows, and whether they can be believed.
///
/// The rows are private so the invariant holds by construction: there is no
/// way to build an absent panel that carries rows, and therefore no way for an
/// absent panel to render numbers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Panel<T> {
    freshness: Freshness,
    rows: Vec<T>,
}

impl<T> Panel<T> {
    /// A panel built from a report that is recent enough to act on.
    ///
    /// An empty `rows` here is a real observation of nothing, and renders as
    /// one.
    pub fn current(rows: Vec<T>, as_of: impl Into<String>) -> Self {
        Self {
            freshness: Freshness::Current {
                as_of: as_of.into(),
            },
            rows,
        }
    }

    /// A panel whose last report is older than its freshness bound.
    ///
    /// The rows are kept, because "here is what we last saw and it is old" is
    /// more useful than "nothing" — but they are kept behind a freshness that
    /// forces every renderer to say so.
    pub fn stale(
        rows: Vec<T>,
        as_of: impl Into<String>,
        age: impl Into<String>,
        bound: impl Into<String>,
    ) -> Self {
        Self {
            freshness: Freshness::Stale {
                as_of: as_of.into(),
                age: age.into(),
                bound: bound.into(),
            },
            rows,
        }
    }

    /// A panel nothing has reported, with the reason.
    ///
    /// Takes no rows. That is the point: an absent panel cannot be given
    /// numbers to show, so it cannot accidentally render as zero.
    pub fn absent(reason: impl Into<String>) -> Self {
        Self {
            freshness: Freshness::Absent {
                reason: reason.into(),
            },
            rows: Vec::new(),
        }
    }

    pub fn freshness(&self) -> &Freshness {
        &self.freshness
    }

    pub fn rows(&self) -> &[T] {
        &self.rows
    }

    /// Whether nothing has reported this panel.
    pub fn is_absent(&self) -> bool {
        matches!(self.freshness, Freshness::Absent { .. })
    }

    /// Whether the panel's rows are older than its freshness bound.
    pub fn is_stale(&self) -> bool {
        matches!(self.freshness, Freshness::Stale { .. })
    }

    /// Whether the panel reports a real, observed absence of rows.
    ///
    /// Distinct from [`Panel::is_absent`], and the distinction is the whole
    /// point of the type: this one means the platform looked and found
    /// nothing, that one means nobody looked.
    pub fn is_empty_but_reported(&self) -> bool {
        self.rows.is_empty() && matches!(self.freshness, Freshness::Current { .. })
    }
}

impl<T> Default for Panel<T> {
    /// Absent, not empty.
    ///
    /// A panel nobody assembled has not observed zero. Defaulting to a current
    /// empty panel would let a forgotten field render as "no exposure", which
    /// is the specific lie this module exists to make unspellable.
    fn default() -> Self {
        Self::absent("this panel was not assembled; nothing reported it")
    }
}
