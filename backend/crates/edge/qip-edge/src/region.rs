//! Which other regions have gone dark, as this cell has been told (§36.3).
//!
//! Blueprint §36.3 gives two rows for a region that stops answering — one
//! for a region's own node crashing and one for the whole cloud region
//! failing — and both say the same thing about *every other* region:
//! **mirrors involving it suspend, and nothing else changes.** A cell whose
//! peer has gone dark keeps running its own strategies and its own
//! intra-venue cycles; what it must not do is take one side of a §31.1
//! mirror whose other side is nobody.
//!
//! # Why a cell cannot work this out for itself
//!
//! It cannot measure a peer. The venues in another region answer this cell
//! whether or not the cell that trades them is alive, so venue reachability
//! says nothing about a peer's node, and a cell that inferred darkness from
//! its own sessions would be publishing a claim about a process it has never
//! spoken to. So this is a *reading*, handed to the cell the way the polled
//! halt is handed to it: [`Cell::apply_region_outlook`] takes one and the
//! cell never reaches for a file, a socket or a clock to obtain it.
//!
//! [`Cell::apply_region_outlook`]: crate::cell::Cell::apply_region_outlook
//!
//! # The three readings, and why the third is not an error
//!
//! [`RegionOutlook::Unreadable`] is the whole reason this is an enum rather
//! than a set. A wire that cannot be read is not the same fact as a wire
//! that says nothing, and collapsing the two would make a missing mount read
//! as "every peer is fine" — the failure mode the polled halt flag names in
//! its own module, arrived at from the other direction. So an unreadable
//! wire darkens **every region other than this cell's own**: mirrored
//! trading stops everywhere, local trading continues, and the cell says so
//! on a gauge rather than going quiet about it.
//!
//! Nothing here can widen anything. The three readings differ only in which
//! mirrors are refused, and a reading naming every region in the world still
//! cannot make the cell send an order it would not otherwise have sent.

use qip_core::error::{Error, Result};
use std::collections::BTreeSet;

/// The most regions one reading may name.
///
/// The platform has seven regional cells (ADR 0008) and this is an order of
/// magnitude above that. It is a bound rather than a fit: the reading is
/// built from bytes somebody outside this process wrote, and the set becomes
/// a metric's value and a journal entry's field, so a reading naming ten
/// thousand regions is refused rather than stored.
pub const MAX_DARK_REGIONS: usize = 64;

/// Why the cell is treating a region as dark.
///
/// Two values, and they are the whole `source` label of
/// `qip_edge_regions_dark`. An enum rather than two string literals at the
/// recording site for the reason every other label in `crate::telemetry` is
/// one: the series' cardinality has to be a property of this source file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DarkSource {
    /// Somebody named the region. This is the ordinary case: an operator
    /// draining a region, or a redeploy that has not finished.
    Declared,
    /// The wire that names them could not be read, so every region other
    /// than this cell's own is treated as dark until it can be.
    Unreadable,
}

impl DarkSource {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Declared => "declared",
            Self::Unreadable => "unreadable",
        }
    }
}

/// What the cell has been told about the other regions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum RegionOutlook {
    /// Nothing is dark. The state every cell has run in, and the state a
    /// present-but-empty declaration produces.
    #[default]
    AllLit,
    /// These regions have gone dark. Ordered, because the set reaches a
    /// journal entry and a replay that reorders is not a replay.
    Dark(BTreeSet<String>),
    /// The wire could not be read, and this says why.
    Unreadable(String),
}

impl RegionOutlook {
    /// A reading that names regions.
    ///
    /// Refuses an empty set rather than accepting it as "nothing is dark":
    /// [`Self::AllLit`] already says that, and two spellings of one state is
    /// how a caller ends up asserting the wrong one. Refuses a region id
    /// that is empty or carries surrounding whitespace, and **does not trim
    /// it** — `RegionId::new`, `CellConfig::with_venue_in_region` and
    /// `MirrorArrangement::with_round_trip` all refuse the same thing for
    /// the same reason: two ids differing by a space are two regions to a
    /// mirror edge, so an id corrected here would darken a region nobody
    /// named and leave the one they did name lit.
    pub fn declared(regions: impl IntoIterator<Item = String>) -> Result<Self> {
        let mut named = BTreeSet::new();
        for region in regions {
            if region.is_empty() || region.trim() != region {
                return Err(Error::invalid(format!(
                    "region id {region:?} is empty or carries surrounding whitespace; two ids \
                     differing by a space are two regions to a mirror edge, so a reading that \
                     trimmed this would suspend mirrors into a region nobody named and leave \
                     the named one trading"
                )));
            }
            named.insert(region);
            if named.len() > MAX_DARK_REGIONS {
                return Err(Error::invalid(format!(
                    "a reading naming more than {MAX_DARK_REGIONS} dark regions is not a \
                     statement about this platform's regions; name the ones that are dark, or \
                     say the wire is unreadable"
                )));
            }
        }
        if named.is_empty() {
            return Err(Error::invalid(
                "a reading naming no region is not the same fact as an unreadable one; use the \
                 all-lit reading to say nothing is dark",
            ));
        }
        Ok(Self::Dark(named))
    }

    /// A reading that could not be obtained, and why.
    ///
    /// The detail is required. This reading suspends every mirror the cell
    /// takes part in, and an operator looking at a cell that has stopped
    /// mirroring needs to be told whether the mount went away or the file
    /// held something nobody could parse.
    pub fn unreadable(detail: impl Into<String>) -> Result<Self> {
        let detail = detail.into();
        if detail.trim().is_empty() {
            return Err(Error::invalid(
                "an unreadable region wire suspends every mirror this cell takes part in, and a \
                 reading that does not say why leaves an operator with a silent cell and no \
                 thread to pull; name what could not be read",
            ));
        }
        Ok(Self::Unreadable(detail))
    }

    /// Whether `region` is dark to a cell whose own region is `home`.
    ///
    /// `home` is a parameter rather than a field because this type is the
    /// *reading* and not the cell: the same reading handed to two cells in
    /// two regions must answer differently for the unreadable arm, and a
    /// home region baked into the reading would be a second place the cell's
    /// identity is written down.
    pub fn is_dark(&self, region: &str, home: &str) -> bool {
        match self {
            Self::AllLit => false,
            Self::Dark(named) => named.contains(region),
            // Everything that is not this cell. A cell is never dark to
            // itself: it is the process asking, and a cell that suspended
            // its own side would refuse its local cycles too, which is the
            // one thing §36.3's "everything else" column forbids.
            Self::Unreadable(_) => region != home,
        }
    }

    /// The source label for [`Self::is_dark`]'s answers, or `None` when
    /// nothing is dark.
    pub const fn source(&self) -> Option<DarkSource> {
        match self {
            Self::AllLit => None,
            Self::Dark(_) => Some(DarkSource::Declared),
            Self::Unreadable(_) => Some(DarkSource::Unreadable),
        }
    }

    /// The regions this reading names, for the journal. Empty for the two
    /// readings that name none.
    pub fn named(&self) -> Vec<String> {
        match self {
            Self::AllLit | Self::Unreadable(_) => Vec::new(),
            Self::Dark(named) => named.iter().cloned().collect(),
        }
    }

    /// The most bytes a declaration may hold.
    ///
    /// A declaration is a handful of region ids, one per line. A file larger
    /// than this is not the declaration, whatever put it there, and reading
    /// the beginning of a larger one would suspend the mirrors the first
    /// kilobyte happened to name.
    pub const MAX_CONTENT_BYTES: usize = 4 * 1024;

    /// Read a declaration's bytes: one region id per line, blank lines
    /// ignored.
    ///
    /// Every failure is [`Self::Unreadable`], which darkens every region
    /// other than the reading cell's own. That is the fail-closed direction
    /// and it is the opposite of what a set would do: a parse that dropped
    /// the lines it could not read would answer "fewer regions are dark",
    /// which is the one answer that lets a cell trade one side of a mirror
    /// whose other side is gone.
    ///
    /// The content is never echoed into the reason. Whatever ended up in the
    /// file is not a fact the cell's chain should carry — the same decision
    /// `PolledHalt::from_content` makes about the halt flag, and for the same
    /// reason.
    pub fn from_content(bytes: &[u8]) -> Self {
        if bytes.len() > Self::MAX_CONTENT_BYTES {
            return Self::Unreadable(format!(
                "the declaration holds {} bytes and may hold at most {}",
                bytes.len(),
                Self::MAX_CONTENT_BYTES
            ));
        }
        let Ok(text) = std::str::from_utf8(bytes) else {
            return Self::Unreadable("the declaration is not text".to_string());
        };
        let mut named: Vec<String> = Vec::new();
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if line.trim() != line {
                return Self::Unreadable(
                    "a line of the declaration carries surrounding whitespace, and two region \
                     ids differing by a space are two regions"
                        .to_string(),
                );
            }
            named.push(line.to_string());
        }
        if named.is_empty() {
            // Present and naming nothing is the operator saying nothing is
            // dark. Distinct from absent only in that somebody wrote it.
            return Self::AllLit;
        }
        match Self::declared(named) {
            Ok(outlook) => outlook,
            Err(refusal) => Self::Unreadable(format!(
                "the declaration could not be read: {}",
                refusal.message()
            )),
        }
    }

    /// The sentence an operator reads beside a suspended mirror.
    pub fn describe(&self) -> String {
        match self {
            Self::AllLit => "every region is answering".to_string(),
            Self::Dark(named) => format!(
                "{} region(s) dark: {}",
                named.len(),
                named
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<&str>>()
                    .join(", ")
            ),
            Self::Unreadable(detail) => format!(
                "the region availability wire could not be read ({detail}), so every other \
                 region is treated as dark"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reading_that_names_no_region_is_refused_rather_than_read_as_all_lit() {
        // The failure this prevents: two spellings of "nothing is dark", one
        // of which arrives from a wire. A caller that built `Dark({})` from
        // an empty file would hold a reading whose `source()` says regions
        // are dark and whose `is_dark` says none are, and the gauge would
        // report a suspension nobody could find.
        let refusal = RegionOutlook::declared(Vec::new())
            .expect_err("an empty set is not a statement that regions are dark");
        assert_eq!(refusal.code(), "invalid");
        assert!(
            refusal.message().contains("use the all-lit reading"),
            "the refusal should name what to use instead: {}",
            refusal.message()
        );
        // The half that proves it admits a good value.
        assert!(RegionOutlook::declared(["eu-west".to_string()]).is_ok());
    }

    #[test]
    fn a_region_id_with_surrounding_whitespace_is_refused_rather_than_trimmed() {
        let refusal = RegionOutlook::declared([" eu-west".to_string()])
            .expect_err("whitespace makes it a different region");
        assert_eq!(refusal.code(), "invalid");
        assert!(
            refusal.message().contains("surrounding whitespace"),
            "the refusal should say why: {}",
            refusal.message()
        );
    }

    #[test]
    fn an_unreadable_wire_darkens_every_region_except_the_cells_own() {
        // §36.3's "everything else" column: the mirrors suspend and the
        // local side keeps trading. A reading that darkened the home region
        // too would suspend nothing extra — no mirror edge has both ends at
        // home — but it would report the cell as dark to itself, and an
        // operator reading that gauge would take the cell for stopped.
        let outlook = RegionOutlook::unreadable("the mount is gone").expect("a stated reason");
        assert_eq!(outlook.source(), Some(DarkSource::Unreadable));
        assert!(outlook.is_dark("eu-west", "us-east"));
        assert!(outlook.is_dark("ap-south", "us-east"));
        assert!(
            !outlook.is_dark("us-east", "us-east"),
            "a cell is never dark to itself"
        );
    }

    #[test]
    fn an_unreadable_reading_with_no_stated_reason_is_refused() {
        let refusal = RegionOutlook::unreadable("   ")
            .expect_err("a reading that suspends every mirror must say why");
        assert_eq!(refusal.code(), "invalid");
        assert!(
            RegionOutlook::unreadable("the directory is missing").is_ok(),
            "a stated reason is admitted"
        );
    }

    #[test]
    fn a_declared_reading_darkens_exactly_the_regions_it_names() {
        let outlook = RegionOutlook::declared(["eu-west".to_string(), "ap-south".to_string()])
            .expect("valid");
        // Premise: the reading really holds two regions, or the negative
        // assertion below would pass on an empty set.
        assert_eq!(outlook.named(), vec!["ap-south", "eu-west"]);
        assert!(outlook.is_dark("eu-west", "us-east"));
        assert!(!outlook.is_dark("us-west", "us-east"));
        assert_eq!(outlook.source(), Some(DarkSource::Declared));
    }

    #[test]
    fn the_default_reading_darkens_nothing_and_names_no_source() {
        let outlook = RegionOutlook::default();
        assert_eq!(outlook, RegionOutlook::AllLit);
        assert_eq!(outlook.source(), None);
        assert!(!outlook.is_dark("eu-west", "us-east"));
    }

    #[test]
    fn a_declaration_that_cannot_be_parsed_darkens_every_region_rather_than_fewer() {
        // The fail-closed direction, and the one a set would get wrong. A
        // parse that dropped the lines it could not read would answer "fewer
        // regions are dark", which is the answer that lets a cell take one
        // side of a mirror whose other side is gone.
        let oversized = vec![b'x'; RegionOutlook::MAX_CONTENT_BYTES + 1];
        let outlook = RegionOutlook::from_content(&oversized);
        assert_eq!(outlook.source(), Some(DarkSource::Unreadable));
        assert!(outlook.is_dark("eu-west", "us-east"));
        assert!(!outlook.is_dark("us-east", "us-east"));

        let indented = RegionOutlook::from_content(b"  eu-west\n");
        assert_eq!(
            indented.source(),
            Some(DarkSource::Unreadable),
            "an indented id is not the region it looks like, and is not silently trimmed"
        );
        assert!(
            !indented.describe().contains("eu-west"),
            "the declaration's own content must not be echoed into the reason: {}",
            indented.describe()
        );
    }

    #[test]
    fn a_declaration_naming_regions_one_per_line_darkens_exactly_those() {
        let outlook = RegionOutlook::from_content(b"eu-west\n\nap-south\n");
        // Premise: this really parsed two regions, or the negative assertion
        // below would pass on an all-lit reading.
        assert_eq!(outlook.named(), vec!["ap-south", "eu-west"]);
        assert!(outlook.is_dark("eu-west", "us-east"));
        assert!(outlook.is_dark("ap-south", "us-east"));
        assert!(!outlook.is_dark("us-west", "us-east"));
    }

    #[test]
    fn a_declaration_that_names_nothing_leaves_every_region_lit() {
        // An operator who empties the file has released every region, and a
        // present-but-empty file that read as unreadable would suspend every
        // mirror on the deployment's own housekeeping.
        assert_eq!(RegionOutlook::from_content(b""), RegionOutlook::AllLit);
        assert_eq!(RegionOutlook::from_content(b"\n\n"), RegionOutlook::AllLit);
    }

    #[test]
    fn a_reading_naming_more_regions_than_the_bound_is_refused() {
        let many: Vec<String> = (0..=MAX_DARK_REGIONS)
            .map(|n| format!("region-{n}"))
            .collect();
        // Premise: the set really is over the bound rather than deduplicated
        // under it.
        assert_eq!(many.len(), MAX_DARK_REGIONS + 1);
        let refusal = RegionOutlook::declared(many).expect_err("over the bound");
        assert_eq!(refusal.code(), "invalid");
        let at_bound: Vec<String> = (0..MAX_DARK_REGIONS)
            .map(|n| format!("region-{n}"))
            .collect();
        assert!(
            RegionOutlook::declared(at_bound).is_ok(),
            "the bound itself is admitted"
        );
    }
}
