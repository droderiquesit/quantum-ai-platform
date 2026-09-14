//! What collateralises what, across margin regimes (blueprint §25.6).
//!
//! A per-book margin requirement — [`crate::margin::MarginModel`] — answers
//! "how much must be posted against this book". It cannot answer the question
//! that decides whether the book survives a bad morning: *which* asset stands
//! behind *which* obligation, at which venue, and what happens to the second
//! obligation when the first one is closed out.
//!
//! Four things this graph holds that a scalar requirement cannot.
//!
//! **A margin domain nets inside itself and nowhere else.** Excess collateral
//! at one venue does not cure a call at another unless somebody signed an
//! agreement saying it does. A book reported as one requirement against one
//! collateral balance is a book assumed to be cross-margined everywhere,
//! which is the rarest arrangement in the market and the most dangerous one
//! to assume. [`MarginRegime::Isolated`] is therefore the `Default` and the
//! only regime a caller gets without naming the other one.
//!
//! **An asset pledged twice is one asset.** [`CollateralGraph::build`]
//! refuses a pledge set whose face amounts against one asset exceed the
//! asset's mark, because that arithmetic is how a collateral balance comes to
//! be larger than the collateral. The refusal names the asset and the total.
//!
//! **Re-pledged collateral is counted more than once on purpose, and the
//! count is published.** Rehypothecation is a legitimate arrangement; a
//! rehypothecation nobody has counted is not. [`CollateralGraph::reuse`]
//! returns every asset whose value reaches more than one domain's coverage,
//! with the domains named and the phantom amount stated. A re-pledge *cycle*
//! is refused outright: value that flows A → B → A is one asset standing
//! behind itself.
//!
//! **Collateral that falls with what it backs is not collateral.** Each asset
//! and each domain carries driver labels; [`DomainCoverage::correlated`] is
//! the part of a domain's cover that shares a driver with the domain's own
//! exposure, and [`DomainCoverage::uncorrelated_excess`] is the excess once
//! that part is disbelieved. This is the margin spiral as arithmetic: a
//! domain whose excess is positive and whose uncorrelated excess is negative
//! is solvent only while the thing it is long stays up.
//!
//! And [`CollateralGraph::cascade`] is the question asked before the event
//! rather than after: a forced close-out at one venue consumes the assets
//! pledged there, every other domain counting on those same assets loses that
//! cover, and the ones that fall below maintenance are closed out in turn. It
//! terminates because a domain is closed out at most once — a property of the
//! algorithm, not an iteration cap chosen to stop a runaway.
//!
//! # What this module is not
//!
//! It refuses nothing at trade time and sizes nothing. It is a read, in the
//! shape [`crate::margin`] already is: a requirement and a coverage are facts
//! about a book, and what a desk does about them is a decision with a person
//! in it. There is no path from here to an order.
//!
//! # Money and statistics
//!
//! Every mark, haircut, pledge, requirement and consumed amount here is
//! [`Decimal`]. The only `f64` are ratios of two `Decimal` sums —
//! [`DomainCoverage::correlated_share`] and [`DomainCoverage::utilisation`] —
//! and the crossing is marked where it happens. Iteration is over
//! [`BTreeMap`] and [`BTreeSet`] throughout, so a cascade replayed over the
//! same graph names the same venues in the same order.

use qip_contracts::venue::VenueId;
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Whether a venue nets exposure across what it holds, or ring-fences each
/// obligation.
///
/// [`MarginRegime::Isolated`] is the `Default` and there is no constructor
/// that produces the other arm implicitly. Portfolio margin is a term of a
/// specific agreement with a specific counterparty; assuming it is how a book
/// comes to look over-collateralised on a spreadsheet and under-collateralised
/// at the close.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum MarginRegime {
    /// Each obligation stands alone; cover held elsewhere does not reach it.
    #[default]
    Isolated,
    /// The venue nets across everything it holds for this account.
    Portfolio,
}

impl MarginRegime {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Isolated => "isolated",
            Self::Portfolio => "portfolio",
        }
    }
}

/// Whether collateral posted at a venue may be reused by that venue.
///
/// [`Rehypothecation::Forbidden`] is the `Default`, for the reason the regime
/// defaults to isolated: permission to reuse is a clause somebody signed, and
/// a model that assumes it produces a collateral pool larger than the assets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Rehypothecation {
    /// Posted collateral is segregated and cannot be re-pledged onward.
    #[default]
    Forbidden,
    /// The venue may re-pledge what it holds.
    Permitted,
}

impl Rehypothecation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Forbidden => "forbidden",
            Self::Permitted => "permitted",
        }
    }
}

/// One thing that can stand behind an obligation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CollateralAsset {
    /// Stable identity — an instrument symbol, or a cash balance's name.
    pub id: String,
    /// Mark, gross of the haircut.
    pub value: Decimal,
    /// The fraction of `value` the taker will not lend against, in `[0, 1)`.
    pub haircut: Decimal,
    /// What moves this asset. Anything carrying a driver a domain also
    /// carries falls with that domain's exposure. `None` means nobody has
    /// said, and an unlabelled asset counts as uncorrelated —
    /// [`CollateralGraph::unlabelled_value`] publishes how much of the graph
    /// that is, rather than folding the gap into a risk number where no
    /// reader could find it.
    pub driver: Option<String>,
}

impl CollateralAsset {
    /// Mark after the haircut: what the taker will actually lend against.
    pub fn lendable(&self) -> Result<Decimal> {
        let retained = Decimal::ONE - self.haircut;
        self.value
            .checked_mul(retained)
            .ok_or_else(|| Error::numeric(format!("the lendable value of {} overflowed", self.id)))
    }
}

/// One venue's account: a margin domain that nets inside itself.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarginDomain {
    pub venue: VenueId,
    pub regime: MarginRegime,
    pub rehypothecation: Rehypothecation,
    /// What must be posted before new risk may be taken here.
    pub initial: Decimal,
    /// What must stay posted, below which the venue closes the account out.
    pub maintenance: Decimal,
    /// What moves the exposure this domain carries. Cover sharing one of
    /// these is cover that falls with what it backs.
    pub drivers: BTreeSet<String>,
}

/// An asset standing behind one domain's obligation, at a face amount.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pledge {
    pub asset: String,
    pub venue: VenueId,
    /// Face committed here, before the haircut. The sum of an asset's pledges
    /// may not exceed its mark.
    pub amount: Decimal,
}

/// Collateral held at one venue and re-pledged to margin another.
///
/// The rare and dangerous arrangement §25.6 names. It is never implied: a
/// link exists only because a caller constructed one, the source domain must
/// permit rehypothecation, and the destination must be portfolio-margined —
/// an isolated domain by definition does not accept cover held elsewhere.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CrossVenueLink {
    pub from: VenueId,
    pub to: VenueId,
    /// Face re-pledged onward. May not exceed what is pledged at `from`.
    pub amount: Decimal,
}

/// What a domain has against what it owes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DomainCoverage {
    pub venue: VenueId,
    pub regime: MarginRegime,
    /// Whether this venue may re-pledge what is posted here. Carried on the
    /// coverage rather than left on the domain, because a coverage line that
    /// does not say whether the venue may lend your collateral on is missing
    /// the fact §25.6's third row exists for: the cover is the same figure
    /// and the counterparty risk behind it is not.
    pub rehypothecation: Rehypothecation,
    pub initial: Decimal,
    pub maintenance: Decimal,
    /// Post-haircut value pledged directly here.
    pub own: Decimal,
    /// Post-haircut value reaching here only through a re-pledge.
    pub rehypothecated: Decimal,
    /// The part of `own + rehypothecated` whose asset shares a driver with
    /// this domain's own exposure.
    pub correlated: Decimal,
}

impl DomainCoverage {
    /// Everything standing behind this domain, post-haircut.
    pub fn effective(&self) -> Decimal {
        self.own + self.rehypothecated
    }

    /// Cover above maintenance. Negative is a close-out.
    pub fn excess(&self) -> Decimal {
        self.effective() - self.maintenance
    }

    /// Whether the venue would close this account out right now.
    pub fn is_call(&self) -> bool {
        self.excess().is_negative()
    }

    /// Whether new risk may be taken here without posting more.
    pub fn can_open(&self) -> bool {
        self.effective() >= self.initial
    }

    /// Excess counting only the cover that does not fall with what it backs.
    ///
    /// The figure §25.6's last row exists for. A domain whose `excess` is
    /// positive and whose `uncorrelated_excess` is negative is collateralised
    /// by its own thesis: the move that makes the position a loss is the move
    /// that makes the cover insufficient, and both arrive on one morning.
    pub fn uncorrelated_excess(&self) -> Decimal {
        self.effective() - self.correlated - self.maintenance
    }

    /// Share of the cover that shares a driver with the exposure.
    ///
    /// Money → statistic: two `Decimal` sums become one ratio, because what
    /// is asked is a proportion and not an amount. No cover has no share
    /// rather than a fabricated one.
    pub fn correlated_share(&self) -> f64 {
        let effective = self.effective();
        if !effective.is_positive() {
            return 0.0;
        }
        self.correlated.to_f64() / effective.to_f64()
    }

    /// Maintenance as a fraction of cover. Money → statistic, as above.
    /// Infinite where there is a requirement and no cover, which is a
    /// distinct fact from a merely high ratio.
    pub fn utilisation(&self) -> f64 {
        let effective = self.effective();
        if !effective.is_positive() {
            return if self.maintenance.is_positive() {
                f64::INFINITY
            } else {
                0.0
            };
        }
        self.maintenance.to_f64() / effective.to_f64()
    }

    pub fn describe(&self) -> String {
        format!(
            "{} ({} margin, rehypothecation {}) holds {} against {} maintenance, {} of it \
             falling with the book it backs; {}",
            self.venue,
            self.regime.as_str(),
            self.rehypothecation.as_str(),
            self.effective(),
            self.maintenance,
            self.correlated,
            if self.is_call() {
                "a close-out"
            } else if self.uncorrelated_excess().is_negative() {
                "covered only by collateral correlated with its own exposure"
            } else if self.can_open() {
                "room to open"
            } else {
                "held, but no room to open"
            }
        )
    }
}

/// One asset whose value is counted by more than one domain.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CollateralReuse {
    pub asset: String,
    /// The asset's own mark, gross of haircut.
    pub value: Decimal,
    /// The asset's own post-haircut value: what it is actually worth as
    /// cover, once, to whoever holds it.
    pub lendable: Decimal,
    /// Domains counting it, in venue order.
    pub domains: Vec<VenueId>,
    /// Post-haircut value counted across those domains added up. Above
    /// `lendable` is the whole finding.
    pub counted: Decimal,
}

impl CollateralReuse {
    /// Counted cover above what the asset is worth: collateral that exists in
    /// the arithmetic and not in the account.
    pub fn phantom(&self) -> Decimal {
        (self.counted - self.lendable).max(Decimal::ZERO)
    }

    pub fn describe(&self) -> String {
        format!(
            "{} is worth {} as cover and is counted for {} across {}; {} of it is in the \
             arithmetic and not in the account",
            self.asset,
            self.lendable,
            self.counted,
            self.domains
                .iter()
                .map(VenueId::to_string)
                .collect::<Vec<_>>()
                .join(", "),
            self.phantom()
        )
    }
}

/// One venue closed out, and what it cost the rest of the graph.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CascadeStep {
    pub venue: VenueId,
    /// How far below maintenance this domain was when it was closed out. The
    /// seed's is zero where the seed was solvent: the seed is closed out
    /// because the caller asked what would happen if it were, not because it
    /// had to be.
    pub shortfall: Decimal,
    /// Post-haircut cover that stopped standing behind anything when this
    /// domain's assets were sold.
    pub collateral_consumed: Decimal,
}

/// A forced close-out at one venue, followed to its fixed point.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LiquidationCascade {
    pub seed: VenueId,
    /// In the order they were closed out, the seed first.
    pub steps: Vec<CascadeStep>,
    /// Post-haircut cover consumed across every step.
    pub collateral_consumed: Decimal,
    /// Domains left standing, with their cover after the cascade.
    pub survivors: BTreeMap<VenueId, Decimal>,
}

impl LiquidationCascade {
    /// Domains closed out *because* the seed was, rather than with it.
    pub fn contagion(&self) -> usize {
        self.steps.len().saturating_sub(1)
    }

    /// Whether closing one venue out closed another.
    pub fn is_contagious(&self) -> bool {
        self.contagion() > 0
    }

    pub fn describe(&self) -> String {
        if self.is_contagious() {
            format!(
                "closing {} out takes {} further domain(s) with it and consumes {} of cover: {}",
                self.seed,
                self.contagion(),
                self.collateral_consumed,
                self.steps
                    .iter()
                    .map(|step| step.venue.to_string())
                    .collect::<Vec<_>>()
                    .join(" -> ")
            )
        } else {
            format!(
                "closing {} out consumes {} of cover and reaches no other domain",
                self.seed, self.collateral_consumed
            )
        }
    }
}

/// The collateral graph: assets, domains, pledges, and re-pledge links.
///
/// [`CollateralGraph::build`] is the only constructor and there is no setter,
/// so every instance has been through the refusals and stays through them. A
/// graph that could be edited after construction is a graph whose invariants
/// held at one instant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CollateralGraph {
    assets: BTreeMap<String, CollateralAsset>,
    domains: BTreeMap<VenueId, MarginDomain>,
    /// Face pledged, keyed venue then asset, so a coverage walk is ordered.
    pledges: BTreeMap<VenueId, BTreeMap<String, Decimal>>,
    links: Vec<CrossVenueLink>,
}

impl CollateralGraph {
    /// Assemble a graph, refusing every arrangement that would make the
    /// collateral balance larger than the collateral.
    ///
    /// Each refusal names what to do instead, because each is a caller's
    /// model being wrong rather than a transient condition:
    ///
    /// * a haircut outside `[0, 1)` — the asset is either not collateral or
    ///   the rate is a typo, and clamping it would post a number nobody
    ///   agreed;
    /// * a non-positive mark or pledge;
    /// * a pledge naming an asset or a domain the graph does not hold;
    /// * an asset pledged beyond its mark across every domain — the
    ///   double-pledge;
    /// * maintenance above initial, which closes an account out the instant
    ///   it opens;
    /// * a re-pledge out of a domain that forbids rehypothecation, into an
    ///   isolated domain, or beyond what is pledged at the source;
    /// * a cycle among re-pledge links.
    pub fn build(
        assets: Vec<CollateralAsset>,
        domains: Vec<MarginDomain>,
        pledges: Vec<Pledge>,
        links: Vec<CrossVenueLink>,
    ) -> Result<Self> {
        let by_id = Self::index_assets(assets)?;
        let by_venue = Self::index_domains(domains)?;
        let pledged = Self::index_pledges(pledges, &by_id, &by_venue)?;
        Self::refuse_repledge_chain(&links)?;
        Self::check_links(&links, &by_venue, &pledged)?;
        Ok(Self {
            assets: by_id,
            domains: by_venue,
            pledges: pledged,
            links,
        })
    }

    fn index_assets(assets: Vec<CollateralAsset>) -> Result<BTreeMap<String, CollateralAsset>> {
        let mut by_id: BTreeMap<String, CollateralAsset> = BTreeMap::new();
        for asset in assets {
            if asset.id.trim().is_empty() {
                return Err(Error::invalid(
                    "a collateral asset must be named, or nothing can say which pledge is \
                     against it",
                ));
            }
            if !asset.value.is_positive() {
                return Err(Error::invalid(format!(
                    "collateral {} is marked at {}; give it a positive mark or leave it out — an \
                     asset worth nothing posted as collateral covers nothing and hides nothing",
                    asset.id, asset.value
                )));
            }
            if asset.haircut.is_negative() || asset.haircut >= Decimal::ONE {
                return Err(Error::invalid(format!(
                    "collateral {} carries a haircut of {}, outside [0, 1); a haircut of one or \
                     more means the taker does not accept the asset, so remove it from the graph \
                     rather than posting it at a rate nobody agreed",
                    asset.id, asset.haircut
                )));
            }
            if by_id.insert(asset.id.clone(), asset.clone()).is_some() {
                return Err(Error::invalid(format!(
                    "collateral {} appears twice; merge the two marks into one asset — two rows \
                     for one holding is how a book comes to post the same security to two venues",
                    asset.id
                )));
            }
        }
        Ok(by_id)
    }

    fn index_domains(domains: Vec<MarginDomain>) -> Result<BTreeMap<VenueId, MarginDomain>> {
        let mut by_venue: BTreeMap<VenueId, MarginDomain> = BTreeMap::new();
        for domain in domains {
            if domain.initial.is_negative() || domain.maintenance.is_negative() {
                return Err(Error::invalid(format!(
                    "margin domain {} states a negative requirement; a requirement is what must \
                     be posted and cannot be below zero",
                    domain.venue
                )));
            }
            if domain.maintenance > domain.initial {
                return Err(Error::invalid(format!(
                    "margin domain {} maintains {} against an initial of {}; maintenance above \
                     initial closes an account out the instant it opens, so state the two the \
                     way the agreement does",
                    domain.venue, domain.maintenance, domain.initial
                )));
            }
            if by_venue
                .insert(domain.venue.clone(), domain.clone())
                .is_some()
            {
                return Err(Error::invalid(format!(
                    "margin domain {} appears twice; one venue is one account, and two rows for \
                     it would net exposure the venue does not net",
                    domain.venue
                )));
            }
        }
        Ok(by_venue)
    }

    fn index_pledges(
        pledges: Vec<Pledge>,
        assets: &BTreeMap<String, CollateralAsset>,
        domains: &BTreeMap<VenueId, MarginDomain>,
    ) -> Result<BTreeMap<VenueId, BTreeMap<String, Decimal>>> {
        let mut pledged: BTreeMap<VenueId, BTreeMap<String, Decimal>> = BTreeMap::new();
        let mut per_asset: BTreeMap<String, Decimal> = BTreeMap::new();
        for pledge in pledges {
            if !pledge.amount.is_positive() {
                return Err(Error::invalid(format!(
                    "the pledge of {} to {} is for {}; a pledge of nothing is not a pledge, so \
                     omit it",
                    pledge.asset, pledge.venue, pledge.amount
                )));
            }
            let Some(asset) = assets.get(&pledge.asset) else {
                return Err(Error::invalid(format!(
                    "{} is pledged to {} and is not an asset in this graph; add the holding \
                     before pledging it — a pledge against nothing is cover that exists only in \
                     the total",
                    pledge.asset, pledge.venue
                )));
            };
            if !domains.contains_key(&pledge.venue) {
                return Err(Error::invalid(format!(
                    "{} is pledged to {}, which is not a margin domain in this graph; declare \
                     the venue's regime before posting collateral to it",
                    pledge.asset, pledge.venue
                )));
            }
            let running = per_asset
                .entry(pledge.asset.clone())
                .or_insert(Decimal::ZERO);
            *running += pledge.amount;
            if *running > asset.value {
                return Err(Error::invalid(format!(
                    "{} is pledged for {} in total against a mark of {}; reduce the pledges or \
                     raise the mark — an asset pledged beyond its value is one holding counted \
                     twice, which is how a collateral balance comes to be larger than the \
                     collateral",
                    pledge.asset, running, asset.value
                )));
            }
            *pledged
                .entry(pledge.venue.clone())
                .or_default()
                .entry(pledge.asset.clone())
                .or_insert(Decimal::ZERO) += pledge.amount;
        }
        Ok(pledged)
    }

    fn check_links(
        links: &[CrossVenueLink],
        domains: &BTreeMap<VenueId, MarginDomain>,
        pledged: &BTreeMap<VenueId, BTreeMap<String, Decimal>>,
    ) -> Result<()> {
        for link in links {
            let Some(source) = domains.get(&link.from) else {
                return Err(Error::invalid(format!(
                    "collateral is re-pledged from {}, which is not a margin domain in this \
                     graph; declare the venue before recording an arrangement about it",
                    link.from
                )));
            };
            let Some(destination) = domains.get(&link.to) else {
                return Err(Error::invalid(format!(
                    "collateral is re-pledged to {}, which is not a margin domain in this graph; \
                     declare the venue before recording an arrangement about it",
                    link.to
                )));
            };
            if link.from == link.to {
                return Err(Error::invalid(format!(
                    "{} re-pledges its collateral to itself; remove the link — it would count \
                     the same assets twice at one venue",
                    link.from
                )));
            }
            if !link.amount.is_positive() {
                return Err(Error::invalid(format!(
                    "the re-pledge from {} to {} is for {}; omit the link rather than recording \
                     an arrangement that moves nothing",
                    link.from, link.to, link.amount
                )));
            }
            if source.rehypothecation != Rehypothecation::Permitted {
                return Err(Error::denied(format!(
                    "{} does not permit rehypothecation, so its collateral cannot margin {}; set \
                     {}'s rehypothecation to permitted only if the agreement with it says so",
                    link.from, link.to, link.from
                )));
            }
            if destination.regime != MarginRegime::Portfolio {
                return Err(Error::denied(format!(
                    "{} is isolated and does not accept collateral held at {}; an isolated \
                     domain ring-fences its own cover, so either record the cross-venue \
                     agreement by making {} portfolio-margined or drop the link",
                    link.to, link.from, link.to
                )));
            }
            let available: Decimal = pledged
                .get(&link.from)
                .map(|assets| assets.values().copied().sum())
                .unwrap_or(Decimal::ZERO);
            if link.amount > available {
                return Err(Error::invalid(format!(
                    "{} re-pledges {} to {} against {} posted there; re-pledge no more than is \
                     posted — the difference would be collateral invented out of an agreement",
                    link.from, link.amount, link.to, available
                )));
            }
        }
        Ok(())
    }

    /// Refuse a re-pledge chain deeper than one link.
    ///
    /// A venue that is the destination of one re-pledge may not be the source
    /// of another. Two refusals fall out of one rule, and both are wanted.
    ///
    /// The **cycle** — A → B → A — is one asset standing behind two
    /// obligations and behind itself, and every coverage figure computed over
    /// it is larger than the assets.
    ///
    /// The **chain** — A → B → C — is refused for a different and less
    /// obvious reason, and it is the one worth stating. What B re-pledges
    /// onward comes out of a pool holding A's collateral beside every other
    /// client's, and *which* of it travelled to C is a fact B knows and this
    /// platform does not. Modelling it would mean choosing an attribution
    /// rule — pro rata, first in, A's alone — and every one of those choices
    /// is a number presented as a measurement. The honest arm is to refuse
    /// the arrangement, so a desk that has genuinely signed a chain finds out
    /// here rather than reading a coverage figure derived from an assumption
    /// nobody made. Fail closed: the refusal names what to do instead.
    ///
    /// Two *independent* re-pledges out of one venue — A → B and A → C — are
    /// depth one each and admitted. The rule is about depth, not about
    /// rehypothecation.
    fn refuse_repledge_chain(links: &[CrossVenueLink]) -> Result<()> {
        let destinations: BTreeSet<&VenueId> = links.iter().map(|link| &link.to).collect();
        for link in links {
            if destinations.contains(&link.from) {
                return Err(Error::invalid(format!(
                    "{} re-pledges collateral onward to {} and is itself the destination of a \
                     re-pledge; a chain deeper than one link cannot be costed here, because \
                     which of the assets posted to {} travelled onward is a fact its holder \
                     knows and this platform does not. Record the arrangement as a direct pledge \
                     from whoever actually holds the asset, or leave the second link out — a \
                     coverage figure derived from a guess about which securities moved is worse \
                     than none",
                    link.from, link.to, link.from
                )));
            }
        }
        Ok(())
    }

    pub fn assets(&self) -> impl Iterator<Item = &CollateralAsset> {
        self.assets.values()
    }

    pub fn domains(&self) -> impl Iterator<Item = &MarginDomain> {
        self.domains.values()
    }

    pub fn links(&self) -> &[CrossVenueLink] {
        &self.links
    }

    /// Marked value of assets carrying no driver label.
    ///
    /// Published rather than folded into the correlation figures: an
    /// unlabelled asset counts as uncorrelated, and a reader who does not
    /// know how much of the graph is unlabelled cannot tell a book whose
    /// cover is genuinely independent from one nobody has labelled.
    pub fn unlabelled_value(&self) -> Decimal {
        self.assets
            .values()
            .filter(|asset| asset.driver.is_none())
            .map(|asset| asset.value)
            .sum()
    }

    /// The post-haircut value one face amount of one asset contributes.
    fn lendable_pledge(&self, asset_id: &str, face: Decimal) -> Result<Decimal> {
        let asset = self.assets.get(asset_id).ok_or_else(|| {
            Error::invalid(format!(
                "{asset_id} is pledged and is not in the graph; rebuild the graph, because a \
                 coverage figure computed without it would omit both the cover and the gap"
            ))
        })?;
        let retained = Decimal::ONE - asset.haircut;
        face.checked_mul(retained).ok_or_else(|| {
            Error::numeric(format!(
                "the lendable value of the pledge of {asset_id} overflowed"
            ))
        })
    }

    /// Whether an asset's driver is one of the drivers moving a domain.
    fn shares_driver(&self, asset_id: &str, drivers: &BTreeSet<String>) -> bool {
        self.assets
            .get(asset_id)
            .and_then(|asset| asset.driver.as_ref())
            .is_some_and(|driver| drivers.contains(driver))
    }

    /// What each domain has against what it owes.
    ///
    /// Re-pledged cover is attributed to the destination and *also* left
    /// standing at the source: that double count is what rehypothecation is,
    /// and [`Self::reuse`] names every asset it happens to rather than
    /// quietly netting it away.
    pub fn coverage(&self) -> Result<BTreeMap<VenueId, DomainCoverage>> {
        self.coverage_excluding(&BTreeSet::new(), &BTreeSet::new())
    }

    /// Coverage with a set of assets already sold and a set of domains
    /// already closed out.
    ///
    /// The one coverage walk in this module. [`Self::coverage`] is this with
    /// both sets empty, so the figure a cascade compares against and the
    /// figure a desk reads are produced by one function: two walks over one
    /// graph is two answers to one question, one cycle away from disagreeing.
    ///
    /// A re-pledge out of a closed domain carries nothing: the venue that
    /// closed the account out is not passing its collateral on.
    fn coverage_excluding(
        &self,
        consumed: &BTreeSet<String>,
        closed: &BTreeSet<VenueId>,
    ) -> Result<BTreeMap<VenueId, DomainCoverage>> {
        let mut coverage = BTreeMap::new();
        for (venue, domain) in &self.domains {
            let mut own = Decimal::ZERO;
            let mut correlated = Decimal::ZERO;
            for (asset_id, face) in self.pledges.get(venue).into_iter().flatten() {
                if consumed.contains(asset_id) {
                    continue;
                }
                let lendable = self.lendable_pledge(asset_id, *face)?;
                own += lendable;
                if self.shares_driver(asset_id, &domain.drivers) {
                    correlated += lendable;
                }
            }
            coverage.insert(
                venue.clone(),
                DomainCoverage {
                    venue: venue.clone(),
                    regime: domain.regime,
                    rehypothecation: domain.rehypothecation,
                    initial: domain.initial,
                    maintenance: domain.maintenance,
                    own,
                    rehypothecated: Decimal::ZERO,
                    correlated,
                },
            );
        }
        for link in &self.links {
            if closed.contains(&link.from) {
                continue;
            }
            let Some(destination) = self.domains.get(&link.to) else {
                continue;
            };
            let mut lendable = Decimal::ZERO;
            let mut correlated = Decimal::ZERO;
            for (asset_id, share) in self.repledged_shares(link, consumed)? {
                let value = self.lendable_pledge(&asset_id, share)?;
                lendable += value;
                if self.shares_driver(&asset_id, &destination.drivers) {
                    correlated += value;
                }
            }
            if let Some(entry) = coverage.get_mut(&link.to) {
                entry.rehypothecated += lendable;
                entry.correlated += correlated;
            }
        }
        Ok(coverage)
    }

    /// The face of each source asset a re-pledge carries onward.
    ///
    /// A re-pledge moves a share of a mixed pool, so the link's amount is
    /// split across the source's pledges pro rata by face. Naming which
    /// individual securities travelled would be a fact nobody recorded. The
    /// multiplication is done before the division so a small share of a large
    /// pool does not round to nothing.
    fn repledged_shares(
        &self,
        link: &CrossVenueLink,
        consumed: &BTreeSet<String>,
    ) -> Result<BTreeMap<String, Decimal>> {
        let mut shares = BTreeMap::new();
        let Some(source_pledges) = self.pledges.get(&link.from) else {
            return Ok(shares);
        };
        let face: Decimal = source_pledges.values().copied().sum();
        if !face.is_positive() {
            return Ok(shares);
        }
        for (asset_id, amount) in source_pledges {
            if consumed.contains(asset_id) {
                continue;
            }
            let share = amount
                .checked_mul(link.amount)
                .and_then(|numerator| numerator.checked_div(face))
                .ok_or_else(|| {
                    Error::numeric(format!(
                        "the re-pledged share of {asset_id} from {} overflowed",
                        link.from
                    ))
                })?;
            shares.insert(asset_id.clone(), share);
        }
        Ok(shares)
    }

    /// Every asset whose value is counted by more than one domain.
    ///
    /// Split pledges and re-pledges are different arrangements producing the
    /// same exposure, which is why one function finds both.
    pub fn reuse(&self) -> Result<Vec<CollateralReuse>> {
        let mut domains_of: BTreeMap<&str, BTreeSet<VenueId>> = BTreeMap::new();
        let mut counted: BTreeMap<String, Decimal> = BTreeMap::new();
        for (venue, assets) in &self.pledges {
            for (asset_id, face) in assets {
                domains_of
                    .entry(asset_id)
                    .or_default()
                    .insert(venue.clone());
                *counted.entry(asset_id.clone()).or_insert(Decimal::ZERO) +=
                    self.lendable_pledge(asset_id, *face)?;
            }
        }
        for link in &self.links {
            for (asset_id, share) in self.repledged_shares(link, &BTreeSet::new())? {
                let value = self.lendable_pledge(&asset_id, share)?;
                *counted.entry(asset_id.clone()).or_insert(Decimal::ZERO) += value;
                if let Some((key, _)) = self.assets.get_key_value(&asset_id) {
                    domains_of
                        .entry(key.as_str())
                        .or_default()
                        .insert(link.to.clone());
                }
            }
        }

        let mut findings = Vec::new();
        for (asset_id, venues) in domains_of {
            if venues.len() < 2 {
                continue;
            }
            let Some(asset) = self.assets.get(asset_id) else {
                continue;
            };
            findings.push(CollateralReuse {
                asset: asset_id.to_string(),
                value: asset.value,
                lendable: asset.lendable()?,
                domains: venues.into_iter().collect(),
                counted: counted.get(asset_id).copied().unwrap_or(Decimal::ZERO),
            });
        }
        Ok(findings)
    }

    /// Domains below maintenance right now.
    pub fn calls(&self) -> Result<Vec<DomainCoverage>> {
        Ok(self
            .coverage()?
            .into_values()
            .filter(DomainCoverage::is_call)
            .collect())
    }

    /// Domains covered only by collateral that falls with their own exposure:
    /// above maintenance on the arithmetic, below it once the correlated part
    /// of the cover is disbelieved.
    pub fn spirals(&self) -> Result<Vec<DomainCoverage>> {
        Ok(self
            .coverage()?
            .into_values()
            .filter(|cover| !cover.is_call() && cover.uncorrelated_excess().is_negative())
            .collect())
    }

    /// What a forced close-out at `seed` does to margin elsewhere.
    ///
    /// The venue closing an account out sells what it holds. Those assets
    /// stop standing behind anything — including behind the *other* domains
    /// counting the same assets through a split pledge or a re-pledge. Every
    /// domain that drops below maintenance as a result is closed out in turn,
    /// and so on to a fixed point.
    ///
    /// It terminates because a domain is closed out at most once, so the loop
    /// runs at most `domains` times. That is a property of the algorithm, not
    /// an iteration cap chosen to stop a runaway — a cap would make a genuine
    /// deep cascade indistinguishable from a bug.
    ///
    /// A cascade of one step is the answer "nothing else was counting on
    /// those assets", which is worth recording. It is not a failure to model
    /// anything.
    /// Post-haircut cover standing across every domain in a coverage map.
    fn cover_total(coverage: &BTreeMap<VenueId, DomainCoverage>) -> Decimal {
        coverage.values().map(DomainCoverage::effective).sum()
    }

    pub fn cascade(&self, seed: &VenueId) -> Result<LiquidationCascade> {
        if !self.domains.contains_key(seed) {
            return Err(Error::invalid(format!(
                "{seed} is not a margin domain in this graph, so there is nothing to close out \
                 there; name a venue the graph holds"
            )));
        }
        let mut closed: BTreeSet<VenueId> = BTreeSet::new();
        let mut consumed: BTreeSet<String> = BTreeSet::new();
        let mut steps = Vec::new();
        let mut total_consumed = Decimal::ZERO;
        let mut next = Some(seed.clone());
        // Cover standing across the whole graph. A step's consumption is the
        // fall in *this* rather than the face pledged at the venue being
        // closed, because selling an asset takes it away from every domain
        // counting it and the second of those is the finding. Summing the
        // venue's own pledges would report the 60 posted at the venue that
        // failed and stay silent about the 40 that vanished from the venue
        // next door, which is the contagion this function exists to show.
        let mut standing = Self::cover_total(&self.coverage_excluding(&consumed, &closed)?);

        while let Some(venue) = next {
            closed.insert(venue.clone());
            let shortfall = self
                .coverage_excluding(&consumed, &closed)?
                .get(&venue)
                .map(|cover| cover.excess().min(Decimal::ZERO).abs())
                .unwrap_or(Decimal::ZERO);
            for asset_id in self
                .pledges
                .get(&venue)
                .into_iter()
                .flatten()
                .map(|(id, _)| id)
            {
                consumed.insert(asset_id.clone());
            }
            let remaining = Self::cover_total(&self.coverage_excluding(&consumed, &closed)?);
            let consumed_here = standing - remaining;
            standing = remaining;
            total_consumed += consumed_here;
            steps.push(CascadeStep {
                venue,
                shortfall,
                collateral_consumed: consumed_here,
            });
            // The first still-open domain, in venue order, that the sale just
            // put under maintenance. Venue order rather than discovery order
            // so a replay over one graph reports one contagion path.
            next = self
                .coverage_excluding(&consumed, &closed)?
                .into_iter()
                .find(|(venue, cover)| !closed.contains(venue) && cover.is_call())
                .map(|(venue, _)| venue);
        }

        let survivors = self
            .coverage_excluding(&consumed, &closed)?
            .into_iter()
            .filter(|(venue, _)| !closed.contains(venue))
            .map(|(venue, cover)| (venue, cover.effective()))
            .collect();

        Ok(LiquidationCascade {
            seed: seed.clone(),
            steps,
            collateral_consumed: total_consumed,
            survivors,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qip_core::dec;

    fn venue(id: &str) -> VenueId {
        VenueId::new(id)
    }

    fn asset(id: &str, value: &str, haircut: &str, driver: Option<&str>) -> CollateralAsset {
        CollateralAsset {
            id: id.to_string(),
            value: Decimal::parse(value).unwrap_or(Decimal::ZERO),
            haircut: Decimal::parse(haircut).unwrap_or(Decimal::ZERO),
            driver: driver.map(str::to_string),
        }
    }

    fn domain(
        id: &str,
        regime: MarginRegime,
        rehypothecation: Rehypothecation,
        initial: &str,
        maintenance: &str,
        drivers: &[&str],
    ) -> MarginDomain {
        MarginDomain {
            venue: venue(id),
            regime,
            rehypothecation,
            initial: Decimal::parse(initial).unwrap_or(Decimal::ZERO),
            maintenance: Decimal::parse(maintenance).unwrap_or(Decimal::ZERO),
            drivers: drivers.iter().map(|d| d.to_string()).collect(),
        }
    }

    fn pledge(asset: &str, at: &str, amount: &str) -> Pledge {
        Pledge {
            asset: asset.to_string(),
            venue: venue(at),
            amount: Decimal::parse(amount).unwrap_or(Decimal::ZERO),
        }
    }

    #[test]
    fn an_asset_pledged_beyond_its_mark_is_refused_and_the_same_asset_split_is_admitted() {
        // The double-pledge. Two venues each told the whole 100 is theirs
        // adds up to 200 of cover against 100 of asset, and every coverage
        // figure downstream is then larger than the account. The admitting
        // half matters as much: splitting the same asset 60/40 across the two
        // is a legitimate arrangement and must build, or the refusal would be
        // a gate that refuses everything.
        let assets = vec![asset("cash-usd", "100", "0", None)];
        let domains = vec![
            domain(
                "XNYS",
                MarginRegime::Isolated,
                Rehypothecation::Forbidden,
                "10",
                "5",
                &[],
            ),
            domain(
                "XLON",
                MarginRegime::Isolated,
                Rehypothecation::Forbidden,
                "10",
                "5",
                &[],
            ),
        ];
        let over = CollateralGraph::build(
            assets.clone(),
            domains.clone(),
            vec![
                pledge("cash-usd", "XNYS", "100"),
                pledge("cash-usd", "XLON", "100"),
            ],
            Vec::new(),
        );
        let message = over
            .expect_err("200 pledged against 100 built")
            .message()
            .to_string();
        assert!(
            message.contains("pledged for 200 in total against a mark of 100"),
            "the refusal did not name the overage: {message}"
        );

        let split = CollateralGraph::build(
            assets,
            domains,
            vec![
                pledge("cash-usd", "XNYS", "60"),
                pledge("cash-usd", "XLON", "40"),
            ],
            Vec::new(),
        )
        .expect("a 60/40 split of one asset is a legitimate arrangement");
        let coverage = split.coverage().expect("coverage");
        assert_eq!(coverage[&venue("XNYS")].effective(), dec!("60"));
        assert_eq!(coverage[&venue("XLON")].effective(), dec!("40"));
    }

    #[test]
    fn a_haircut_of_one_or_more_is_refused_rather_than_clamped() {
        // A haircut of one means the taker does not accept the asset. Clamped
        // to 0.99 it becomes a row contributing a percent of cover, and the
        // arrangement nobody agreed is now in the total. Just under one is
        // admitted, so the bound is a bound.
        let refused = CollateralGraph::build(
            vec![asset("junk", "100", "1", None)],
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        let message = refused
            .expect_err("a haircut of one built")
            .message()
            .to_string();
        assert!(
            message.contains("outside [0, 1)"),
            "the refusal did not name the bound: {message}"
        );

        let admitted = CollateralGraph::build(
            vec![asset("junk", "100", "0.99", None)],
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
        .expect("a severe haircut is still a haircut");
        assert_eq!(
            admitted
                .assets()
                .next()
                .map(CollateralAsset::lendable)
                .transpose()
                .expect("lendable"),
            Some(dec!("1"))
        );
    }

    #[test]
    fn a_repledge_chain_is_refused_and_two_independent_repledges_are_admitted() {
        // Depth, not rehypothecation. A -> B -> A is one asset standing
        // behind itself; A -> B -> C cannot be costed because which of A's
        // securities B passed on to C is a fact B knows and this platform
        // does not. Both are two links and both are refused. A -> B and
        // A -> C are also two links, are depth one each, and must build — a
        // rule that refused those would be refusing rehypothecation outright
        // while claiming to refuse chains.
        let assets = vec![asset("bond", "100", "0", None)];
        let permitted = |id: &str, regime: MarginRegime| {
            domain(id, regime, Rehypothecation::Permitted, "10", "5", &[])
        };
        let domains = vec![
            permitted("A", MarginRegime::Portfolio),
            permitted("B", MarginRegime::Portfolio),
            permitted("C", MarginRegime::Portfolio),
        ];
        let pledges = vec![pledge("bond", "A", "100")];

        let cycle = CollateralGraph::build(
            assets.clone(),
            domains.clone(),
            pledges.clone(),
            vec![
                CrossVenueLink {
                    from: venue("A"),
                    to: venue("B"),
                    amount: dec!("50"),
                },
                CrossVenueLink {
                    from: venue("B"),
                    to: venue("A"),
                    amount: dec!("50"),
                },
            ],
        );
        let message = cycle.expect_err("a cycle built").message().to_string();
        assert!(
            message.contains("is itself the destination of a re-pledge"),
            "the refusal was not the depth refusal: {message}"
        );

        let chain = CollateralGraph::build(
            assets.clone(),
            domains.clone(),
            pledges.clone(),
            vec![
                CrossVenueLink {
                    from: venue("A"),
                    to: venue("B"),
                    amount: dec!("50"),
                },
                CrossVenueLink {
                    from: venue("B"),
                    to: venue("C"),
                    amount: dec!("25"),
                },
            ],
        );
        let message = chain.expect_err("a chain built").message().to_string();
        assert!(
            message.contains("is itself the destination of a re-pledge"),
            "the refusal was not the depth refusal: {message}"
        );

        let fan_out = CollateralGraph::build(
            assets,
            domains,
            pledges,
            vec![
                CrossVenueLink {
                    from: venue("A"),
                    to: venue("B"),
                    amount: dec!("50"),
                },
                CrossVenueLink {
                    from: venue("A"),
                    to: venue("C"),
                    amount: dec!("25"),
                },
            ],
        )
        .expect("two depth-one re-pledges out of one venue are a legitimate arrangement");
        let coverage = fan_out.coverage().expect("coverage");
        assert_eq!(coverage[&venue("B")].rehypothecated, dec!("50"));
        assert_eq!(coverage[&venue("C")].rehypothecated, dec!("25"));
    }

    #[test]
    fn a_repledge_into_an_isolated_domain_is_denied_and_permitted_into_a_portfolio_one() {
        // An isolated domain ring-fences its own cover by definition. Letting
        // a link reach one would be the model asserting a cross-margin
        // agreement that does not exist, which is the arrangement §25.6 calls
        // rare and dangerous.
        let assets = vec![asset("bond", "100", "0", None)];
        let pledges = vec![pledge("bond", "A", "100")];
        let link = vec![CrossVenueLink {
            from: venue("A"),
            to: venue("B"),
            amount: dec!("50"),
        }];
        let isolated = CollateralGraph::build(
            assets.clone(),
            vec![
                domain(
                    "A",
                    MarginRegime::Portfolio,
                    Rehypothecation::Permitted,
                    "10",
                    "5",
                    &[],
                ),
                domain(
                    "B",
                    MarginRegime::Isolated,
                    Rehypothecation::Permitted,
                    "10",
                    "5",
                    &[],
                ),
            ],
            pledges.clone(),
            link.clone(),
        );
        let message = isolated
            .expect_err("a link into an isolated domain built")
            .message()
            .to_string();
        assert!(
            message.contains("is isolated and does not accept collateral held at"),
            "the refusal was not the isolation refusal: {message}"
        );

        let portfolio = CollateralGraph::build(
            assets,
            vec![
                domain(
                    "A",
                    MarginRegime::Portfolio,
                    Rehypothecation::Permitted,
                    "10",
                    "5",
                    &[],
                ),
                domain(
                    "B",
                    MarginRegime::Portfolio,
                    Rehypothecation::Permitted,
                    "10",
                    "5",
                    &[],
                ),
            ],
            pledges,
            link,
        )
        .expect("a declared cross-venue arrangement is legitimate");
        let coverage = portfolio.coverage().expect("coverage");
        assert_eq!(coverage[&venue("B")].rehypothecated, dec!("50"));
        // And the coverage says whether the venue may lend the cover on. Two
        // domains holding the same figure under opposite rehypothecation
        // terms carry different counterparty risk, and a coverage line that
        // omits the term reads identically for both.
        assert_eq!(
            coverage[&venue("B")].rehypothecation,
            Rehypothecation::Permitted
        );
        assert!(
            coverage[&venue("B")]
                .describe()
                .contains("rehypothecation permitted"),
            "the coverage line did not name the term: {}",
            coverage[&venue("B")].describe()
        );
    }

    #[test]
    fn a_rehypothecated_asset_is_counted_at_both_ends_and_the_phantom_amount_is_named() {
        // Rehypothecation is legitimate; rehypothecation nobody has counted
        // is not. 100 of bond posted at A and 50 re-pledged to B is 150 of
        // cover across the graph against 100 of asset, and the 50 is the
        // number a desk needs stated rather than discovered.
        let graph = CollateralGraph::build(
            vec![asset("bond", "100", "0", None)],
            vec![
                domain(
                    "A",
                    MarginRegime::Portfolio,
                    Rehypothecation::Permitted,
                    "10",
                    "5",
                    &[],
                ),
                domain(
                    "B",
                    MarginRegime::Portfolio,
                    Rehypothecation::Permitted,
                    "10",
                    "5",
                    &[],
                ),
            ],
            vec![pledge("bond", "A", "100")],
            vec![CrossVenueLink {
                from: venue("A"),
                to: venue("B"),
                amount: dec!("50"),
            }],
        )
        .expect("build");
        let reuse = graph.reuse().expect("reuse");
        assert_eq!(reuse.len(), 1, "the premise is one reused asset: {reuse:?}");
        assert_eq!(reuse[0].asset, "bond");
        assert_eq!(reuse[0].lendable, dec!("100"));
        assert_eq!(reuse[0].counted, dec!("150"));
        assert_eq!(reuse[0].phantom(), dec!("50"));
        assert_eq!(reuse[0].domains, vec![venue("A"), venue("B")]);

        // And an asset pledged to exactly one domain is not a finding, or
        // every graph would be one.
        let plain = CollateralGraph::build(
            vec![asset("bond", "100", "0", None)],
            vec![domain(
                "A",
                MarginRegime::Isolated,
                Rehypothecation::Forbidden,
                "10",
                "5",
                &[],
            )],
            vec![pledge("bond", "A", "100")],
            Vec::new(),
        )
        .expect("build");
        assert!(plain.reuse().expect("reuse").is_empty());
    }

    #[test]
    fn a_domain_collateralised_by_its_own_driver_is_a_spiral_and_not_a_call() {
        // The margin spiral. 100 of oil-driven cover against 80 of
        // maintenance reads as 20 of excess; the domain's own exposure is
        // oil-driven too, so the move that makes the position a loss is the
        // move that takes the cover away. Both facts are published: it is not
        // a call today, and it has no excess a desk should believe.
        let graph = CollateralGraph::build(
            vec![asset("BRENT-FUT", "100", "0", Some("oil"))],
            vec![domain(
                "A",
                MarginRegime::Isolated,
                Rehypothecation::Forbidden,
                "90",
                "80",
                &["oil"],
            )],
            vec![pledge("BRENT-FUT", "A", "100")],
            Vec::new(),
        )
        .expect("build");
        let coverage = graph.coverage().expect("coverage");
        let cover = &coverage[&venue("A")];
        assert_eq!(
            cover.excess(),
            dec!("20"),
            "the premise is a positive excess"
        );
        assert!(!cover.is_call());
        assert_eq!(cover.correlated, dec!("100"));
        assert_eq!(cover.uncorrelated_excess(), dec!("-80"));
        assert!((cover.correlated_share() - 1.0).abs() < 1e-9);
        let spirals = graph.spirals().expect("spirals");
        assert_eq!(spirals.len(), 1);
        assert_eq!(spirals[0].venue, venue("A"));

        // The same book collateralised by something that does not move with
        // oil is not a spiral — otherwise the finding would fire on every
        // domain and mean nothing.
        let unrelated = CollateralGraph::build(
            vec![asset("cash-usd", "100", "0", Some("cash"))],
            vec![domain(
                "A",
                MarginRegime::Isolated,
                Rehypothecation::Forbidden,
                "90",
                "80",
                &["oil"],
            )],
            vec![pledge("cash-usd", "A", "100")],
            Vec::new(),
        )
        .expect("build");
        assert!(unrelated.spirals().expect("spirals").is_empty());
    }

    #[test]
    fn closing_one_venue_out_closes_a_second_that_was_counting_the_same_cash() {
        // The liquidation cascade. One 100 cash balance split 60/40 across
        // two isolated venues; XNYS is closed out and sells the whole
        // balance, and XLON — which was 40 against 35 of maintenance, and
        // solvent — is left with nothing. This is the contagion a per-book
        // margin number cannot show, because on the book the 100 covers the
        // 55 of total maintenance twice over.
        let graph = CollateralGraph::build(
            vec![asset("cash-usd", "100", "0", None)],
            vec![
                domain(
                    "XLON",
                    MarginRegime::Isolated,
                    Rehypothecation::Forbidden,
                    "38",
                    "35",
                    &[],
                ),
                domain(
                    "XNYS",
                    MarginRegime::Isolated,
                    Rehypothecation::Forbidden,
                    "25",
                    "20",
                    &[],
                ),
            ],
            vec![
                pledge("cash-usd", "XNYS", "60"),
                pledge("cash-usd", "XLON", "40"),
            ],
            Vec::new(),
        )
        .expect("build");
        let before = graph.coverage().expect("coverage");
        assert!(
            !before[&venue("XLON")].is_call(),
            "the premise is that XLON is solvent before the cascade"
        );
        assert!(!before[&venue("XNYS")].is_call());

        let cascade = graph.cascade(&venue("XNYS")).expect("cascade");
        assert!(cascade.is_contagious(), "{}", cascade.describe());
        assert_eq!(cascade.contagion(), 1);
        assert_eq!(
            cascade
                .steps
                .iter()
                .map(|s| s.venue.clone())
                .collect::<Vec<_>>(),
            vec![venue("XNYS"), venue("XLON")]
        );
        assert_eq!(cascade.steps[1].shortfall, dec!("35"));
        assert_eq!(cascade.collateral_consumed, dec!("100"));
        assert!(cascade.survivors.is_empty());
    }

    #[test]
    fn closing_a_venue_out_whose_collateral_nobody_else_counts_reaches_no_one() {
        // The other half of the cascade: two venues each with their own
        // asset. Closing one out consumes its own cover and the second is
        // untouched. Without this the contagion test would pass on a cascade
        // that closed every domain unconditionally.
        let graph = CollateralGraph::build(
            vec![
                asset("cash-usd", "100", "0", None),
                asset("cash-gbp", "100", "0", None),
            ],
            vec![
                domain(
                    "XLON",
                    MarginRegime::Isolated,
                    Rehypothecation::Forbidden,
                    "38",
                    "35",
                    &[],
                ),
                domain(
                    "XNYS",
                    MarginRegime::Isolated,
                    Rehypothecation::Forbidden,
                    "25",
                    "20",
                    &[],
                ),
            ],
            vec![
                pledge("cash-usd", "XNYS", "60"),
                pledge("cash-gbp", "XLON", "40"),
            ],
            Vec::new(),
        )
        .expect("build");
        let cascade = graph.cascade(&venue("XNYS")).expect("cascade");
        assert!(!cascade.is_contagious(), "{}", cascade.describe());
        assert_eq!(cascade.steps.len(), 1);
        assert_eq!(cascade.collateral_consumed, dec!("60"));
        assert_eq!(cascade.survivors.get(&venue("XLON")), Some(&dec!("40")));
    }

    #[test]
    fn a_cascade_seeded_at_a_venue_the_graph_does_not_hold_is_refused() {
        // A typo in a venue name would otherwise answer "nothing happens",
        // which is the most reassuring possible answer to a question that was
        // never asked.
        let graph = CollateralGraph::build(
            vec![asset("cash-usd", "100", "0", None)],
            vec![domain(
                "XNYS",
                MarginRegime::Isolated,
                Rehypothecation::Forbidden,
                "25",
                "20",
                &[],
            )],
            vec![pledge("cash-usd", "XNYS", "60")],
            Vec::new(),
        )
        .expect("build");
        let message = graph
            .cascade(&venue("XNYSE"))
            .expect_err("an unknown seed answered")
            .message()
            .to_string();
        assert!(
            message.contains("is not a margin domain in this graph"),
            "the refusal did not name the missing domain: {message}"
        );
        assert!(
            graph.cascade(&venue("XNYS")).is_ok(),
            "the real seed was refused"
        );
    }

    #[test]
    fn maintenance_above_initial_is_refused_because_such_an_account_never_opens() {
        let refused = CollateralGraph::build(
            Vec::new(),
            vec![domain(
                "XNYS",
                MarginRegime::Isolated,
                Rehypothecation::Forbidden,
                "10",
                "20",
                &[],
            )],
            Vec::new(),
            Vec::new(),
        );
        let message = refused
            .expect_err("maintenance above initial built")
            .message()
            .to_string();
        assert!(
            message.contains("maintains 20 against an initial of 10"),
            "the refusal did not name the two figures: {message}"
        );
    }

    #[test]
    fn an_unlabelled_asset_is_published_rather_than_counted_as_correlated() {
        // An unlabelled asset counts as uncorrelated, which is the generous
        // arm. The gap is therefore published as a figure rather than left
        // for a reader to infer from a correlation number that looks low for
        // two quite different reasons.
        let graph = CollateralGraph::build(
            vec![
                asset("BRENT-FUT", "100", "0", Some("oil")),
                asset("mystery", "40", "0", None),
            ],
            vec![domain(
                "A",
                MarginRegime::Isolated,
                Rehypothecation::Forbidden,
                "50",
                "40",
                &["oil"],
            )],
            vec![
                pledge("BRENT-FUT", "A", "100"),
                pledge("mystery", "A", "40"),
            ],
            Vec::new(),
        )
        .expect("build");
        assert_eq!(graph.unlabelled_value(), dec!("40"));
        let coverage = graph.coverage().expect("coverage");
        assert_eq!(coverage[&venue("A")].correlated, dec!("100"));
        assert_eq!(coverage[&venue("A")].effective(), dec!("140"));
    }
}
