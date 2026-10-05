//! An operator's capital-fabric declaration, parsed into the commands the
//! journal writes.
//!
//! # Why this lives in the kernel and not in the API that reads the file
//!
//! `api_boundary.rs` forbids `qip-api` an edge to `qip-capital-fabric`, along
//! with every other crate that can name an order constructor, a venue adapter
//! or a capital movement: "the application layer composes reads and raises
//! intents, and a crate that can name an order constructor or a venue adapter
//! is no longer that layer". A producer for §37.1, §37.3 and §38.4 needs the
//! command vocabulary, and the straightforward way to get it — adding the
//! dependency to `qip-api` — would have bought a feature by deleting a
//! reviewed boundary.
//!
//! So the vocabulary stays here, where the fabric is already composed, and
//! the composition root keeps only what a composition root owns: the file,
//! its fingerprint, and how much of it has been applied. The API hands this
//! module text and receives an opaque `Declaration`; it never names a
//! `FabricCommand`, and the boundary test still passes without an exception
//! written for this change.
//!
//! # What a declaration is
//!
//! An ordered list of commands, each one a [`FabricCommand`] in exactly the
//! shape the event log writes — the same shape a replay reads, so what an
//! operator declares and what the chain carries cannot drift apart. Order is
//! the operator's and it is applied in order: a corridor naming a destination
//! the list has not yet proposed is refused *by the control*, and that
//! refusal is a record, because a refusal is a decision and belongs in the
//! log.
//!
//! # The corridor policy the gate is measured against
//!
//! A declaration may also carry `corridor_policy`: the corridors the
//! Intelligence layer rules on, each with the strategies it funds and the two
//! ceilings the desk stated. It is applied through
//! [`Platform::declare_corridors`] before any command, every time the
//! declaration is applied.
//!
//! Until this key existed `declare_corridors` was called by tests and by
//! nothing else, which made the gate unreachable from a deployed process in a
//! way that read as reachable: `Platform::decide_fabric` re-derives the ruling
//! a gate command states and refuses the command when no policy has been
//! declared, so a `gate` command naming a proposed corridor stopped the feed
//! on every deployment, and the only assessment an operator could put on the
//! chain was a refusal against a corridor nobody had proposed. The seven
//! checks ran in the kernel's suite and for no operator.
//!
//! The policy is a statement and not an act. A command is history, sealed on
//! the chain and never amended; a ceiling is a figure the desk holds and may
//! restate, and the ruling each assessment was made under is written into its
//! own gate record. So the policy is outside the append-only prefix the
//! composition root compares, and restating it replaces it.
//!
//! Each subject is built through `CorridorSubject::new` rather than
//! deserialised into place, because that constructor is where a zero ceiling
//! and a pilot ceiling above the full one are refused, and a derive would
//! have walked straight past both.
//!
//! Nothing here performs I/O or reads a clock. `Declaration::parse` takes
//! text and `Declaration::apply_counting` takes the instant its caller holds,
//! so the same declaration replays identically.

use crate::Platform;
use qip_capital_fabric::journal::FabricCommand;
use qip_contracts::signal::StrategyId;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Timestamp};
use qip_lifecycle::corridor::{CorridorRoute, CorridorSubject};
use serde::Deserialize;

/// The most commands one declaration may carry.
///
/// A bound rather than none, because every command becomes a record on the
/// hash-chained log and the whole file is applied inside one start-up or one
/// cycle. This is the ceiling on that burst, not a judgement about how many
/// corridors a desk may run; a declaration past it is refused naming the
/// count, never truncated.
pub const MAX_FABRIC_COMMANDS: usize = 1024;

/// The keys a declaration document may carry.
///
/// Anything else is refused by name. A misspelt `command` that was silently
/// ignored would leave a process serving with nothing declared while its
/// banner said a declaration was loaded, which is the state this module
/// exists to end.
const DECLARATION_KEYS: [&str; 2] = ["commands", "corridor_policy"];

/// One corridor's policy subject as an operator writes it.
///
/// A document type rather than `CorridorSubject`'s own derive, so that every
/// subject passes through the constructor that refuses a ceiling of zero.
/// Unknown fields are denied for the reason unknown keys are: a misspelt
/// `pilot_celing` that was dropped would leave the corridor governed by a
/// figure nobody wrote.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SubjectDocument {
    route: RouteDocument,
    ceiling: Decimal,
    pilot_ceiling: Decimal,
    funds: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteDocument {
    source: String,
    destination: String,
    asset: String,
}

/// Where the declaration is read from, named here so the refusals this
/// module raises can say how to run with none.
///
/// The variable is *read* by the composition root and by nothing else; this
/// constant is the name, not a reader of it.
pub const FABRIC_PATH_VARIABLE: &str = "QIP_CAPITAL_FABRIC_PATH";

/// A validated declaration: an ordered, non-empty list of fabric commands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Declaration {
    /// The commands, in file order.
    ///
    /// Private, and that is the second half of the boundary this module
    /// exists to keep. A composition root holds a `Declaration` so it can
    /// apply it and compare it against the file's next version; a public
    /// field would hand that root the command vocabulary anyway, through a
    /// type it is allowed to name, and `api_boundary.rs`'s dependency check
    /// would go on passing while the thing it protects had gone.
    commands: Vec<FabricCommand>,
    /// The corridor policy's subjects, in file order. Empty when the
    /// declaration carries no `corridor_policy`, which leaves whatever policy
    /// the platform already holds alone.
    policy: Vec<CorridorSubject>,
}

impl Declaration {
    /// Parse and validate a declaration document.
    ///
    /// Every refusal names the position or the key and never the value.
    pub fn parse(text: &str) -> Result<Self> {
        let document: serde_json::Value = serde_json::from_str(text).map_err(|error| {
            // Position and class, never `error` itself: a `serde_json` syntax
            // error quotes the bytes it stopped on, and those bytes are the
            // operator's declaration. See the refusal rule on this module.
            let class = match error.classify() {
                serde_json::error::Category::Io => "the file could not be read",
                serde_json::error::Category::Syntax => "a syntax error",
                serde_json::error::Category::Data => "a value of the wrong shape",
                serde_json::error::Category::Eof => "an unexpected end of input",
            };
            Error::invalid(format!(
                "the file is not JSON: {class} at line {} column {}",
                error.line(),
                error.column()
            ))
        })?;
        let object = document.as_object().ok_or_else(|| {
            Error::invalid("the declaration is not a JSON object; write { \"commands\": [ … ] }")
        })?;
        for key in object.keys() {
            if !DECLARATION_KEYS.contains(&key.as_str()) {
                return Err(Error::invalid(format!(
                    "the declaration carries the key {key}, which is not one it may carry; the \
                     keys are commands and corridor_policy, and a misspelt one would be ignored \
                     in silence"
                )));
            }
        }

        // Before any command is deserialised, because the deserialiser is
        // where the damage happens: `Decimal`'s `Deserialize` falls through
        // to `from_f64` for anything that is not an exact integer, so a cap
        // written `0.1` would be recorded as the nearest binary double and
        // the corridor would carry a ceiling nobody wrote.
        refuse_inexact_numbers(&document, "the declaration")?;

        let commands_value = object.get("commands").ok_or_else(|| {
            Error::invalid("commands is missing; a declaration is a list of fabric commands")
        })?;
        let list = commands_value.as_array().ok_or_else(|| {
            Error::invalid("commands is not a list; a declaration is a list of fabric commands")
        })?;
        if list.is_empty() {
            return Err(Error::invalid(format!(
                "commands is empty; a declaration of nothing declares nothing, and the fabric \
                 would stay unreached while the banner said a declaration was loaded. Unset \
                 {FABRIC_PATH_VARIABLE} to run with none"
            )));
        }
        if list.len() > MAX_FABRIC_COMMANDS {
            return Err(Error::denied(format!(
                "commands lists {} entries against a bound of {MAX_FABRIC_COMMANDS}; every \
                 command becomes a record on the chain and the whole file is applied in one \
                 pass, so split the declaration rather than raising the bound",
                list.len()
            )));
        }

        let mut commands = Vec::with_capacity(list.len());
        for (index, value) in list.iter().enumerate() {
            // `serde_json::from_value` on the command types, so the document
            // an operator writes is the document the log carries. The error
            // is restated by position: serde's message quotes the field it
            // stopped on and can carry the value with it.
            let command: FabricCommand =
                serde_json::from_value(value.clone()).map_err(|error| {
                    Error::invalid(format!(
                        "commands[{index}] is not a fabric command: {}. A command names its \
                         subject (destination, corridor, wallet or gate) and the action within \
                         it",
                        shape_of(&error)
                    ))
                })?;
            // The command types do not deny unknown fields — they are the
            // log's own decode, and a replay must keep reading records
            // written before a field existed. So the check lives here, where
            // the input is an operator's file rather than the chain's
            // history: anything the round trip drops is something the desk
            // wrote and the log will not carry, and a misspelt `adress` that
            // was ignored in silence would journal a destination keyed on
            // something nobody meant.
            let round_tripped = serde_json::to_value(&command).map_err(|error| {
                Error::invalid(format!(
                    "commands[{index}] parsed and does not re-serialise: {}. The parser and the \
                     log's own encoding disagree, which is a defect in this module",
                    shape_of(&error)
                ))
            })?;
            refuse_dropped_keys(value, &round_tripped, &format!("commands[{index}]"))?;
            commands.push(command);
        }

        let policy = match object.get("corridor_policy") {
            None => Vec::new(),
            Some(value) => Self::parse_policy(value)?,
        };

        Ok(Self { commands, policy })
    }

    /// Build the corridor policy's subjects, each through its constructor.
    ///
    /// Refusals name the position and the rule, and never the constructor's
    /// own message: that message names the route, a route's destination is an
    /// account, and no refusal from this module repeats what the file says.
    fn parse_policy(value: &serde_json::Value) -> Result<Vec<CorridorSubject>> {
        let list = value.as_array().ok_or_else(|| {
            Error::invalid(
                "corridor_policy is not a list; it is a list of corridors, each with its route, \
                 its two ceilings and the strategies it funds",
            )
        })?;
        // Refused rather than read as "no policy". An absent key leaves the
        // platform's policy alone; an empty list looks like a policy
        // withdrawn, and treating it as absent would leave every corridor
        // ruled by the one last declared while the file said there was none.
        if list.is_empty() {
            return Err(Error::invalid(
                "corridor_policy is empty; omit the key to leave the policy as it stands, or \
                 name the corridors it rules on. An empty list would read as a policy withdrawn \
                 while the corridors went on being ruled by the one last declared",
            ));
        }
        if list.len() > MAX_FABRIC_COMMANDS {
            return Err(Error::denied(format!(
                "corridor_policy lists {} corridors against a bound of {MAX_FABRIC_COMMANDS}; \
                 the policy is re-derived whenever a strategy's rung moves, so split the desk's \
                 corridors rather than raising the bound",
                list.len()
            )));
        }
        let mut subjects = Vec::with_capacity(list.len());
        for (index, entry) in list.iter().enumerate() {
            let document: SubjectDocument =
                serde_json::from_value(entry.clone()).map_err(|error| {
                    Error::invalid(format!(
                        "corridor_policy[{index}] is not a corridor subject: {}. A subject names \
                         its route (source, destination, asset), a ceiling, a pilot_ceiling and \
                         the strategies it funds",
                        shape_of(&error)
                    ))
                })?;
            let subject = CorridorRoute::new(
                document.route.source,
                document.route.destination,
                document.route.asset,
            )
            .and_then(|route| {
                CorridorSubject::new(
                    route,
                    document.ceiling,
                    document.pilot_ceiling,
                    document.funds.into_iter().map(StrategyId::new),
                )
            })
            .map_err(|refusal| {
                Error::invalid(format!(
                    "corridor_policy[{index}] is refused ({}): every leg of the route is named, \
                     both ceilings are positive, the pilot ceiling is at most the full one, and \
                     the corridor funds at least one strategy. A corridor that should carry \
                     nothing is suspended by where its strategies stand, not by a ceiling of zero",
                    refusal.code()
                ))
            })?;
            // Two subjects on one route are two claims about one fact. The
            // lifecycle ledger refuses the pair too, in words that name the
            // route; refusing here names the two positions instead.
            if let Some(earlier) = subjects
                .iter()
                .position(|held: &CorridorSubject| held.route() == subject.route())
            {
                return Err(Error::invalid(format!(
                    "corridor_policy[{index}] rules on the same route as \
                     corridor_policy[{earlier}]; give each route one policy, because a transfer \
                     matched against whichever was found first would be governed by a cap nobody \
                     chose"
                )));
            }
            subjects.push(subject);
        }
        Ok(subjects)
    }

    /// Apply the commands from `from` onward, returning how many were
    /// applied.
    ///
    /// A refusal by the control is **not** an error here: `decide_fabric`
    /// answers with an `Outcome::Refused` inside an `Ok` record, because the
    /// refusal is a decision and belongs in the log. An `Err` is the journal
    /// or the log refusing to take the record at all, and it stops the
    /// caller — the root refuses to start, the middleware refuses the cycle —
    /// because commands before it have been journalled and the rest have not,
    /// and a process that carried on would be serving over a half-applied
    /// declaration.
    pub fn apply_into(
        &self,
        platform: &mut Platform,
        from: usize,
        now: Timestamp,
    ) -> Result<usize> {
        let mut applied = 0;
        self.apply_counting(platform, from, now, &mut applied)?;
        Ok(applied)
    }

    /// The loop [`Declaration::apply_into`] wraps, counting into `applied` as
    /// it goes.
    ///
    /// Separate because the count matters most on the path that fails. The
    /// kernel offers no transaction: the commands before a refusal are on the
    /// chain and the rest are not, so a caller that retried from the start
    /// would offer the control transitions the log has already made. Only a
    /// caller holding the count can resume at the command that did not land,
    /// and a `Result<usize>` cannot carry a count and an error at once.
    pub fn apply_counting(
        &self,
        platform: &mut Platform,
        from: usize,
        now: Timestamp,
        applied: &mut usize,
    ) -> Result<()> {
        // The policy first, and on every application: a gate command further
        // down states a ruling `decide_fabric` re-derives from this policy,
        // so a policy applied after the commands would refuse them against
        // the one it replaced. An absent policy declares nothing rather than
        // clearing what is held — silence in a file is not an instruction.
        if !self.policy.is_empty() {
            platform
                .declare_corridors(self.policy.clone(), now)
                .map_err(|refusal| {
                    Error::invalid(format!(
                        "corridor_policy was refused when it was declared ({}); the subjects \
                         parsed, so check them against what the lifecycle ledger already holds",
                        refusal.code()
                    ))
                })?;
        }
        for (index, command) in self.commands.iter().enumerate().skip(from) {
            platform
                .decide_fabric(command.clone(), now)
                .map_err(|error| {
                    Error::invalid(format!(
                        "commands[{index}] was refused when it was journalled ({}); the kernel \
                     judges the ruling a gate command states and the log judges the record, so \
                     check that command against Platform::corridor_funding and against what the \
                     chain already carries",
                        error.code()
                    ))
                })?;
            *applied += 1;
        }
        Ok(())
    }
}

/// Refuse any JSON number in `value` that is not an exact integer.
///
/// Walked over the whole document rather than checked field by field: the
/// command types own their own fields and gain more, and a list of field
/// names here would be a second claim about which of them are decimals — one
/// that goes stale silently the first time a cap gains a component. The path
/// is built as it descends so the refusal names where to look.
fn refuse_inexact_numbers(value: &serde_json::Value, at: &str) -> Result<()> {
    match value {
        serde_json::Value::Number(number) => {
            if number.is_i64() {
                Ok(())
            } else {
                Err(Error::invalid(format!(
                    "{at} is a JSON number that is not an exact integer; write it as a string. \
                     A decimal read as a JSON number goes through a binary float, so the figure \
                     recorded would not be the figure written"
                )))
            }
        }
        serde_json::Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                refuse_inexact_numbers(item, &format!("{at}[{index}]"))?;
            }
            Ok(())
        }
        serde_json::Value::Object(fields) => {
            for (key, field) in fields {
                refuse_inexact_numbers(field, &format!("{at}.{key}"))?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Refuse any key in `wrote` that is absent from `kept`.
///
/// Key sets rather than whole values, because the two differ legitimately: a
/// timestamp re-serialises in its canonical form and a decimal written as an
/// integer comes back as a string. What must not differ is which keys
/// survived — a key the operator wrote and the log will not carry is a field
/// that was ignored in silence, and every such field is either a misspelling
/// or a belief about the command types that is no longer true.
fn refuse_dropped_keys(
    wrote: &serde_json::Value,
    kept: &serde_json::Value,
    at: &str,
) -> Result<()> {
    match (wrote, kept) {
        (serde_json::Value::Object(wrote), serde_json::Value::Object(kept)) => {
            for (key, value) in wrote {
                let Some(kept_value) = kept.get(key) else {
                    return Err(Error::invalid(format!(
                        "{at} carries the field {key}, which the command it names does not have; \
                         it would be dropped in silence, so correct the spelling or remove it"
                    )));
                };
                refuse_dropped_keys(value, kept_value, &format!("{at}.{key}"))?;
            }
            Ok(())
        }
        (serde_json::Value::Array(wrote), serde_json::Value::Array(kept)) => {
            for (index, (value, kept_value)) in wrote.iter().zip(kept).enumerate() {
                refuse_dropped_keys(value, kept_value, &format!("{at}[{index}]"))?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// The class of a `serde_json` error, without its message.
///
/// The message quotes the input it stopped on, and the input is the
/// operator's declaration. The line and column are enough to find the
/// command; the bytes are not this module's to repeat.
fn shape_of(error: &serde_json::Error) -> String {
    let class = match error.classify() {
        serde_json::error::Category::Io => "the value could not be read",
        serde_json::error::Category::Syntax => "a syntax error",
        serde_json::error::Category::Data => "a field of the wrong shape, or one missing",
        serde_json::error::Category::Eof => "an unexpected end of input",
    };
    format!("{class} at line {} column {}", error.line(), error.column())
}

impl Declaration {
    /// How many commands the declaration carries.
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    /// Whether it carries none. Cannot be true of a parsed declaration —
    /// [`Declaration::parse`] refuses an empty list — and present because
    /// clippy asks for it beside `len`.
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// How many corridors the declaration states a policy for.
    pub fn corridors_ruled(&self) -> usize {
        self.policy.len()
    }

    /// Whether this declaration's first `count` commands are the same acts,
    /// in the same order, as `earlier`'s.
    ///
    /// The question the composition root asks of a file that has changed
    /// under it. Exposed as a comparison rather than as the commands
    /// themselves so that the caller — an app, on the far side of a boundary
    /// that forbids it the command vocabulary — can hold a declaration
    /// without being able to name what is in it.
    pub fn shares_prefix_with(&self, earlier: &Self, count: usize) -> bool {
        self.commands.len() >= count
            && earlier.commands.len() >= count
            && self.commands[..count] == earlier.commands[..count]
    }

    /// The position of the first command that differs from `earlier`'s within
    /// the first `count`, or `None` when they agree.
    ///
    /// Only ever called to name a position in a refusal, which is why it
    /// answers with one rather than with the commands that differ.
    pub fn first_difference_with(&self, earlier: &Self, count: usize) -> Option<usize> {
        self.commands
            .iter()
            .zip(&earlier.commands)
            .take(count)
            .position(|(now, before)| now != before)
    }
}
