//! ADR 0100 §1: the `qip event-fabric …` subcommand family. Filled by
//! SLICE-34 (`grant`) and SLICE-37 (`inspect`), both of which own this file
//! to add their subcommand's match arm.

pub mod grant;
pub mod inspect;
