//! `qip change-window`: a plan that changes a core resource is applied only
//! inside a committed change window (R12-GCP-9120).
//!
//! ```text
//! qip change-window --plan <terraform show -json of the saved plan> \
//!                   --policy infrastructure/change-windows.json [--now <RFC 3339>]
//! ```
//!
//! The plan is the saved plan `infra.yml` is about to apply, so what is judged
//! is exactly what lands. A core resource — the hierarchy, a network, a
//! database, the policy's `core_resource_types` — that the plan creates,
//! updates, replaces or deletes makes the apply refused outside every window;
//! a plan touching none is admitted at any time. A core change reaches every
//! workload at once, so it lands while a person is watching, never because a
//! dispatch happened at 03:00.
//!
//! Fails closed: an unreadable plan or policy, a policy with no window or an
//! empty one, and a core resource change with no planned actions are each a
//! refusal, never a pass.

use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use serde_json::Value;

/// Exit code for a plan refused outside its window.
pub const OUTSIDE_WINDOW: u8 = 3;

const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// One window: the days it is open on and the UTC minutes it spans.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    pub days: Vec<usize>,
    pub from: u32,
    pub to: u32,
}

/// The committed policy, validated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    pub core_resource_types: Vec<String>,
    pub windows: Vec<Window>,
}

fn minutes(text: &str) -> Result<u32> {
    let parse = |part: Option<&str>| part.and_then(|p| p.parse::<u32>().ok());
    let mut parts = text.split(':');
    match (
        parse(parts.next()),
        parse(parts.next()),
        parts.next(),
        text.len(),
    ) {
        (Some(h), Some(m), None, 5) if h < 24 && m < 60 => Ok(h * 60 + m),
        _ => Err(Error::invalid(format!("\"{text}\" is not an HH:MM time"))),
    }
}

impl Policy {
    /// Read and validate the policy, refusing one a core change could never
    /// pass or one that names nothing as core.
    pub fn parse(policy: &Value) -> Result<Self> {
        let core_resource_types: Vec<String> = policy["core_resource_types"]
            .as_array()
            .map(|types| {
                types
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        if core_resource_types.is_empty() {
            return Err(Error::invalid("the policy names no core resource types"));
        }
        let raw = policy["windows"].as_array().cloned().unwrap_or_default();
        if raw.is_empty() {
            return Err(Error::invalid(
                "the policy names no change window, so a core change could never land",
            ));
        }
        let mut windows = Vec::new();
        for window in raw {
            let mut days = Vec::new();
            for day in window["days"].as_array().cloned().unwrap_or_default() {
                let found = day
                    .as_str()
                    .and_then(|d| DAYS.iter().position(|known| *known == d));
                match found {
                    Some(index) => days.push(index),
                    None => {
                        return Err(Error::invalid(format!(
                            "a window's day {day} is not one of {}",
                            DAYS.join(", ")
                        )));
                    }
                }
            }
            if days.is_empty() {
                return Err(Error::invalid("a window names no days"));
            }
            let from = minutes(window["from"].as_str().unwrap_or(""))?;
            let to = minutes(window["to"].as_str().unwrap_or(""))?;
            if from >= to {
                return Err(Error::invalid(format!(
                    "a window from {} to {} is empty",
                    window["from"], window["to"]
                )));
            }
            windows.push(Window { days, from, to });
        }
        Ok(Self {
            core_resource_types,
            windows,
        })
    }

    /// Whether `now` falls inside a window, in UTC.
    pub fn open_at(&self, now: Timestamp) -> bool {
        let seconds = now.as_nanos().div_euclid(1_000_000_000);
        let day_number = seconds.div_euclid(86_400);
        // 1970-01-01 was a Thursday, index 4 counting from Sunday.
        let weekday = usize::try_from((day_number + 4).rem_euclid(7)).unwrap_or(0);
        let minute = u32::try_from(seconds.rem_euclid(86_400) / 60).unwrap_or(0);
        self.windows
            .iter()
            .any(|w| w.days.contains(&weekday) && minute >= w.from && minute < w.to)
    }
}

/// The core resources `plan` changes, as `address [actions]`.
pub fn core_changes(plan: &Value, policy: &Policy) -> Result<Vec<String>> {
    let mut changes = Vec::new();
    for change in plan["resource_changes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        let kind = change["type"].as_str().unwrap_or("");
        if !policy.core_resource_types.iter().any(|core| core == kind) {
            continue;
        }
        let address = change["address"].as_str().unwrap_or(kind);
        let actions: Vec<&str> = change["change"]["actions"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        if actions.is_empty() {
            return Err(Error::invalid(format!(
                "{address} has no planned actions to judge"
            )));
        }
        if actions.iter().all(|a| *a == "no-op" || *a == "read") {
            continue;
        }
        changes.push(format!("{address} [{}]", actions.join(",")));
    }
    Ok(changes)
}

/// The refusal for applying `plan` at `now`, or `None` when admitted.
pub fn judge(plan: &Value, policy: &Value, now: Timestamp) -> Result<Option<String>> {
    let policy = Policy::parse(policy)?;
    let changes = core_changes(plan, &policy)?;
    if changes.is_empty() || policy.open_at(now) {
        return Ok(None);
    }
    Ok(Some(format!(
        "{} core resource change(s) outside every change window: {}",
        changes.len(),
        changes.join("; ")
    )))
}

/// `qip change-window`, from its arguments; `now` is the clock's reading,
/// overridden by `--now`.
pub fn run(arguments: &[String], now: Timestamp) -> Result<u8> {
    let value = |flag: &str| {
        arguments
            .iter()
            .position(|a| a == flag)
            .and_then(|i| arguments.get(i + 1))
    };
    let read = |flag: &str| -> Result<Value> {
        let path =
            value(flag).ok_or_else(|| Error::invalid(format!("{flag} <path> is required")))?;
        let text = std::fs::read_to_string(path)
            .map_err(|e| Error::io(format!("cannot read {path}: {e}")))?;
        serde_json::from_str(&text).map_err(|e| Error::invalid(format!("{path} is not JSON: {e}")))
    };
    let plan = read("--plan")?;
    let policy = read("--policy")?;
    let now = match value("--now") {
        Some(text) => Timestamp::parse_rfc3339(text)
            .ok_or_else(|| Error::invalid(format!("--now {text} is not an RFC 3339 time")))?,
        None => now,
    };
    match judge(&plan, &policy, now)? {
        None => {
            println!("change window: admitted");
            Ok(0)
        }
        Some(refusal) => {
            eprintln!("change window: refused: {refusal}");
            Ok(OUTSIDE_WINDOW)
        }
    }
}
