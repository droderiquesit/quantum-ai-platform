//! `qip-strategy` — a typed IR, a compiler that refuses what it cannot bound,
//! and a runtime that cannot call a language model.
//!
//! A strategy on the execution path is not a program in a general-purpose
//! language. It is an expression in [`ir`], type-checked and lowered by
//! [`compile::StrategyCompiler`] into a flat arena of shared nodes, and
//! evaluated by [`runtime::StrategyRuntime`] in a cost that was computed
//! before it was ever deployed.
//!
//! ## Why the language is this small
//!
//! * **No call, anywhere.** [`ir::Expr`] has no variant that reaches outside
//!   the process — no network, no file, no model service, no language model.
//!   This is structural, not a rule applied elsewhere: there is no
//!   constructor, so there is nothing to audit for and nothing a later change
//!   can quietly re-enable without altering that enum. The only learned
//!   function admitted is [`model::DistilledModel`], which carries its own
//!   coefficients and evaluates in a number of steps its own value states.
//! * **No loop and no recursion.** The IR is a finite tree and the compiled
//!   form is an arena in which every reference points strictly backwards. A
//!   worst-case cost therefore always exists, and a strategy whose cost
//!   exceeds the budget is refused rather than deployed and watched.
//! * **Exact and statistical values are different types.** Comparing a
//!   `Decimal` price to an `f64` volatility is a compile error, not a silent
//!   conversion. Crossing between them is written out — [`ir::Expr::Widen`] or
//!   [`ir::Expr::Ratio`] — and survives into the compiled form where a
//!   reviewer can see it.
//! * **Undefined inputs emit nothing.** The runtime checks the vector before
//!   it evaluates anything, so a strategy physically cannot act on a feature
//!   that has no value.
//!
//! ## Sharing
//!
//! The compiler interns children before parents, so two strategies that ask
//! the same question share the node that answers it, and
//! [`compile::CompilerReport::deduplication_ratio`] says how much of that
//! happened.
//!
//! ```
//! use qip_contracts::{FeatureKey, SignalKind, StrategyId};
//! use qip_core::{Decimal, Duration, ObjectId};
//! use qip_strategy::catalogue::FeatureCatalogue;
//! use qip_strategy::compile::StrategyCompiler;
//! use qip_strategy::ir::{Expr, Rule, StrategySpec, Type};
//!
//! let subject = ObjectId::from_string("OBJ00000000000000000000AAA");
//! let pressure = FeatureKey::new("book_pressure", subject.clone()).with("levels", 5);
//!
//! let mut catalogue = FeatureCatalogue::new();
//! catalogue.declare(pressure.clone(), Type::Statistic)?;
//!
//! let spec = StrategySpec::new(
//!     StrategyId::new("lean-with-the-book"),
//!     subject,
//!     Duration::from_millis(250),
//! )
//! .with_rule(Rule::new(
//!     "enter",
//!     SignalKind::Enter,
//!     Expr::feature(pressure).greater_than(Expr::Statistic(0.4)),
//!     Expr::Exact(Decimal::from_int(100)),
//!     Expr::Statistic(0.62),
//!     500,
//! ));
//!
//! let mut compiler = StrategyCompiler::new(catalogue);
//! let compiled = compiler.compile(&spec)?;
//! assert!(compiled.cost() <= compiler.limits().max_nodes);
//! # Ok::<(), qip_core::Error>(())
//! ```

pub mod catalogue;
pub mod compile;
pub mod ir;
pub mod model;
pub mod program;
pub mod runtime;

pub use catalogue::FeatureCatalogue;
pub use compile::{
    CompiledRule, CompiledStrategy, CompilerLimits, CompilerReport, StrategyCompiler, Warning,
    WarningKind,
};
pub use ir::{ArithmeticOp, CompareOp, Expr, ExtremumOp, LogicalOp, Rule, StrategySpec, Type};
pub use model::{DistilledModel, ModelForm, TreeNode};
pub use program::{Node, NodeRef, Op, Program};
pub use runtime::StrategyRuntime;
