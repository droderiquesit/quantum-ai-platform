//! `qip-normalization` — canonicalisation and data contracts.
//!
//! Providers disagree about everything: the same instrument is `AAPL`, `AAPL.O`
//! and `AAPL US Equity`; the same venue is `XNYS`, `NYSE` and `N`; UK equities
//! are quoted in pence while their currency is pounds; one feed stamps
//! milliseconds and another microseconds. Left alone, those differences become
//! phantom instruments, hundred-fold price errors and misaligned time series.
//!
//! Two things happen here. [`Normalizer`] rewrites records into the platform's
//! canonical form, and [`DataContract`] asserts what a well-formed record for a
//! topic must look like, so a provider that silently changes its units is
//! caught at the boundary rather than in a portfolio.

pub mod contract;
pub mod normalizer;

pub use contract::{ContractViolation, DataContract, FieldRule, RuleOutcome};
pub use normalizer::{NormalizationReport, Normalizer, ScaleGuard, SymbolMapping, UnitConversion};
