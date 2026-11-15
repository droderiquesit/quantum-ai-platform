//! Where the node reads the published reflex packages its cell evaluates
//! (R12-REASON-3063, ADR 0600).
//!
//! `QIP_REFLEX_PACKAGES_DIR` names a directory of `*.json` files, each a
//! [`qip_contracts::reflex_package::PublishedReflexPackage`] the two-person
//! `model-pack` publish signed. Absent, the cell evaluates nothing and says
//! so. Present, **every file must verify**: a package that is in the
//! directory and does not load stops the process naming the file, because a
//! node that skipped it would run without a rule its operators believe is
//! installed. Files are read in name order so two nodes given one directory
//! admit the same set in the same order.

use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_edge::TrustRoot;
use qip_edge::reflex_rules::ReflexRuleBook;
use std::path::Path;

/// The directory of published packages.
pub const REFLEX_PACKAGES_VARIABLE: &str = "QIP_REFLEX_PACKAGES_DIR";

/// Admit every package under `dir` into a fresh book, or refuse the lot.
pub fn load_dir(dir: &Path, root: &TrustRoot, now: Timestamp) -> Result<ReflexRuleBook> {
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| {
            Error::io(format!(
                "configuration: {REFLEX_PACKAGES_VARIABLE} names {} which cannot be read: {e}",
                dir.display()
            ))
        })?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    names.sort();
    let mut book = ReflexRuleBook::new();
    for path in names {
        let bytes = std::fs::read(&path).map_err(|e| {
            Error::io(format!(
                "reflex package {} cannot be read: {e}",
                path.display()
            ))
        })?;
        book.install(&bytes, root, now).map_err(|e| {
            Error::denied(format!(
                "reflex package {} was refused and the node does not start without it: {}",
                path.display(),
                e.message()
            ))
        })?;
    }
    Ok(book)
}
