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
//! Nothing here performs I/O or reads a clock. `Declaration::parse` takes
//! text and `Declaration::apply_counting` takes the instant its caller holds,
//! so the same declaration replays identically.

use crate::Platform;
use qip_capital_fabric::journal::FabricCommand;
use qip_core::Timestamp;
use qip_core::error::{Error, Result};

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
const DECLARATION_KEYS: [&str; 1] = ["commands"];

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
                     only key is commands, and a misspelt one would be ignored in silence"
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

        Ok(Self { commands })
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
