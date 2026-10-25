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
    /// R12-CAPITAL-4088: the liquidity and leverage bounds, inside the
    /// signed payload when present. Absent on an envelope issued before they
    /// existed; such an envelope still verifies, and a cell built with
    /// `CellConfig::with_exposure_bounds_required` refuses every order
    /// against it rather than reading the absence as "unbounded".
    ///
    /// Boxed so the envelope a cell holds by value in every channel slot and
    /// deployment does not grow by the bounds' size for every envelope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    exposure: Option<Box<ExposureBounds>>,
    /// ADR 0126 decision 2 and 3: the peak-to-trough drawdown stop, and the
    /// liquidity reserve and ADV share when the issuer states them, inside
    /// the signed payload when present. Absent on an envelope issued before
    /// the block existed, which still verifies and is bounded by its loss
    /// limit alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    drawdown: Option<Box<DrawdownBounds>>,
}

/// The fields of [`DrawdownBounds`] (ADR 0126, R12-CONTRACT-7066,
/// R12-CAPITAL-4088).
///
/// The loss limit stops a strategy once its realised loss reaches it, and
/// that number is net: a strategy that made a hundred and then lost a
/// hundred has lost nothing by it, while the desk watched a peak-to-trough
/// fall the size of the loss limit. The drawdown bound is that fall, so a
/// grant can stop a strategy that has given back what it made and not only
/// one that is under water. The reserve and the ADV share are what the
/// `CapitalGrant` the ledger books used to *write* (zero and the whole);
/// stated here, the grant reads them instead.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DrawdownBoundsFields {
    pub schema_version: u32,
    /// The realised peak-to-trough fall, in the account currency, at which
    /// the cell stops the strategy. Never above the loss limit, which
    /// [`CapitalEnvelope::with_drawdown`] checks.
    pub max_drawdown: Decimal,
    /// Capital the grant must hold back from deployment, when the issuer
    /// states one; `None` is "not stated", never zero.
    pub liquidity_reserve: Option<Decimal>,
    /// The largest share of average daily volume the grant may take, in
    /// basis points, when the issuer states one; `None` is "not stated".
    pub max_adv_share_bp: Option<u32>,
}

/// Validated drawdown bounds. Built only through [`Validated::new`];
/// deserialisation runs the same refusals, so a bound widened on the wire
/// is refused at decode rather than enforced as sent.
pub type DrawdownBounds = Validated<DrawdownBoundsFields>;

impl Contract for DrawdownBoundsFields {
    const NAME: &'static str = "DrawdownBounds";
    const SCHEMA_VERSION: u32 = 1;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Refuses a drawdown that is not positive (a zero stop is a halt
    /// written as a limit), a negative reserve and an ADV share of zero or
    /// above the whole volume. Nothing is clamped.
    fn check(&self) -> Result<()> {
        let c = Self::NAME;
        positive(c, "max_drawdown", self.max_drawdown)?;
        if let Some(reserve) = self.liquidity_reserve {
            non_negative(c, "liquidity_reserve", reserve)?;
        }
        if let Some(share) = self.max_adv_share_bp
            && (share == 0 || share > MAX_BP)
        {
            return refuse(
                c,
                format!("states an ADV share of {share} bp; give one in (0, {MAX_BP}]"),
            );
        }
        Ok(())
    }
}

impl DrawdownBounds {
    /// The bounds at the current schema version.
    pub fn bounded(
        max_drawdown: Decimal,
        liquidity_reserve: Option<Decimal>,
        max_adv_share_bp: Option<u32>,
    ) -> Result<Self> {
        Validated::new(DrawdownBoundsFields {
            schema_version: DrawdownBoundsFields::SCHEMA_VERSION,
            max_drawdown,
            liquidity_reserve,
            max_adv_share_bp,
        })
    }
}

/// The fields of [`ExposureBounds`] (R12-CAPITAL-4088).
///
/// The envelope's money limits say how much a cell may commit; they say
/// nothing about *where* or *on what*. Two positions inside the same gross
/// limit can differ in everything that matters to a disconnected cell: one
/// was built at a tenth of a venue's visible depth and can be exited, the
/// other took the whole book and cannot; one is carried on the equity the
/// grant was backed by, the other on that equity after the loss the cell has
/// already taken. These two bounds are what the cell checks for each, before
/// an order exists.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExposureBoundsFields {
    pub schema_version: u32,
    /// The largest share of the visible depth on the side an order takes
    /// that the order may ask for, in basis points of that depth.
    pub max_participation_bp: u32,
    /// The most gross the grant may carry over its remaining equity —
    /// `capital` less the realised loss — after the order; one is unlevered.
    pub max_leverage: Decimal,
    /// The equity the grant is backed by: what the leverage is measured
    /// against. Never less than the gross limit over the leverage, which
    /// [`CapitalEnvelope::with_exposure`] checks.
    pub capital: Decimal,
}

/// Validated liquidity and leverage bounds. Built only through
/// [`Validated::new`]; deserialisation runs the same refusals, so a bound
/// widened on the wire is refused at decode rather than enforced as sent.
pub type ExposureBounds = Validated<ExposureBoundsFields>;

impl Contract for ExposureBoundsFields {
    const NAME: &'static str = "ExposureBounds";
    const SCHEMA_VERSION: u32 = 1;

    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Refuses a participation of zero (a grant that may take no liquidity
    /// is a grant of nothing, which should be refused at issue and say so)
    /// or above the whole visible side, a leverage below one, and a
    /// non-positive capital. Nothing is clamped.
    fn check(&self) -> Result<()> {
        let c = Self::NAME;
        if self.max_participation_bp == 0 || self.max_participation_bp > MAX_BP {
            return refuse(
                c,
                format!(
                    "states a participation of {} bp of visible depth; give one in (0, {MAX_BP}]",
                    self.max_participation_bp
                ),
            );
        }
        if self.max_leverage < Decimal::ONE {
            return refuse(
                c,
                format!(
                    "states leverage {}; one is unlevered and less is a smaller gross limit \
                     stated in the wrong field",
                    self.max_leverage
                ),
            );
        }
        positive(c, "capital", self.capital)
    }
}

impl ExposureBounds {
    /// The bounds at the current schema version.
    pub fn bounded(
        max_participation_bp: u32,
        max_leverage: Decimal,
        capital: Decimal,
    ) -> Result<Self> {
        Validated::new(ExposureBoundsFields {
            schema_version: ExposureBoundsFields::SCHEMA_VERSION,
            max_participation_bp,
            max_leverage,
            capital,
        })
    }
}

/// Which exposure bound an order breached, so a cell journals it under the
/// gate that names it rather than under the envelope's money gate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExposureBreach {
    /// The envelope carries no exposure bounds at all.
    Unbounded(String),
    /// The order asks for more of the visible depth than the grant allows,
    /// or there is no visible depth to measure it against.
    Liquidity(String),
    /// The order would carry the grant past its leverage on remaining
    /// equity, or there is no remaining equity to carry it on.
    Leverage(String),
}

impl ExposureBreach {
    pub fn reason(&self) -> &str {
        match self {
            Self::Unbounded(reason) | Self::Liquidity(reason) | Self::Leverage(reason) => reason,
        }
    }
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
    /// Gross committed after the order over capital less realised loss — the
    /// one measure [`CapitalEnvelope::check_exposure`] computes (ADR 0155
    /// section 4), and not gross over the gross limit, which the money limits
    /// already cap at one and so could never fire. One is unlevered. When an
    /// envelope carries both this block and [`ExposureBounds`] the two
    /// `max_leverage` values must be equal, refused at construction.
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
            exposure: None,
            drawdown: None,
        })
    }

    /// This envelope with liquidity and leverage bounds, and its signature
    /// cleared: the bounds enter the signed payload, so a signature taken
    /// before them is over the wrong thing. Refuses an envelope already
    /// carrying exposure bounds — a changed bound is a new envelope — and a
    /// gross limit the leverage could never carry on the stated capital,
    /// which would be a grant whose two bounds contradict each other.
    pub fn with_exposure(mut self, exposure: ExposureBounds) -> Result<Self> {
        if self.exposure.is_some() {
            return Err(Error::denied(
                "this envelope already carries exposure bounds; a changed bound is a new \
                 envelope, signed anew",
            ));
        }
        if let Some(bounds) = &self.bounds
            && bounds.max_leverage != exposure.max_leverage
        {
            return Err(Error::invalid(format!(
                "the exposure bounds state a leverage of {} and the authority bounds {}; there \
                 is one leverage measure, so state one claim about it",
                exposure.max_leverage, bounds.max_leverage
            )));
        }
        let carried = exposure
            .capital
            .checked_mul(exposure.max_leverage)
            .ok_or_else(|| Error::numeric("the envelope's capital times its leverage overflows"))?;
        if self.gross_limit > carried {
            return Err(Error::invalid(format!(
                "a gross limit of {} cannot be carried at leverage {} on capital {}; raise the \
                 capital or lower the gross limit rather than issue a grant whose bounds \
                 contradict",
                self.gross_limit, exposure.max_leverage, exposure.capital
            )));
        }
        self.exposure = Some(Box::new(exposure));
        self.signature = String::new();
        Ok(self)
    }

    pub fn exposure(&self) -> Option<&ExposureBounds> {
        self.exposure.as_deref()
    }

    /// This envelope with a drawdown bounds block, and its signature
    /// cleared: the block enters the signed payload. Refuses an envelope
    /// already carrying one (a changed bound is a new envelope) and any
    /// block [`Self::check_drawdown_consistent`] refuses.
    pub fn with_drawdown(mut self, drawdown: DrawdownBounds) -> Result<Self> {
        if self.drawdown.is_some() {
            return Err(Error::denied(
                "this envelope already carries drawdown bounds; a changed bound is a new \
                 envelope, signed anew",
            ));
        }
        self.drawdown = Some(Box::new(drawdown));
        self.check_drawdown_consistent()?;
        self.signature = String::new();
        Ok(self)
    }

    pub fn drawdown(&self) -> Option<&DrawdownBounds> {
        self.drawdown.as_deref()
    }

    /// The cross-field half of the drawdown block's validation, which a
    /// block's own decode cannot see: a drawdown stop above the loss limit
    /// is not a bound (the loss limit fires first on every path to it), and
    /// a reserve above the gross limit holds back more than was granted.
    /// Called by [`Self::with_drawdown`] and by the cell when it verifies an
    /// envelope, so an envelope decoded off the wire meets the same refusal
    /// as one built in process.
    pub fn check_drawdown_consistent(&self) -> Result<()> {
        let Some(drawdown) = self.drawdown.as_deref() else {
            return Ok(());
        };
        if drawdown.max_drawdown > self.loss_limit {
            return Err(Error::invalid(format!(
                "a drawdown stop of {} is looser than the {} loss limit, so it could never fire; \
                 state one at or below the loss limit",
                drawdown.max_drawdown, self.loss_limit
            )));
        }
        if let Some(reserve) = drawdown.liquidity_reserve
            && reserve > self.gross_limit
        {
            return Err(Error::invalid(format!(
                "a liquidity reserve of {reserve} holds back more than the {} gross limit granted",
                self.gross_limit
            )));
        }
        Ok(())
    }

    /// Refuse a further order once `drawdown` — the strategy's realised
    /// peak-to-trough fall within this grant, as the cell measures it — has
    /// reached the bound. `Ok` for an envelope with no drawdown block: it
    /// was never signed over one, and its loss limit still binds.
    pub fn check_drawdown(&self, drawdown: Decimal) -> std::result::Result<(), String> {
        match self.drawdown.as_deref() {
            Some(bounds) if drawdown >= bounds.max_drawdown => Err(format!(
                "a realised drawdown of {drawdown} reached the grant's {} drawdown stop",
                bounds.max_drawdown
            )),
            _ => Ok(()),
        }
    }

    /// Check an order against the envelope's liquidity and leverage bounds
    /// (R12-CAPITAL-4088), separately from [`Self::admit`] so the caller can
    /// journal a breach under the gate that names it.
    ///
    /// `quantity` is what the order asks for and `visible_depth` the size
    /// resting on the side it takes; `notional` is its cash value. Visible
    /// depth of zero refuses: participation in a book that shows nothing
    /// cannot be measured, and an order the cell cannot measure is not one it
    /// may assume is small. Leverage is the gross committed after the order
    /// over the capital less the realised loss; equity at or below zero
    /// refuses whatever the order.
    pub fn check_exposure(
        &self,
        quantity: Decimal,
        visible_depth: Decimal,
        notional: Decimal,
        used: &Utilisation,
    ) -> std::result::Result<(), ExposureBreach> {
        let Some(bounds) = &self.exposure else {
            return Err(ExposureBreach::Unbounded(
                "the envelope carries no liquidity or leverage bound, so it authorises no \
                 exposure this cell can check; ask the centre for a bounded envelope"
                    .to_string(),
            ));
        };
        if !visible_depth.is_positive() {
            return Err(ExposureBreach::Liquidity(format!(
                "the book shows {visible_depth} on the side the order takes, so its \
                 participation cannot be measured against the {} bp bound",
                bounds.max_participation_bp
            )));
        }
        let allowed = visible_depth
            .checked_mul(Decimal::from_int(i64::from(bounds.max_participation_bp)))
            .and_then(|scaled| scaled.checked_div(Decimal::from_int(i64::from(MAX_BP))))
            .ok_or_else(|| {
                ExposureBreach::Liquidity(
                    "the participation bound cannot be computed on this depth".to_string(),
                )
            })?;
        if quantity > allowed {
            return Err(ExposureBreach::Liquidity(format!(
                "the order asks for {quantity} against {visible_depth} visible, above the {allowed} \
                 that {} bp of it allows",
                bounds.max_participation_bp
            )));
        }
        let equity = bounds
            .capital
            .checked_sub(used.realised_loss)
            .filter(|equity| equity.is_positive())
            .ok_or_else(|| {
                ExposureBreach::Leverage(format!(
                    "realised loss {} leaves no equity of the {} capital to carry gross on",
                    used.realised_loss, bounds.capital
                ))
            })?;
        let gross_after = used.gross_committed.checked_add(notional).ok_or_else(|| {
            ExposureBreach::Leverage("the gross after the order overflows".into())
        })?;
        let carried = equity.checked_mul(bounds.max_leverage).ok_or_else(|| {
            ExposureBreach::Leverage("the leverage bound cannot be computed".to_string())
        })?;
        if gross_after > carried {
            return Err(ExposureBreach::Leverage(format!(
                "gross of {gross_after} after the order exceeds the {carried} that leverage {} \
                 allows on equity of {equity}",
                bounds.max_leverage
            )));
        }
        Ok(())
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
        if let Some(exposure) = &self.exposure
            && exposure.max_leverage != bounds.max_leverage
        {
            return Err(Error::invalid(format!(
                "the authority bounds state a leverage of {} and the exposure bounds {}; there \
                 is one leverage measure, so state one claim about it",
                bounds.max_leverage, exposure.max_leverage
            )));
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

    /// When the grant took effect. Part of the signed payload, and the one
    /// instant that tells two grants to the same book apart: the ledger
    /// funding an envelope becomes (`ledger_operation::OperationKind::CapitalGrantFunded`)
    /// is keyed on it, so a renewal is a second funding and a resend of the
    /// same grant is the same record.
    pub fn granted_at(&self) -> Timestamp {
        self.granted_at
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
        let base = match &self.bounds {
            None => base,
            Some(bounds) => format!(
                "{base}|bounds|{}|{}|{}",
                bounds.max_leverage,
                bounds.product_classes.join(","),
                bounds.regulated_functions.join(",")
            ),
        };
        // The schema version is in the payload so a later revision of the
        // bounds cannot be verified as this one: a cell that reads version 1
        // and is handed version 2's bytes refuses the signature rather than
        // enforcing a subset of what was signed.
        let base = match &self.exposure {
            None => base,
            Some(exposure) => format!(
                "{base}|exposure.v{}|{}|{}|{}",
                exposure.schema_version,
                exposure.max_participation_bp,
                exposure.max_leverage,
                exposure.capital
            ),
        };
        // An unstated reserve or ADV share is `-`, never `0`: zero is a
        // statement, and a payload that spelled "not stated" as zero would
        // sign the two as one claim.
        match &self.drawdown {
            None => base,
            Some(drawdown) => format!(
                "{base}|drawdown.v{}|{}|{}|{}",
                drawdown.schema_version,
                drawdown.max_drawdown,
                drawdown
                    .liquidity_reserve
                    .map_or_else(|| "-".to_string(), |reserve| reserve.to_string()),
                drawdown
                    .max_adv_share_bp
                    .map_or_else(|| "-".to_string(), |share| share.to_string())
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
        if let Err(breach) = self.check_authority(product_class, leverage, regulated_function) {
            return CapitalAdmission::Refused(breach.reason().to_string());
        }
        self.admit(venue, notional, used, now)
    }

    /// The authority bounds alone: product class, leverage and regulated
    /// function, with no money limit. `Err` names which bound refused.
    /// Separate from [`Self::admit_bounded`] so a caller that has already
    /// run the money limits (the cell does, before any intent exists) need
    /// not run them twice.
    pub fn check_authority(
        &self,
        product_class: &str,
        leverage: Decimal,
        regulated_function: Option<&str>,
    ) -> std::result::Result<(), AuthorityBreach> {
        let Some(bounds) = &self.bounds else {
            return Err(AuthorityBreach::Unbounded(
                "the envelope carries no authority bounds, so it authorises no product class \
                 and no leverage; ask for a bounded envelope"
                    .to_string(),
            ));
        };
        if !bounds.product_classes.iter().any(|c| c == product_class) {
            return Err(AuthorityBreach::Product(format!(
                "product class {product_class} is outside the envelope's {}",
                bounds.product_classes.join(", ")
            )));
        }
        if leverage > bounds.max_leverage {
            return Err(AuthorityBreach::Leverage(format!(
                "leverage {leverage} exceeds the envelope's {}",
                bounds.max_leverage
            )));
        }
        if let Some(function) = regulated_function
            && !bounds.regulated_functions.iter().any(|f| f == function)
        {
            return Err(AuthorityBreach::Function(format!(
                "regulated function {function} is not one the envelope authorises"
            )));
        }
        Ok(())
    }
}

/// Which authority bound refused an order (ADR 0155 section 2 to 4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthorityBreach {
    /// The envelope carries no authority bounds at all.
    Unbounded(String),
    /// The instrument's product class is not one the grant names.
    Product(String),
    /// The order's leverage is above the grant's.
    Leverage(String),
    /// The order performs a regulated function the grant does not name.
    Function(String),
}

impl AuthorityBreach {
    /// The refusal text, naming the bound.
    pub fn reason(&self) -> &str {
        match self {
            Self::Unbounded(reason)
            | Self::Product(reason)
            | Self::Leverage(reason)
            | Self::Function(reason) => reason,
        }
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
