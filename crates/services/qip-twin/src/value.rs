//! The taint that keeps a number that never happened out of the books.
//!
//! Everything a counterfactual produces is wrapped in [`Simulated`]. The wrapper
//! exists for one reason: a simulated profit that can be added to a realised one
//! is the single most dangerous thing this crate could ship. A quarter that was
//! flat and a quarter that was flat plus four alternatives the platform did not
//! take look identical on a P&L line, and the second one is a fabrication.
//!
//! So the separation is made by the type system rather than by a field anyone
//! can forget to check:
//!
//! * There is no accessor that returns the wrapped value. No `into_inner`, no
//!   `Deref`, no `From<Simulated<Decimal>> for Decimal`, no public field, and
//!   no general `map` — a closure can write what it is handed into a variable
//!   the caller owns, so every operation here is a named one that returns
//!   another [`Simulated`].
//! * [`Simulated`] adds only to [`Simulated`]. `Decimal + Simulated<Decimal>`
//!   does not exist, and Rust's orphan rules mean no crate other than this one
//!   could add it — `Add` and `Decimal` are both foreign everywhere else.
//! * The taint survives serialization. A [`Simulated`] is written as a two-field
//!   object carrying an explicit `simulated: true`, so the JSON of a simulated
//!   P&L will not deserialize into a `Decimal` field, and one written with the
//!   flag false is refused on the way back in.
//!
//! The one door out is [`Simulated::as_f64_for_statistics`], because ranking
//! regrets needs a magnitude. It lands in the `f64` lane the house rules
//! reserve for statistics, and getting from there back into the books needs an
//! explicit `Decimal::from_f64` that a reviewer can grep for. That is the
//! honest boundary: unrepresentable would be better, and this is as close as a
//! reporting surface gets.

use qip_core::Decimal;
use serde::de::Error as DeError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::iter::Sum;
use std::ops::{Add, Neg, Sub};

/// A value that was computed about a world that did not happen.
///
/// See the module documentation for why this wrapper has no way out.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Simulated<T> {
    value: T,
}

impl<T> Simulated<T> {
    /// Mark a value as simulated.
    ///
    /// Deliberately the only direction that is easy. Wrapping a real number as
    /// simulated is conservative; the reverse is the mistake this type exists
    /// to prevent, and there is no method for it.
    pub const fn of(value: T) -> Self {
        Self { value }
    }
}

impl Simulated<Decimal> {
    /// Zero, simulated.
    pub const ZERO: Self = Self {
        value: Decimal::ZERO,
    };

    /// The magnitude, in the `f64` lane the house rules reserve for statistics.
    ///
    /// Named at length on purpose. Ranking alternatives by how much they would
    /// have earned needs a number; calling it anything shorter would make the
    /// call site read like an accessor rather than like a deliberate crossing.
    pub fn as_f64_for_statistics(self) -> f64 {
        self.value.to_f64()
    }

    pub fn is_positive(self) -> bool {
        self.value.is_positive()
    }

    pub fn is_negative(self) -> bool {
        self.value.is_negative()
    }

    pub fn is_zero(self) -> bool {
        self.value.is_zero()
    }

    pub fn abs(self) -> Self {
        Self {
            value: self.value.abs(),
        }
    }

    /// Scale by an exact factor, staying simulated.
    ///
    /// Used by the shrinkage in [`crate::regret`]: a mean regret pulled toward
    /// zero by how little evidence supports it is still a simulated figure.
    pub fn scaled_by(self, factor: Decimal) -> Self {
        Self {
            value: self.value.checked_mul(factor).unwrap_or(Decimal::ZERO),
        }
    }

    /// Divide by an exact divisor, staying simulated.
    ///
    /// Present because the arithmetic a total needs — a mean over a sample —
    /// has to happen somewhere, and the alternative was a general `map` that
    /// hands the inner value to a closure. A closure can write what it is given
    /// into a variable the caller owns, so a general `map` would have been
    /// exactly the escape hatch this type exists not to have. Every operation
    /// here is therefore a named one that returns another [`Simulated`].
    ///
    /// A zero divisor yields zero rather than failing: this is a reporting
    /// path, and understating a regret is the safe direction to fail in.
    pub fn divided_by(self, divisor: Decimal) -> Self {
        Self {
            value: self.value.checked_div(divisor).unwrap_or(Decimal::ZERO),
        }
    }
}

impl<T: Add<Output = T>> Add for Simulated<T> {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self {
            value: self.value + rhs.value,
        }
    }
}

impl<T: Sub<Output = T>> Sub for Simulated<T> {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self {
            value: self.value - rhs.value,
        }
    }
}

impl<T: Neg<Output = T>> Neg for Simulated<T> {
    type Output = Self;
    fn neg(self) -> Self {
        Self { value: -self.value }
    }
}

impl Sum for Simulated<Decimal> {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::ZERO, |a, b| a + b)
    }
}

/// Renders the taint, so a simulated figure in a log cannot be misread as a
/// real one by whoever is reading the log at three in the morning.
impl<T: fmt::Display> fmt::Display for Simulated<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (simulated)", self.value)
    }
}

impl<T: fmt::Display> fmt::Debug for Simulated<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "simulated({})", self.value)
    }
}

/// The field name carrying the value, and the flag that has to accompany it.
const VALUE_FIELD: &str = "simulated_value";
const FLAG_FIELD: &str = "simulated";

/// Written as `{"simulated_value": …, "simulated": true}`.
///
/// A bare number would deserialize straight into a `Decimal` field somewhere
/// downstream and the taint would be gone the moment it crossed a wire.
impl<T: Serialize> Serialize for Simulated<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Simulated", 2)?;
        state.serialize_field(VALUE_FIELD, &self.value)?;
        state.serialize_field(FLAG_FIELD, &true)?;
        state.end()
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Simulated<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Repr<T> {
            simulated_value: T,
            simulated: bool,
        }
        let repr = Repr::<T>::deserialize(deserializer)?;
        if !repr.simulated {
            // A record claiming a simulated figure is real is either corrupt or
            // an attempt to launder one. Neither should parse.
            return Err(D::Error::custom(
                "a simulated value arrived with simulated=false; a counterfactual figure cannot be reported as an actual",
            ));
        }
        Ok(Self {
            value: repr.simulated_value,
        })
    }
}
