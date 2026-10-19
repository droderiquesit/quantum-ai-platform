//! The operator's declaration of where idle cash may be parked, and how much
//! of it, read from a committed file by the composition roots (ADR 0159
//! decisions 4, 6 and 7).
//!
//! There is no licensed shelf and no custodian feed behind this: both are data
//! subscriptions, and a hand-declared shelf is the only one this platform has.
//! The consequence is the posture the whole module keeps — **an absent
//! declaration routes nothing**. There is no default shelf, no default share
//! and no default horizon; a declaration with instruments and no `routing`
//! block is a shelf that is listed and not armed, and the router is not called.
//!
//! The file is parsed here and validated at load so a composition root fails
//! closed on a typo instead of discovering it on the first cycle that has cash
//! to place. This module reads no file and no environment: the apps do, as the
//! architecture requires, and hand the text in.
//!
//! The values in the committed file are the RISK lane's to declare (ADR 0159,
//! "Not decided here"): every capacity, every realisation, the share and the
//! ladder horizons. The committed file declares none of them.

use qip_capital::bank::{InstrumentClass, RoutingTerms, Shelf, YieldInstrument};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration};
use serde::{Deserialize, Serialize};

/// The schema the loader understands. A file naming another is refused rather
/// than read as this one.
pub const DECLARATION_SCHEMA: u32 = 1;

/// One instrument as the operator wrote it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredInstrument {
    pub id: String,
    /// `cash`, `stable_value`, `short_duration` or `other`. Nothing else:
    /// financing and credit have no home for idle cash (ADR 0003, ADR 0021).
    pub class: String,
    /// Hours after placing before the cash can be had back.
    pub realisation_hours: u32,
    pub capacity: Decimal,
    pub yield_rate: Decimal,
}

/// The two limits that arm routing, as the operator wrote them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredRouting {
    /// Fraction of the cycle's unreleased capital that may be routed.
    pub max_share: Decimal,
    /// Ladder rungs in hours, shortest first.
    pub ladder_horizons_hours: Vec<u32>,
}

/// The declaration file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct YieldShelfDeclaration {
    pub schema: u32,
    #[serde(default)]
    pub instruments: Vec<DeclaredInstrument>,
    /// Absent means routing is not armed.
    #[serde(default)]
    pub routing: Option<DeclaredRouting>,
    /// A sentence for the next reader of the file. Read by nothing.
    #[serde(default)]
    pub note: Option<String>,
}

/// A declaration that passed validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeclaredShelf {
    pub shelf: Shelf,
    /// `None` when routing is not armed.
    pub terms: Option<RoutingTerms>,
}

impl YieldShelfDeclaration {
    /// Parse a declaration, refusing unknown fields and a foreign schema.
    pub fn parse(text: &str) -> Result<Self> {
        let declaration: Self = serde_json::from_str(text).map_err(|error| {
            Error::invalid(format!(
                "the yield shelf is not valid JSON of the declared shape: {error}"
            ))
        })?;
        if declaration.schema != DECLARATION_SCHEMA {
            return Err(Error::invalid(format!(
                "the yield shelf declares schema {}, and this build reads schema \
                 {DECLARATION_SCHEMA}; a file in another shape is not read as this one",
                declaration.schema
            )));
        }
        Ok(declaration)
    }

    /// Validate every instrument and the routing terms, naming what is wrong.
    pub fn validate(&self) -> Result<DeclaredShelf> {
        let mut instruments = Vec::with_capacity(self.instruments.len());
        for declared in &self.instruments {
            let class = match declared.class.as_str() {
                "cash" => InstrumentClass::Cash,
                "stable_value" => InstrumentClass::StableValue,
                "short_duration" => InstrumentClass::ShortDuration,
                "other" => InstrumentClass::Other,
                unknown => {
                    return Err(Error::invalid(format!(
                        "shelf instrument {} names class {unknown:?}; the classes are cash, \
                         stable_value, short_duration and other, and there is no credit or \
                         financing class because idle cash is never lent (ADR 0003, ADR 0021)",
                        declared.id
                    )));
                }
            };
            instruments.push(YieldInstrument {
                id: declared.id.clone(),
                class,
                realisation: Duration::from_hours(i64::from(declared.realisation_hours)),
                capacity: declared.capacity,
                yield_rate: declared.yield_rate,
            });
        }
        let shelf = Shelf::new(instruments)?;
        let terms = self
            .routing
            .as_ref()
            .map(|routing| {
                RoutingTerms::new(
                    routing.max_share,
                    routing
                        .ladder_horizons_hours
                        .iter()
                        .map(|hours| Duration::from_hours(i64::from(*hours)))
                        .collect(),
                )
            })
            .transpose()?;
        Ok(DeclaredShelf { shelf, terms })
    }
}

/// Parse and validate in one step, for a composition root that has read the
/// file. The declaration is returned rather than the validated shelf because
/// it is what the platform's configuration carries; `Platform::new` validates
/// it again, so a configuration assembled another way is held to the same rule.
pub fn load(text: &str) -> Result<YieldShelfDeclaration> {
    let declaration = YieldShelfDeclaration::parse(text)?;
    declaration.validate()?;
    Ok(declaration)
}

/// The variable a composition root reads the shelf's path from. Unset means no
/// shelf is declared and nothing is routed; set and unreadable, or readable and
/// invalid, stops the process.
pub const PATH_VARIABLE: &str = "QIP_YIELD_SHELF_PATH";

/// Read and validate the declaration at `path`, which a composition root took
/// from [`PATH_VARIABLE`]. Reads one file and no environment.
///
/// Refused, not skipped, when the file does not read or does not validate: a
/// process that fell back to "no shelf" because the file the operator named was
/// missing would run healthy with routing silently off, the state that hid the
/// empty universe once already.
pub fn load_file(path: &str) -> Result<YieldShelfDeclaration> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        Error::io(format!(
            "configuration: {PATH_VARIABLE} names {path}, which cannot be read: {error}. Unset \
             it to run with no shelf; a named shelf that does not read is not a shelf that is off"
        ))
    })?;
    load(&text).map_err(|error| {
        Error::invalid(format!(
            "configuration: {PATH_VARIABLE} names {path}, which is not a valid yield shelf: {}",
            error.message()
        ))
    })
}
