//! Agent resource budgets.
//!
//! An agent that runs long enough will always find something to say. Budgets
//! bound that: wall time, tool calls, language-model calls, tokens and money.
//!
//! Exceeding a budget is an error, not a warning. A run that quietly stopped
//! early and returned its partial findings as if they were complete is worse
//! than one that failed, because only the second is visible.

use qip_core::error::{Error, Result};
use qip_core::time::Duration;
use serde::{Deserialize, Serialize};
use std::fmt;

/// The limits one agent run may consume.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budget {
    /// Maximum wall-clock duration of a single run.
    pub wall_time: Duration,
    /// Maximum number of capability invocations.
    pub tool_calls: u32,
    /// Maximum number of language-model completions.
    pub language_model_calls: u32,
    /// Maximum total tokens across those completions.
    pub tokens: u32,
    /// Maximum monetary cost in USD micro-units (1e-6 USD), so the budget
    /// stays exact and comparable without pulling in `Decimal` here.
    pub cost_micros: u64,
}

impl Budget {
    /// A research agent: minutes of thinking, a handful of model calls.
    pub fn research_default() -> Self {
        Self {
            wall_time: Duration::from_mins(5),
            tool_calls: 200,
            language_model_calls: 12,
            tokens: 60_000,
            cost_micros: 500_000, // $0.50
        }
    }

    /// A fast-path agent: no language model at all, and a hard microsecond
    /// bound. The Fast Brain must never wait on a model.
    pub fn fast_path() -> Self {
        Self {
            wall_time: Duration::from_millis(50),
            tool_calls: 64,
            language_model_calls: 0,
            tokens: 0,
            cost_micros: 0,
        }
    }

    /// A deep-research agent given room to run.
    pub fn deep_research() -> Self {
        Self {
            wall_time: Duration::from_mins(30),
            tool_calls: 2_000,
            language_model_calls: 60,
            tokens: 400_000,
            cost_micros: 5_000_000, // $5.00
        }
    }

    pub fn with_wall_time(mut self, wall_time: Duration) -> Self {
        self.wall_time = wall_time;
        self
    }

    pub fn with_language_model_calls(mut self, calls: u32) -> Self {
        self.language_model_calls = calls;
        self
    }

    pub fn with_tokens(mut self, tokens: u32) -> Self {
        self.tokens = tokens;
        self
    }

    pub fn validate(&self) -> Result<()> {
        if self.wall_time.as_nanos() <= 0 {
            return Err(Error::invalid("budget wall time must be positive"));
        }
        if self.tool_calls == 0 {
            return Err(Error::invalid(
                "budget allows no tool calls; the agent could not observe anything",
            ));
        }
        if self.language_model_calls > 0 && self.tokens == 0 {
            return Err(Error::invalid(
                "budget allows language-model calls but no tokens",
            ));
        }
        Ok(())
    }
}

/// Which budget line ran out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetLine {
    WallTime,
    ToolCalls,
    LanguageModelCalls,
    Tokens,
    Cost,
}

impl BudgetLine {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::WallTime => "wall_time",
            Self::ToolCalls => "tool_calls",
            Self::LanguageModelCalls => "language_model_calls",
            Self::Tokens => "tokens",
            Self::Cost => "cost",
        }
    }
}

impl fmt::Display for BudgetLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a run has spent so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spend {
    pub elapsed: Duration,
    pub tool_calls: u32,
    pub language_model_calls: u32,
    pub tokens: u32,
    pub cost_micros: u64,
}

impl Spend {
    /// Fraction of the tightest budget line consumed, in `[0, ∞)`.
    ///
    /// Reported so an operator can see an agent that habitually runs to 98% of
    /// its budget before it starts failing.
    pub fn utilisation(&self, budget: &Budget) -> f64 {
        let ratio = |used: f64, limit: f64| if limit <= 0.0 { 0.0 } else { used / limit };
        [
            ratio(
                self.elapsed.as_nanos() as f64,
                budget.wall_time.as_nanos() as f64,
            ),
            ratio(f64::from(self.tool_calls), f64::from(budget.tool_calls)),
            ratio(
                f64::from(self.language_model_calls),
                f64::from(budget.language_model_calls),
            ),
            ratio(f64::from(self.tokens), f64::from(budget.tokens)),
            ratio(self.cost_micros as f64, budget.cost_micros as f64),
        ]
        .into_iter()
        .fold(0.0_f64, f64::max)
    }
}

/// Tracks spend against a budget and refuses the call that would exceed it.
///
/// The ledger charges *before* the work happens, so a call that would breach
/// the budget never runs. Charging afterwards would mean the expensive
/// operation always gets one free breach.
#[derive(Clone, Debug)]
pub struct BudgetLedger {
    budget: Budget,
    spend: Spend,
    agent: String,
    exhausted: Option<BudgetLine>,
}

impl BudgetLedger {
    pub fn new(budget: Budget, agent: impl Into<String>) -> Self {
        Self {
            budget,
            spend: Spend::default(),
            agent: agent.into(),
            exhausted: None,
        }
    }

    pub fn budget(&self) -> &Budget {
        &self.budget
    }

    pub fn spend(&self) -> Spend {
        self.spend
    }

    /// The line that ran out, once one has.
    pub fn exhausted(&self) -> Option<BudgetLine> {
        self.exhausted
    }

    pub fn utilisation(&self) -> f64 {
        self.spend.utilisation(&self.budget)
    }

    /// Charge one capability invocation.
    pub fn charge_tool_call(&mut self) -> Result<()> {
        if self.spend.tool_calls >= self.budget.tool_calls {
            return self.exceed(BudgetLine::ToolCalls, u64::from(self.budget.tool_calls));
        }
        self.spend.tool_calls += 1;
        Ok(())
    }

    /// Charge one language-model completion and its tokens.
    ///
    /// Tokens are charged with the call because a model that returns is
    /// billable whether or not the caller liked the answer.
    pub fn charge_language_model(&mut self, tokens: u32, cost_micros: u64) -> Result<()> {
        if self.spend.language_model_calls >= self.budget.language_model_calls {
            return self.exceed(
                BudgetLine::LanguageModelCalls,
                u64::from(self.budget.language_model_calls),
            );
        }
        if self.spend.tokens.saturating_add(tokens) > self.budget.tokens {
            return self.exceed(BudgetLine::Tokens, u64::from(self.budget.tokens));
        }
        if self.spend.cost_micros.saturating_add(cost_micros) > self.budget.cost_micros {
            return self.exceed(BudgetLine::Cost, self.budget.cost_micros);
        }
        self.spend.language_model_calls += 1;
        self.spend.tokens += tokens;
        self.spend.cost_micros += cost_micros;
        Ok(())
    }

    /// Charge tokens without a call, for the difference between an estimated
    /// prompt and what a completion actually consumed.
    pub fn charge_tokens_only(&mut self, tokens: u32) -> Result<()> {
        if self.spend.tokens.saturating_add(tokens) > self.budget.tokens {
            return self.exceed(BudgetLine::Tokens, u64::from(self.budget.tokens));
        }
        self.spend.tokens += tokens;
        Ok(())
    }

    /// Record elapsed time, failing once the run has overrun.
    ///
    /// Called at every capability boundary rather than by a watchdog thread:
    /// the platform is deterministic, and a wall-clock interrupt is not.
    pub fn observe_elapsed(&mut self, elapsed: Duration) -> Result<()> {
        self.spend.elapsed = elapsed;
        if elapsed > self.budget.wall_time {
            return self.exceed(
                BudgetLine::WallTime,
                self.budget.wall_time.as_nanos() as u64,
            );
        }
        Ok(())
    }

    /// Whether the run can still do useful work.
    pub fn has_headroom(&self) -> bool {
        self.exhausted.is_none()
            && self.spend.tool_calls < self.budget.tool_calls
            && self.spend.elapsed <= self.budget.wall_time
    }

    fn exceed(&mut self, line: BudgetLine, limit: u64) -> Result<()> {
        self.exhausted = Some(line);
        Err(Error::denied(format!(
            "agent {} exhausted its {line} budget (limit {limit})",
            self.agent
        )))
    }
}
