//! Whether a market may be created, listed or seeded at all (EXEC-007).
//!
//! Three authorities each have to permit it, and none of them speaks for the
//! others: the **venue**, whose rulebook says what may be listed there; the
//! **product** rules, which say whether this instrument may exist in this
//! form; and the **jurisdiction**, whose law says whether the trading entity
//! may do it. A request lacking a recorded permission from any one of the
//! three is refused here, before any venue is called and before any record of
//! a market exists, because [`CreationPermissions::admit`] returns the only
//! [`CreationAdmission`] there is and a create, list or seed operation that
//! demands one cannot be reached by a refused request.
//!
//! With nothing recorded, every creation is refused. That is the default
//! rather than a state somebody has to configure: the register starts empty
//! and has no constructor that starts it any other way.
//!
//! # What this is not
//!
//! * It is not [`crate::origination`]. That gate decides whether the desk may
//!   *quote* an instrument with no observable market — a business approval by
//!   instrument class, valuation confidence and an operator's sign-off. This
//!   one decides whether a market may be brought into existence, on the
//!   separate question of whether venue, product rules and law allow it. An
//!   approved quote is not a permitted listing.
//! * It is not [`crate::modes`], which enables an execution *mode* at a venue.
//!   A venue where quoting is enabled is not thereby a venue where the
//!   platform may list something new.
//! * It creates nothing. No Market Factory exists in this workspace
//!   (EXEC-006), so nothing consumes an admission yet; the gate is built
//!   first so that the capability, when it is built, has no ungated form.
//! * It does not relax the paper-trading boundary. An admitted request still
//!   reaches only a simulated or sandbox venue (ADR 0003).
//!
//! A permission is a recorded fact with a reference to its evidence, and it
//! covers exactly one action for one product at one venue in one
//! jurisdiction. Permission to list is not permission to seed: seeding
//! commits capital, and a register that read one as the other would have
//! widened itself.

use std::collections::BTreeMap;

use qip_core::error::{Error, Result};

/// Who has to permit a creation. All three, always.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CreationAuthority {
    /// The venue's own rulebook.
    Venue,
    /// The rules governing the product itself.
    Product,
    /// The law of the trading entity's jurisdiction.
    Jurisdiction,
}

impl CreationAuthority {
    /// Every authority, so a refusal can name each one that is missing and a
    /// test can prove it covered all of them.
    pub const ALL: [Self; 3] = [Self::Venue, Self::Product, Self::Jurisdiction];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Venue => "venue",
            Self::Product => "product",
            Self::Jurisdiction => "jurisdiction",
        }
    }
}

/// What is being done to the market.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CreationAction {
    /// Bring a market or instrument into existence.
    Create,
    /// List an existing instrument at a venue.
    List,
    /// Put initial liquidity into a created market.
    Seed,
}

impl CreationAction {
    pub const ALL: [Self; 3] = [Self::Create, Self::List, Self::Seed];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::List => "list",
            Self::Seed => "seed",
        }
    }
}

/// One action on one product at one venue in one jurisdiction.
///
/// Matched exactly, with no wildcard and no normalisation: a permission for
/// one venue is not a permission for a venue whose name differs by case, and
/// a register that guessed they were the same would be granting something
/// nobody recorded.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CreationRequest {
    pub action: CreationAction,
    pub venue: String,
    pub product: String,
    pub jurisdiction: String,
}

impl CreationRequest {
    pub fn new(
        action: CreationAction,
        venue: impl Into<String>,
        product: impl Into<String>,
        jurisdiction: impl Into<String>,
    ) -> Self {
        Self {
            action,
            venue: venue.into(),
            product: product.into(),
            jurisdiction: jurisdiction.into(),
        }
    }

    /// Refuse a request that names nothing, with what to name instead.
    fn validate(&self) -> Result<()> {
        for (field, value) in [
            ("venue", &self.venue),
            ("product", &self.product),
            ("jurisdiction", &self.jurisdiction),
        ] {
            if value.trim().is_empty() {
                return Err(Error::invalid(format!(
                    "a market-creation request with no {field} cannot be matched against any \
                     recorded permission; name the {field}"
                )));
            }
        }
        Ok(())
    }
}

/// Proof that all three authorities permit a request.
///
/// Private fields, one mint — [`CreationPermissions::admit`] — and no
/// `Deserialize`: an admission cannot be built by a caller or decoded out of
/// a payload, so a function that takes one cannot be reached by a model
/// output, a config value or a request the register refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreationAdmission {
    request: CreationRequest,
    /// The evidence each authority's permission was recorded with, in
    /// authority order, so the record of what was created can say on whose
    /// word.
    references: Vec<(CreationAuthority, String)>,
}

impl CreationAdmission {
    /// What was admitted. A create operation acts on this and on nothing
    /// else: an admission for one venue is not an admission for another.
    pub fn request(&self) -> &CreationRequest {
        &self.request
    }

    /// The evidence reference recorded for each authority.
    pub fn references(&self) -> &[(CreationAuthority, String)] {
        &self.references
    }
}

/// The register of recorded creation permissions. Empty by construction.
#[derive(Clone, Debug, Default)]
pub struct CreationPermissions {
    /// A `BTreeMap` because the refusal lists what is missing and the order
    /// reaches an operator's log.
    recorded: BTreeMap<(CreationRequest, CreationAuthority), String>,
}

impl CreationPermissions {
    /// A register holding nothing, under which every creation is refused.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `authority` permits `request`, citing `reference`.
    ///
    /// Refused without a reference: a permission nobody can trace to a
    /// rulebook entry, a product approval or a legal opinion is an assertion,
    /// and this register holds evidence. Recording the same permission again
    /// replaces its reference, so a superseded opinion does not linger.
    pub fn record(
        &mut self,
        authority: CreationAuthority,
        request: &CreationRequest,
        reference: &str,
    ) -> Result<()> {
        request.validate()?;
        if reference.trim().is_empty() {
            return Err(Error::invalid(format!(
                "a {} permission to {} {} at {} in {} was offered with no reference; cite the \
                 rulebook entry, product approval or legal opinion it rests on",
                authority.as_str(),
                request.action.as_str(),
                request.product,
                request.venue,
                request.jurisdiction
            )));
        }
        self.recorded
            .insert((request.clone(), authority), reference.trim().to_string());
        Ok(())
    }

    /// The authorities with no recorded permission for `request`.
    pub fn missing(&self, request: &CreationRequest) -> Vec<CreationAuthority> {
        CreationAuthority::ALL
            .into_iter()
            .filter(|authority| !self.recorded.contains_key(&(request.clone(), *authority)))
            .collect()
    }

    /// Admit `request`, or refuse it naming every authority still missing.
    ///
    /// Every missing authority is named at once rather than the first: a
    /// refusal that named one would make obtaining three permissions a
    /// sequence of three refusals, each read as the last obstacle.
    pub fn admit(&self, request: &CreationRequest) -> Result<CreationAdmission> {
        request.validate()?;
        let missing = self.missing(request);
        if !missing.is_empty() {
            let names: Vec<&str> = missing.iter().map(|authority| authority.as_str()).collect();
            return Err(Error::denied(format!(
                "no market may be created here: to {} {} at {} in {} needs a recorded permission \
                 from the venue, the product rules and the jurisdiction, and {} missing ({}). \
                 Record the missing permission with its reference; nothing was sent to the venue",
                request.action.as_str(),
                request.product,
                request.venue,
                request.jurisdiction,
                if missing.len() == 1 {
                    "one is"
                } else {
                    "these are"
                },
                names.join(", ")
            )));
        }
        let references = CreationAuthority::ALL
            .into_iter()
            .filter_map(|authority| {
                self.recorded
                    .get(&(request.clone(), authority))
                    .map(|reference| (authority, reference.clone()))
            })
            .collect();
        Ok(CreationAdmission {
            request: request.clone(),
            references,
        })
    }
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;

    fn request(action: CreationAction) -> CreationRequest {
        CreationRequest::new(action, "XSIM", "event:rain-in-london", "paper")
    }

    fn fully_permitted(request: &CreationRequest) -> Result<CreationPermissions> {
        let mut permissions = CreationPermissions::new();
        for authority in CreationAuthority::ALL {
            permissions.record(authority, request, "ref-1")?;
        }
        Ok(permissions)
    }

    #[test]
    fn with_nothing_recorded_every_action_is_refused_naming_all_three_authorities() {
        // The fail-closed default, for every action rather than the one a
        // test happened to try: a register that refused `create` and admitted
        // `seed` would commit capital to a market it would not have listed.
        let permissions = CreationPermissions::new();
        for action in CreationAction::ALL {
            let refusal = permissions
                .admit(&request(action))
                .expect_err("an empty register admitted a creation")
                .message()
                .to_string();
            // The delimited list, not the three words: the sentence around
            // it says "the venue, the product rules and the jurisdiction"
            // whatever is missing, so `contains("venue")` would pass on a
            // refusal that named nothing.
            assert!(
                refusal.contains("these are missing (venue, product, jurisdiction)"),
                "the refusal of {} does not list all three missing permissions: {refusal}",
                action.as_str()
            );
        }
    }

    #[test]
    fn any_two_of_the_three_permissions_are_not_enough() -> Result<()> {
        // Each authority is left out in turn, so no one of them is the
        // permission the gate forgot to ask for.
        let wanted = request(CreationAction::Create);
        for absent in CreationAuthority::ALL {
            let mut permissions = CreationPermissions::new();
            for authority in CreationAuthority::ALL {
                if authority != absent {
                    permissions.record(authority, &wanted, "ref-1")?;
                }
            }
            assert_eq!(
                permissions.missing(&wanted),
                vec![absent],
                "the premise failed: exactly one permission should be outstanding"
            );
            let refusal = permissions
                .admit(&wanted)
                .expect_err("two permissions of three admitted a creation")
                .message()
                .to_string();
            assert!(
                refusal.contains(&format!("one is missing ({})", absent.as_str())),
                "the refusal does not name the one missing authority ({}): {refusal}",
                absent.as_str()
            );
        }
        Ok(())
    }

    #[test]
    fn a_permission_covers_one_action_venue_product_and_jurisdiction_and_nothing_beside_them()
    -> Result<()> {
        let listed = request(CreationAction::List);
        let permissions = fully_permitted(&listed)?;
        assert!(
            permissions.admit(&listed).is_ok(),
            "the premise failed: the fully permitted request was refused"
        );
        // Permission to list is not permission to seed or to create, and it
        // is not permission anywhere else or for anything else.
        let mut neighbours = vec![
            CreationRequest {
                action: CreationAction::Seed,
                ..listed.clone()
            },
            CreationRequest {
                action: CreationAction::Create,
                ..listed.clone()
            },
        ];
        for (venue, product, jurisdiction) in [
            ("XOTHER", "event:rain-in-london", "paper"),
            ("xsim", "event:rain-in-london", "paper"),
            ("XSIM", "event:rain-in-paris", "paper"),
            ("XSIM", "event:rain-in-london", "elsewhere"),
        ] {
            neighbours.push(CreationRequest::new(
                CreationAction::List,
                venue,
                product,
                jurisdiction,
            ));
        }
        for neighbour in neighbours {
            assert_eq!(
                permissions.missing(&neighbour).len(),
                3,
                "a permission for {listed:?} was read as one for {neighbour:?}"
            );
            assert!(permissions.admit(&neighbour).is_err());
        }
        Ok(())
    }

    #[test]
    fn an_admission_carries_the_request_it_was_minted_for_and_each_authoritys_reference()
    -> Result<()> {
        let wanted = request(CreationAction::Create);
        let mut permissions = CreationPermissions::new();
        permissions.record(CreationAuthority::Venue, &wanted, " rulebook 4.2 ")?;
        permissions.record(CreationAuthority::Product, &wanted, "approval PA-9")?;
        permissions.record(CreationAuthority::Jurisdiction, &wanted, "opinion 2026-03")?;
        let admission = permissions.admit(&wanted)?;
        assert_eq!(admission.request(), &wanted);
        assert_eq!(
            admission.references(),
            &[
                (CreationAuthority::Venue, "rulebook 4.2".to_string()),
                (CreationAuthority::Product, "approval PA-9".to_string()),
                (
                    CreationAuthority::Jurisdiction,
                    "opinion 2026-03".to_string()
                ),
            ]
        );
        Ok(())
    }

    #[test]
    fn a_permission_with_no_reference_or_a_request_that_names_nothing_is_refused() -> Result<()> {
        let wanted = request(CreationAction::Create);
        let mut permissions = CreationPermissions::new();
        let unreferenced = permissions
            .record(CreationAuthority::Venue, &wanted, "   ")
            .expect_err("a permission with no evidence was recorded")
            .message()
            .to_string();
        assert!(
            unreferenced.contains("no reference"),
            "the refusal does not say what was missing: {unreferenced}"
        );
        assert_eq!(
            permissions.missing(&wanted).len(),
            3,
            "the refused permission was recorded anyway"
        );
        for (venue, product, jurisdiction, field) in [
            ("", "p", "j", "venue"),
            ("v", " ", "j", "product"),
            ("v", "p", "", "jurisdiction"),
        ] {
            let blank = CreationRequest::new(CreationAction::Create, venue, product, jurisdiction);
            let refusal = permissions
                .admit(&blank)
                .expect_err("a request naming nothing was admitted")
                .message()
                .to_string();
            assert!(
                refusal.contains(&format!("no {field}")),
                "the refusal does not name the blank {field}: {refusal}"
            );
            assert!(
                permissions
                    .record(CreationAuthority::Venue, &blank, "ref")
                    .is_err(),
                "a permission for a request naming no {field} was recorded"
            );
        }
        Ok(())
    }
}
