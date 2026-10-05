//! AGENCY-026: an action tool is registered with six declarations or not at
//! all, and a tripped rate limit or stop control refuses the next call.
//!
//! The failure prevented is an adapter that can be invoked before anyone said
//! who it acts as, what it may touch, what it must disclose, where it may
//! operate, how often, and how to stop it. Time is passed in by the caller
//! (never read here) so the limiter is deterministic and replayable.

use crate::{required, text};
use qip_core::Error;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default)]
pub struct ToolDraft {
    pub name: Option<String>,
    pub acting_identity: Option<String>,
    pub permission_scope: Option<String>,
    pub disclosure_policy: Option<String>,
    pub jurisdiction_channel_policy: Option<String>,
    /// At most this many calls per `rate_window_secs`.
    pub rate_limit_calls: Option<u32>,
    pub rate_window_secs: Option<u64>,
    /// How the tool is stopped and its effects rolled back.
    pub rollback_stop_control: Option<String>,
}

#[derive(Debug)]
struct Tool {
    max_calls: u32,
    window_secs: u64,
    /// Call times inside the current window; never longer than `max_calls`.
    recent: Vec<u64>,
    stopped: bool,
}

/// Registered tools, keyed by name.
#[derive(Debug, Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Tool>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, draft: ToolDraft) -> Result<(), Error> {
        let name = text("name", draft.name)?;
        text("acting_identity", draft.acting_identity)?;
        text("permission_scope", draft.permission_scope)?;
        text("disclosure_policy", draft.disclosure_policy)?;
        text(
            "jurisdiction_channel_policy",
            draft.jurisdiction_channel_policy,
        )?;
        let max_calls = required("rate_limit_calls", draft.rate_limit_calls)?;
        let window_secs = required("rate_window_secs", draft.rate_window_secs)?;
        if max_calls == 0 || window_secs == 0 {
            return Err(Error::invalid(
                "rate limit and window must be positive; a tool that may never be called is not registered",
            ));
        }
        text("rollback_stop_control", draft.rollback_stop_control)?;
        if self.tools.contains_key(&name) {
            return Err(Error::invalid(format!(
                "tool `{name}` is already registered"
            )));
        }
        self.tools.insert(
            name,
            Tool {
                max_calls,
                window_secs,
                recent: Vec::new(),
                stopped: false,
            },
        );
        Ok(())
    }

    /// Trip the stop control: every later call is refused until re-registered.
    pub fn stop(&mut self, name: &str) -> Result<(), Error> {
        self.tool(name)?.stopped = true;
        Ok(())
    }

    /// Admit or refuse one call at `now_secs`. A refused call is not counted.
    pub fn invoke(&mut self, name: &str, now_secs: u64) -> Result<(), Error> {
        let tool = self.tool(name)?;
        if tool.stopped {
            return Err(Error::guard(format!("tool `{name}` is stopped")));
        }
        let window = tool.window_secs;
        tool.recent.retain(|t| now_secs.saturating_sub(*t) < window);
        if tool.recent.len() >= tool.max_calls as usize {
            return Err(Error::guard(format!(
                "tool `{name}` hit its rate limit of {} calls per {window}s",
                tool.max_calls
            )));
        }
        tool.recent.push(now_secs);
        Ok(())
    }

    fn tool(&mut self, name: &str) -> Result<&mut Tool, Error> {
        self.tools
            .get_mut(name)
            .ok_or_else(|| Error::not_found(format!("tool `{name}` is not registered")))
    }
}
