//! Capital granted to a strategy at an edge cell, and what it has used.
//!
//! An edge cell trades without asking the central plane, which is what makes
//! it fast. The envelope is how that stays safe: capital is granted in
//! advance, signed, bounded and expiring, so the worst a disconnected cell can
//! do is spend an amount somebody already approved.

use crate::signal::StrategyId;
use crate::validated::{
    Contract, MAX_BP, Validated, after, basis_points, named, non_negative, positive, refuse,
};
use crate::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Timestamp};
use serde::{Deserialize, Serialize};

/// What a strategy at a cell is permitted to commit.
///
/// Fields are private and the constructor is the only way in, because an
/// envelope that can be widened in place is not a limit. Widening means asking
/// the central plane for a new one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CapitalEnvelope {
    strategy: StrategyId,
    cell: String,
    /// The most that may be at risk at once.
    gross_limit: Decimal,
    /// The most any single order may commit.
    order_limit: Decimal,
    /// The loss at which the cell stops this strategy on its own authority.
    loss_limit: Decimal,
    /// Venues this grant is good at. An empty list is no venues, not all of
    /// them — the permissive reading of an empty list is how grants leak.
    venues: Vec<VenueId>,
    granted_at: Timestamp,
    expires_at: Timestamp,
    /// Who approved the grant, and the signature over its contents.
    approver: String,
    signature: String,
    /// R12-GOV-6120: leverage, product-class and regulated-function bounds,
    /// inside the signed payload when present. Absent on an envelope issued
    /// before the bounds existed, which still reads; such an envelope admits
    /// nothing through [`Self::admit_bounded`], the path that must check a
    /// product class or a leverage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bounds: Option<AuthorityBounds>,
}

/// What kind of exposure a grant authorises beyond its money limits
/// (R12-GOV-6120): how much leverage, in which product classes, and which
/// regulated functions the grantee may perform.
///
/// Every list is sorted and de-duplicated on construction so the signed
/// payload is a function of the set, not of the order somebody typed it in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorityBounds {
    /// Gross exposure over the envelope's gross limit; one is unlevered.
    pub max_leverage: Decimal,
    /// Product classes the grant may trade. Empty is none, never all.
    pub product_classes: Vec<String>,
    /// Regulated functions the grantee may perform. Empty is none.
    pub regulated_functions: Vec<String>,
}

impl AuthorityBounds {
    /// Refuse a leverage below one (a fraction is a smaller gross limit,
    /// stated in the wrong field), no product class, and a blank entry.
    pub fn new(
        max_leverage: Decimal,
        product_classes: Vec<String>,
        regulated_functions: Vec<String>,
    ) -> Result<Self> {
        if max_leverage < Decimal::ONE {
            return Err(Error::invalid(
                "max_leverage below one is a smaller gross limit; state that limit instead",
            ));
        }
        if product_classes.is_empty() {
            return Err(Error::invalid(
                "a bounded envelope names at least one product class; an empty list is none",
            ));
        }
        let tidy = |field: &str, mut list: Vec<String>| -> Result<Vec<String>> {
            if list
                .iter()
                .any(|s| s.trim().is_empty() || s.contains([',', '|']))
            {
                return Err(Error::invalid(format!(
                    "`{field}` holds a blank entry or one containing a separator"
                )));
            }
            list.sort();
            list.dedup();
            Ok(list)
        };
        Ok(Self {
            max_leverage,
            product_classes: tidy("product_classes", product_classes)?,
            regulated_functions: tidy("regulated_functions", regulated_functions)?,
        })
    }
}

impl CapitalEnvelope {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        strategy: StrategyId,
        cell: impl Into<String>,
        gross_limit: Decimal,
        order_limit: Decimal,
        loss_limit: Decimal,
        venues: Vec<VenueId>,
        granted_at: Timestamp,
        expires_at: Timestamp,
        approver: impl Into<String>,
        signature: impl Into<String>,
    ) -> Result<Self> {
        if gross_limit <= Decimal::ZERO {
            return Err(Error::invalid("a capital envelope needs a positive limit"));
        }
        if order_limit > gross_limit {
            return Err(Error::invalid(
                "a single order may not commit more than the whole envelope",
            ));
        }
        if expires_at <= granted_at {
            return Err(Error::invalid(
                "a capital envelope must expire after it is granted",
            ));
        }
        let approver = approver.into();
        if approver.trim().is_empty() {
            return Err(Error::denied("a capital envelope needs a named approver"));
        }
        Ok(Self {
            strategy,
            cell: cell.into(),
            gross_limit,
            order_limit,
            loss_limit,
            venues,
            granted_at,
            expires_at,
            approver,
            signature: signature.into(),
            bounds: None,
        })
    }

    /// This envelope with authority bounds, and its signature cleared: the
    /// bounds enter the signed payload, so a signature taken before them is
    /// over the wrong thing and must be taken again ([`Self::signed`]). An
    /// envelope that already carries bounds is refused: changing a bound is
    /// asking for a new envelope.
    pub fn bound(mut self, bounds: AuthorityBounds) -> Result<Self> {
        if self.bounds.is_some() {
            return Err(Error::denied(
                "this envelope is already bounded; a changed bound is a new envelope, signed anew",
            ));
        }
        self.bounds = Some(AuthorityBounds::new(
            bounds.max_leverage,
            bounds.product_classes,
            bounds.regulated_functions,
        )?);
        self.signature = String::new();
        Ok(self)
    }

    /// This envelope carrying `signature`. Whether it verifies is the
    /// verifier's question; this only attaches it.
    pub fn signed(mut self, signature: impl Into<String>) -> Self {
        self.signature = signature.into();
        self
    }

    pub fn bounds(&self) -> Option<&AuthorityBounds> {
        self.bounds.as_ref()
    }

    pub fn strategy(&self) -> &StrategyId {
        &self.strategy
    }

    pub fn cell(&self) -> &str {
        &self.cell
    }

    pub fn gross_limit(&self) -> Decimal {
        self.gross_limit
    }

    pub fn order_limit(&self) -> Decimal {
        self.order_limit
    }

    pub fn loss_limit(&self) -> Decimal {
        self.loss_limit
    }

    pub fn approver(&self) -> &str {
        &self.approver
    }

    pub fn signature(&self) -> &str {
        &self.signature
    }

    pub fn expires_at(&self) -> Timestamp {
        self.expires_at
    }

    pub fn is_live(&self, now: Timestamp) -> bool {
        now >= self.granted_at && now < self.expires_at
    }

    pub fn permits_venue(&self, venue: &VenueId) -> bool {
        self.venues.iter().any(|v| v == venue)
    }

    /// Every venue this grant is good at. Read-only: the list is part of the
    /// signed payload, and widening it means asking for a new grant.
    pub fn venues(&self) -> &[VenueId] {
        &self.venues
    }

    /// The bytes a signature is taken over.
    ///
    /// Every field that bounds what the cell may do. A signature that does not
    /// cover a limit is a signature over the wrong thing.
    ///
    /// The authority bounds are appended only when present, so an envelope
    /// issued before they existed signs and verifies exactly as it did.
    pub fn signing_payload(&self) -> String {
        let venues: Vec<&str> = self.venues.iter().map(VenueId::as_str).collect();
        let base = format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}",
            self.strategy.as_str(),
            self.cell,
            self.gross_limit,
            self.order_limit,
            self.loss_limit,
            venues.join(","),
            self.granted_at.as_secs(),
            self.expires_at.as_secs(),
            self.approver
        );
        match &self.bounds {
            None => base,
            Some(bounds) => format!(
                "{base}|bounds|{}|{}|{}",
                bounds.max_leverage,
                bounds.product_classes.join(","),
                bounds.regulated_functions.join(",")
            ),
        }
    }

    /// [`Self::admit`] for an order whose product class and leverage must be
    /// within the envelope's authority bounds (R12-GOV-6120). An envelope
    /// with no bounds refuses: it was never signed over a product class or a
    /// leverage, so it authorises neither.
    #[allow(clippy::too_many_arguments)]
    pub fn admit_bounded(
        &self,
        venue: &VenueId,
        notional: Decimal,
        used: &Utilisation,
        now: Timestamp,
        product_class: &str,
        leverage: Decimal,
        regulated_function: Option<&str>,
    ) -> CapitalAdmission {
        let Some(bounds) = &self.bounds else {
            return CapitalAdmission::Refused(
                "the envelope carries no authority bounds, so it authorises no product class \
                 and no leverage; ask for a bounded envelope"
                    .to_string(),
            );
        };
        if !bounds.product_classes.iter().any(|c| c == product_class) {
            return CapitalAdmission::Refused(format!(
                "product class {product_class} is outside the envelope's {}",
                bounds.product_classes.join(", ")
            ));
        }
        if leverage > bounds.max_leverage {
            return CapitalAdmission::Refused(format!(
                "leverage {leverage} exceeds the envelope's {}",
                bounds.max_leverage
            ));
        }
        if let Some(function) = regulated_function
            && !bounds.regulated_functions.iter().any(|f| f == function)
        {
            return CapitalAdmission::Refused(format!(
                "regulated function {function} is not one the envelope authorises"
            ));
        }
        self.admit(venue, notional, used, now)
    }
}

/// What a strategy has actually committed against its envelope.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Utilisation {
    pub gross_committed: Decimal,
    pub realised_loss: Decimal,
    pub orders_sent: u64,
}

/// The answer to "may this order go" — [`CapitalEnvelope::admit`]'s verdict.
///
/// Named `CapitalGrant` until R12-CONTRACT-6115, when the blueprint's
/// `CapitalGrant` contract (bounded deployable authority, [`CapitalGrant`]
/// below) needed the name. The two are different facts — a per-order verdict
/// and a standing grant of authority — and sharing a name meant a reader
/// searching for the grant contract found a three-arm enum and concluded it
/// was missing fields. The wire form is unchanged: serde writes the variant
/// names, never the type name, so journals written under the old name still
/// read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CapitalAdmission {
    /// Send it at the requested size.
    Full,
    /// Send it, but smaller. [`CapitalEnvelope::admit`] no longer constructs
    /// this: an order the envelope cannot take whole is refused whole. The
    /// variant is kept only so a journal or delta written before that change
    /// still deserialises; removing it is tracked separately.
    Reduced(Decimal),
    /// Do not send it.
    Refused(String),
}

impl CapitalAdmission {
    pub fn permitted_quantity(&self, requested: Decimal) -> Decimal {
        match self {
            Self::Full => requested,
            Self::Reduced(q) => *q,
            Self::Refused(_) => Decimal::ZERO,
        }
    }

    pub const fn is_refused(&self) -> bool {
        matches!(self, Self::Refused(_))
    }
}

impl CapitalEnvelope {
    /// Decide an order against this envelope and what has already been used.
    ///
    /// Every refusal names which bound was hit, because "refused" without a
    /// reason is the least actionable message an execution system can produce.
    pub fn admit(
        &self,
        venue: &VenueId,
        notional: Decimal,
        used: &Utilisation,
        now: Timestamp,
    ) -> CapitalAdmission {
        if !self.is_live(now) {
            return CapitalAdmission::Refused("the capital envelope has expired".to_string());
        }
        if !self.permits_venue(venue) {
            return CapitalAdmission::Refused(format!(
                "the envelope does not cover venue {}",
                venue.as_str()
            ));
        }
        if used.realised_loss >= self.loss_limit {
            return CapitalAdmission::Refused(format!(
                "realised loss {} reached the {} limit",
                used.realised_loss, self.loss_limit
            ));
        }
        let headroom = self.gross_limit - used.gross_committed;
        if headroom <= Decimal::ZERO {
            return CapitalAdmission::Refused(format!(
                "the {} gross limit is fully committed",
                self.gross_limit
            ));
        }
        // Refused whole, never reduced to fit (CAPITAL-025). Answering a
        // larger order with a smaller size sends something the strategy never
        // asked for: the order was sized against a thesis, and a fraction of
        // it is a different trade nobody decided. `Reduced` stays declared so
        // a journal written before this change still reads; nothing here
        // constructs it.
        if notional > self.order_limit {
            return CapitalAdmission::Refused(format!(
                "order notional {notional} exceeds the {} order limit",
                self.order_limit
            ));
        }
        if notional > headroom {
            return CapitalAdmission::Refused(format!(
                "order notional {notional} exceeds the {headroom} of the {} gross limit left",
                self.gross_limit
            ));
        }
        CapitalAdmission::Full
    }
}
// ---- The §24.9 / §27 CapitalGrant contract -----------------------------

/// What the grant's capital is held as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollateralClass {
    Cash,
    GovernmentBonds,
    Equities,
    DigitalAssets,
}

/// The execution mode a grant authorises. **One variant, on purpose**: the
/// platform is paper-trading only (ADR 0003), and a grant that could name a
/// live mode would be one more place that boundary had to be held. A record
/// carrying any other value fails to decode. Adding a variant is ADR 0108's
/// decision, not a contract change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantMode {
    Paper,
}

/// The fields of a [`CapitalGrant`]. This unifies the two blueprint
/// statements of the contract: §24.9 (R12-CONTRACT-6115: amount, strategy,
/// region, venue, risk budget, liquidity reserve, expiry) and §27
/// (R12-CONTRACT-7066: owner/mandate, collateral class, leverage, drawdown
/// and liquidity constraints).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapitalGrantFields {
    pub schema_version: u32,
    pub grant_id: String,
    pub mode: GrantMode,
    /// Who owns the capital.
    pub owner: String,
    /// The mandate the grant is made under.
    pub mandate: String,
    pub strategy: StrategyId,
    pub region: String,
    pub venue: VenueId,
    /// The capital granted, in the account currency.
    pub amount: Decimal,
    pub collateral: CollateralClass,
    /// Gross exposure over `amount`; at least one (one is unlevered).
    pub max_leverage: Decimal,
    /// The most the grant may lose before it is withdrawn; no more than
    /// `amount` — loss beyond the capital granted is not grantable.
    pub risk_budget: Decimal,
    /// Peak-to-trough drawdown that stops the strategy, in basis points.
    pub max_drawdown_bp: u32,
    /// Capital held back and never deployed; less than `amount`.
    pub liquidity_reserve: Decimal,
    /// The largest share of a venue's average daily volume any position may
    /// be, in basis points.
    pub max_adv_share_bp: u32,
    pub granted_at: Timestamp,
    pub expires_at: Timestamp,
}

/// Bounded deployable authority (§24.9 / §27). Built only through
/// [`Validated::new`]; deserialisation goes through the same refusals.
pub type CapitalGrant = Validated<CapitalGrantFields>;

impl Contract for CapitalGrantFields {
    const NAME: &'static str = "CapitalGrant";
    const SCHEMA_VERSION: u32 = 1;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Refuses blank names; a non-positive amount; leverage below one; a
    /// risk budget that is non-positive or exceeds the amount; a drawdown
    /// limit of zero (a strategy stopped before it starts) or above 100%; a
    /// negative reserve or one that leaves nothing to deploy; an ADV share of
    /// zero or above 100%; and an expiry not after the grant. Nothing is
    /// clamped: a grant widened in transit is not a limit.
    fn check(&self) -> Result<()> {
        let c = Self::NAME;
        named(c, "grant_id", &self.grant_id)?;
        named(c, "owner", &self.owner)?;
        named(c, "mandate", &self.mandate)?;
        named(c, "strategy", self.strategy.as_str())?;
        named(c, "region", &self.region)?;
        named(c, "venue", self.venue.as_str())?;
        positive(c, "amount", self.amount)?;
        if self.max_leverage < Decimal::ONE {
            return refuse(
                c,
                format!(
                    "states leverage {}; one is unlevered and less is meaningless",
                    self.max_leverage
                ),
            );
        }
        positive(c, "risk_budget", self.risk_budget)?;
        if self.risk_budget > self.amount {
            return refuse(c, "budgets more risk than the capital it grants");
        }
        basis_points(c, "max_drawdown_bp", self.max_drawdown_bp)?;
        if self.max_drawdown_bp == 0 {
            return refuse(
                c,
                "has a zero drawdown limit; the strategy would stop on its first tick",
            );
        }
        non_negative(c, "liquidity_reserve", self.liquidity_reserve)?;
        if self.liquidity_reserve >= self.amount {
            return refuse(c, "reserves the whole amount; nothing is left to deploy");
        }
        if self.max_adv_share_bp == 0 || self.max_adv_share_bp > MAX_BP {
            return refuse(
                c,
                format!(
                    "states an ADV share of {} bp; give one in (0, {MAX_BP}]",
                    self.max_adv_share_bp
                ),
            );
        }
        after(
            c,
            "expires_at",
            self.expires_at,
            "granted_at",
            self.granted_at,
        )
    }
}

impl CapitalGrant {
    /// Whether the grant is in force at `now`: granted and not expired.
    pub fn is_live(&self, now: Timestamp) -> bool {
        self.granted_at <= now && now < self.expires_at
    }

    /// The capital that may be deployed: the amount less the reserve. Never
    /// negative — [`Contract::check`] refused a reserve at or above the
    /// amount.
    pub fn deployable(&self) -> Decimal {
        self.amount - self.liquidity_reserve
    }
}
