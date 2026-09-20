//! The licensing gate a connector source passes before a composition root
//! opens it.
//!
//! `.claude/rules/domains/data-and-streaming.md` is categorical: a source's
//! licensing posture is evaluated **before** the source is used, and a
//! research-only licence never reaches the trading path. The connector SDK
//! carries a licensing *class* in each manifest, but a class is a label, not
//! an evaluation — the evaluation is the reading of the actual terms, mapped
//! onto the usages this platform makes, and it lives here as a catalogue with
//! one entry per admitted source.
//!
//! The two questions asked of every source are [`Usage::Derive`] and
//! [`Usage::Trade`]. Derive because that is what the loop factually does with
//! a record — features, statistics, simulated decisions — and Trade because
//! the decision loop *is* the trading path, paper today and the recorded
//! destination tomorrow (ADR 0023): a source admitted here on a
//! research-only licence would be promoted onto live trading by nothing more
//! than the ceiling changing, which is precisely the quiet promotion the
//! rule's own example forbids.
//!
//! This module lives in the data finder rather than in a binary so that every
//! composition root that opens a connector — `qip-api` today — asks the same
//! catalogue the same questions. `qip-fastbrain` still carries its own copy of
//! these entries in `licensing.rs`; the two must say the same thing about
//! each source until that root is pointed here, and a disagreement between
//! them is a disagreement about one licence, which is the state the class
//! check below refuses on purpose.
//!
//! [`admit`] returns a [`LicensingDecision`] rather than `()` so the root can
//! state, in its banner, which licence admitted the source for which usages
//! at which instant — a gate whose only output is silence is one an operator
//! cannot tell from a gate that never ran.
//!
//! The gate asks a second question after the licence: who registered with
//! the venue. A [`crate::registration::RegistrationRegistry`] declares what
//! each source demands and holds the records the owner made; a source that
//! needs an account and has no record is refused by name, with a refusal that
//! says anonymous or automated registration is not a path this platform
//! offers. [`admit`] and [`admit_from`] consult the shipped registry, which
//! records nobody; [`admit_registered`] and [`admit_from_registered`] take
//! the owner's.
//!
//! # Before use, and for as long as it is used
//!
//! Each of the four functions above answers **at an instant**, because
//! `LicensingPosture::legality_for` takes one and consults the licence's own
//! effective and expiry dates. Called once at start-up, they satisfy the rule
//! exactly: the evaluation precedes the socket. That is sufficient for a
//! process whose life is a cycle and insufficient for one asked to stream for a
//! week, where a licence can lapse between the start-up that admitted it and
//! the poll that uses it. [`StandingAdmission`] is the same gate held open —
//! the whole of it, re-asked at the instant of each poll — and it exists
//! because a control consulted once is a control that cannot fire.

use qip_contracts::governance::Usage;
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_financial::quality::LicensingClass;
use qip_market_ingestion::connector::{FieldKind, SchemaContract, SourceManifest};
use qip_market_ingestion::connector_feed::KNOWN_SOURCES;
use serde::Serialize;

use crate::category::SourceCategory;
use crate::legal::{LicensingPosture, SourceLicense};
use crate::registration::{RegistrationRegistry, RegistrationStanding};
use crate::schema::{FieldType, SourceSchema};

/// One catalogued source: the evaluation of its actual terms.
///
/// `Clone` so that a caller holding a borrowed catalogue can hand a
/// [`StandingAdmission`] the entries it must keep re-asking. The gate holds the
/// question, not the answer — see its documentation — and a question it could
/// not own would have to be rebuilt from [`catalogue`] on every poll, which is
/// a second reading of the same code and a second place for the two to differ.
#[derive(Clone, Debug)]
pub struct CatalogueEntry {
    /// Must match the manifest's `source_id` exactly.
    pub source_id: &'static str,
    /// Must match the manifest's licensing class. A mismatch means the
    /// manifest and this catalogue were edited independently, and the safe
    /// reading of a disagreement between two claims about one licence is
    /// that neither is current.
    pub expected_class: LicensingClass,
    /// The evaluation itself.
    pub posture: LicensingPosture,
}

/// The usages every source is asked about before it may feed the loop.
pub const REQUIRED_USAGES: [Usage; 2] = [Usage::Derive, Usage::Trade];

/// Proof that [`admit_from_registered`] ran and said yes.
///
/// A zero-sized value with no public constructor, held privately by
/// [`LicensingDecision`]. Every other field of that type is public so a
/// banner can read it, which until this marker existed also meant any code
/// could *write* one — a struct literal naming a licence nobody evaluated
/// would have been indistinguishable from the gate's own answer. Now a
/// decision can only be minted at the one site in this module that has just
/// asked every usage question, so a type that takes `&LicensingDecision` as
/// its precondition is genuinely gated on the licence having been read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GatePassed(());

/// What the gate decided, for the banner and the record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LicensingDecision {
    pub source_id: String,
    /// The licence identifier the catalogue entry was written against.
    pub licence: String,
    /// The class the manifest declares and the catalogue agreed with.
    pub class: LicensingClass,
    /// The usages the licence was found to permit at `decided_at`.
    pub usages: Vec<Usage>,
    /// Who registered with the venue, or that nobody had to. Carried so the
    /// banner names the person the credential is attributed to, and so a
    /// decision on a keyless source says "keyless" rather than nothing.
    pub registration: RegistrationStanding,
    pub decided_at: Timestamp,
    /// See [`GatePassed`]: the one field a caller cannot supply.
    gate: GatePassed,
}

impl LicensingDecision {
    /// One line an operator can read at start-up.
    pub fn describe(&self) -> String {
        format!(
            "admitted under licence `{}` (class {:?}) for {} at {}; {}",
            self.licence,
            self.class,
            self.usages
                .iter()
                .map(|usage| usage.as_str())
                .collect::<Vec<_>>()
                .join(" and "),
            self.decided_at.to_rfc3339(),
            self.registration.describe()
        )
    }
}

/// The catalogue. One entry per source this build may open.
///
/// # coinbase-spot-ticker
///
/// Coinbase Exchange market data over the public, unauthenticated endpoint.
/// The terms read for this evaluation: Coinbase's Market Data terms permit
/// use of the public feed for internal purposes — consumption, analysis,
/// derivation, and acting on it — and do **not** grant redistribution or
/// display of the raw data to third parties. That is `Internal`, not
/// `Public`: the manifest's class says so, and the licence below grants
/// research, derivation and trading while withholding redistribution. No
/// expiry is stated in the terms; the entry carries none, and a change in
/// the vendor's terms is a change to this entry, reviewed like code.
///
/// # frankfurter-ecb-reference-rates
///
/// The euro foreign-exchange reference rates the ECB publishes each working
/// day, served unauthenticated by Frankfurter. The terms read for this
/// evaluation: the ECB permits reuse of its published reference rates,
/// including for commercial purposes, provided the source is acknowledged;
/// Frankfurter is a free relay of that same series and adds no term of its
/// own restricting it. That is `Public` — the one class in this catalogue
/// whose grant includes `Redistribute`, and the reason a number derived from
/// it may be shown to a client where a Coinbase-derived one may not.
///
/// The acknowledgement obligation is the thing to notice and the thing this
/// entry cannot enforce: `LicensingClass::Public` is a statement about what
/// the platform may do, not a mechanism that attributes anything. Displaying
/// these rates in the console without naming the ECB would satisfy every
/// check in this file and still breach the terms it cites. Nothing displays
/// them today; whoever first does owns that.
///
/// No expiry is stated, so the entry carries none. This posture was written
/// from the published terms and not from a negotiated agreement — there is
/// no contract to read — so a change in either party's terms is a change to
/// this entry, reviewed like code.
///
/// # ecb-key-interest-rates
///
/// The three key interest rates the Governing Council sets — the deposit
/// facility, the marginal lending facility and the main refinancing
/// operations fixed rate — served unauthenticated by the ECB's own data
/// portal. The terms read for this evaluation, from the ECB's copyright
/// statement at `https://www.ecb.europa.eu/services/disclaimer/html/index.en.html`
/// as it stood on 2026-09-15: "users of this website may make free use of the
/// information obtained directly from it", subject to three conditions — the
/// information must appear accurately and the ECB must be cited as the source;
/// a publisher who sells a document containing it must tell buyers it is
/// available free from the ECB; and "if the information is modified by the user
/// (e.g. by seasonal adjustment of statistical data or calculation of growth
/// rates) this must be stated explicitly". The one exception the statement
/// carries is for authored documents such as Working Papers, which this source
/// does not serve. That is `Public`, on the same reading as the reference rates
/// above, and for the same reason its grant includes `Redistribute`.
///
/// **The third condition is the one this platform actually triggers**, and it
/// is the reason to notice that a class is a label and the evaluation is the
/// reading. `qip-capital-fabric` does not use the published level: it divides
/// an annual percentage by the days in a year to obtain one day's accrual for
/// §38.3's fiat row. That is a calculation on the vendor's figure of exactly
/// the kind the condition names, so
/// `qip_capital_fabric::tolerance::SourcedIntervalRate::derivation` states it
/// in words and that sentence travels in every record the rate reaches — which
/// is what satisfies the condition, not this comment. The first condition,
/// acknowledgement, this entry cannot enforce any more than the Frankfurter
/// one can; nothing displays these rates today and whoever first does owns it.
///
/// No expiry is stated, so the entry carries none. This posture was written
/// from the published terms and not from a negotiated agreement — there is no
/// contract to read — so a change in the ECB's terms is a change to this entry,
/// reviewed like code.
///
/// # nyfed-effr
///
/// The Effective Federal Funds Rate — the volume-weighted median of overnight
/// federal funds transactions, published each business day for the prior
/// business day — served unauthenticated by the Federal Reserve Bank of New
/// York's own markets API. The terms read for this evaluation are the New York
/// Fed's Terms of Use at `https://www.newyorkfed.org/privacy/termsofuse`,
/// "Last Updated: 6/9/2023", fetched and read on 2026-09-16.
///
/// **The grant.** "The New York Fed grants you a non-exclusive license,
/// subject to the Terms, to use, copy, and distribute Content for your
/// personal or business purposes", and the enumerated permissions include
/// access "manually or through an automated process or device, provided your
/// access does not have the effect of disabling, damaging, or interfering with
/// the function of the Website"; "Download, store, and use Content in any
/// format or media"; "Copy and distribute the Content in any format or media";
/// and "Modify and create derivative works from the Content". That is
/// `Public`, on the same reading as the two ECB entries above, and for the
/// same reason its grant includes `Redistribute`. The Prohibited Uses section
/// names illegal or fraudulent use, impersonation, interference with the site,
/// and unauthorised access; none describes this platform, and the manifest's
/// one-request-per-minute rate limit against an hourly poll is what keeps the
/// "does not interfere" proviso a fact rather than an intention.
///
/// **The conditions, all of which attach.** Copyright notices and source
/// identifiers travel with any copy; where no specific form is given the
/// attribution is "© [year] Federal Reserve Bank of New York. Content from the
/// New York Fed subject to the Terms of Use at newyorkfed.org."; a
/// modification "must clearly label the modified Content" and "You may not
/// attribute any modifications or derivative works to the New York Fed"; a
/// distributor "must make the Content available with the same permissions,
/// conditions, and restrictions set forth in these Terms" and "may not impose
/// more restrictive terms"; and nothing may "state or imply that the New York
/// Fed endorses your use".
///
/// **The use restriction, which is why this entry is longer than the ECB's.**
/// Reference rates are one of the categories the Terms single out, and EFFR is
/// one of the rates named. The restriction is verbatim: "If you use or
/// distribute reference rate data or related information posted to the
/// website, you must include the following notice and disclaimer with your
/// presentation of that data or information: 'The [NAME OF DATA or CONTENT] is
/// subject to the Terms of Use posted at newyorkfed.org. The New York Fed is
/// not responsible for publication of the [DATA NAME] by [NAME OF PUBLISHER],
/// does not [sanction] or [endorse] any particular republication, and has no
/// liability for your use.'" The brackets, the Terms say, "indicate detail to
/// be completed by the person using or distributing the reference rate data".
///
/// **Where that notice lives, and what it cannot do.** The completed text is
/// `qip_market_ingestion::connectors::NyFedEffrConnector::REFERENCE_RATE_NOTICE`,
/// written once, and it is carried by
/// `qip_capital_fabric::tolerance::SourcedIntervalRate::presentation_notice`
/// into the derivation sentence every §38.3 tolerance record derived from this
/// rate keeps — which is the only place the derived figure is written down
/// today. That is a mechanism, not a comment, and it is still **not
/// compliance**: the obligation attaches to *presentation*, and this platform
/// cannot compel a console that does not exist yet to render the sentence it
/// carries. Nothing displays this rate today; whoever first does owns it,
/// exactly as the two ECB entries above say about acknowledgement. Three other
/// obligations are likewise recorded and unenforceable by any check in this
/// file: the attribution format, the same-permissions condition on
/// redistribution (this platform redistributes nothing today), and the Terms'
/// own statement that "Users are responsible for monitoring the Website for
/// any changes" — a change in the New York Fed's terms is a change to this
/// entry, reviewed like code, and no code here will notice it.
///
/// **Two things the evaluation deliberately does not claim.** The trademark
/// carve-out permitting a reference rate's name in a product or service name
/// is not relied on — this platform names no product after EFFR, so the
/// further "[User] is not affiliated with the New York Fed" disclaimer the
/// Terms attach to that use is not triggered. And the SOFR and BGCR series on
/// the same host are **not** covered: the Terms record that they "are
/// calculated using data provided under a license granted to the New York Fed
/// by DTCC Solutions LLC", which is a third party's licence this evaluation
/// has not read. `NyFedEffrConnector` refuses a manifest pointed at either,
/// so that limit is a refusal rather than a sentence.
///
/// No expiry is stated, so the entry carries none. This posture was written
/// from the published terms and not from a negotiated agreement — there is no
/// contract to read.
///
/// # nws-station-observations
///
/// Surface weather observations from one United States National Weather
/// Service station, served unauthenticated by `api.weather.gov`. The terms
/// read for this evaluation are the NWS disclaimer at
/// `https://www.weather.gov/disclaimer`, fetched and read on 2026-09-19,
/// together with the appropriate-use guidance published on the same page.
///
/// **The grant.** "The information on National Weather Service (NWS) Web
/// pages are in the public domain, unless specifically noted otherwise, and
/// may be used without charge for any lawful purpose". Public domain with no
/// charge and no field-of-use limit is `Public`, and it is the broadest grant
/// in this catalogue: unlike the ECB and New York Fed entries, which are
/// permissive licences, this is an absence of copyright under 17 U.S.C. § 105
/// rather than a permission granted. `Redistribute` follows from "for any
/// lawful purpose" rather than from an enumerated permission.
///
/// **The three conditions, all of which attach.** A user may not "claim it is
/// your own (e.g., by claiming copyright for NWS information)"; may not "use
/// it in a manner that implies an endorsement or affiliation with NOAA/NWS";
/// and may not "modify its content and then present it as official government
/// material", nor "present information of your own in a way that makes it
/// appear to be official government information". The completed attribution
/// and non-endorsement sentence is
/// `qip_market_ingestion::connectors::NwsStationObservationsConnector::ATTRIBUTION`,
/// written once, and every record this connector produces carries the
/// `nws-station-observations` source identifier in its `Provenance`. That is a
/// mechanism rather than a comment, and it is still **not compliance**: the
/// first two conditions attach to *presentation*, and nothing displays these
/// readings today. Whoever first does owns them, exactly as the ECB and New
/// York Fed entries say about their own display obligations.
///
/// **Two further obligations recorded and unenforceable by any check here.**
/// 17 U.S.C. § 403 requires a third party producing a copyrighted work
/// consisting predominantly of NWS material to identify the NWS material and
/// state that it is not subject to copyright protection; this platform
/// publishes no such work today. And the NWS name and visual identifier are
/// protected under trademark law — this platform uses neither, and the
/// connector is named for the source rather than after it.
///
/// **The appropriate-use guidance is an operational obligation, and it is the
/// one this entry can point at code for.** The NWS asks a client to "know your
/// data refresh frequency" and to "request only the data that you need", and
/// reserves the right to block an IP address that impacts service delivery.
/// The shipped manifest polls hourly against a rate limit of one request per
/// minute, and asks for one station's last twelve observations rather than the
/// network-wide feed; `NwsStationObservationsConnector::new` refuses a
/// manifest pointed at the network-wide path by name. That is the guidance
/// held by a refusal rather than by an intention.
///
/// **One obligation this platform cannot discharge, stated plainly.** The
/// NWS's API documentation asks a client to identify itself in a `User-Agent`
/// and recommends including a contact address. `qip_transport` writes its own
/// fixed `user-agent` and a manifest cannot override it, so a request from
/// this platform is identified and carries no contact. Changing that is a
/// decision about every connector rather than about this one. Nothing is
/// deployed and no process of this platform has an outbound path to this host,
/// so no request has ever been made.
///
/// No expiry is stated, so the entry carries none. The NWS may revise its
/// disclaimer without notice, so a change in those terms is a change to this
/// entry, reviewed like code, and no code here will notice it.
///
/// # kalshi-markets and alpaca-daily-bars — refused until their terms are read
///
/// The two remaining ADR 0034 candidates. Both connectors exist and both are
/// on `KNOWN_SOURCES`, so this catalogue must say something about them or
/// `admit` refuses them as uncatalogued — which is the right outcome for the
/// wrong reason, and an operator reading the refusal would go looking for a
/// missing entry rather than for the terms. Each carries
/// [`LicensingPosture::Ambiguous`]: the terms exist and nobody has mapped
/// them onto this platform's usages, so every usage question answers
/// `unknown` and `admit` refuses. ADR 0034 is explicit that its description
/// of each vendor's terms is not an evaluation and not legal advice; the
/// evidence names the document to read. Neither is `Declared`, and neither
/// becomes so by an edit to the connector or the manifest — only by
/// replacing the posture here, with the terms cited, under review.
///
/// Both manifests declare `Restricted`, the most restrictive class short of
/// `Synthetic`, and `expected_class` agrees so that the refusal an operator
/// sees is the licensing one and not a class disagreement. When the terms
/// are read the class may relax; it does so in the manifest and here in one
/// commit, or the disagreement check refuses the source again.
pub fn catalogue() -> Result<Vec<CatalogueEntry>> {
    Ok(vec![
        CatalogueEntry {
            source_id: "kalshi-markets",
            expected_class: LicensingClass::Restricted,
            posture: LicensingPosture::ambiguous(
                "Kalshi's terms of service and API terms at https://kalshi.com/terms have not \
                 been read against this platform's usages; ADR 0034 names the source as a \
                 candidate only",
            ),
        },
        CatalogueEntry {
            source_id: "alpaca-daily-bars",
            expected_class: LicensingClass::Restricted,
            posture: LicensingPosture::ambiguous(
                "Alpaca's market-data terms and account agreement at \
                 https://alpaca.markets/terms-and-conditions have not been read against this \
                 platform's usages; ADR 0034 names the source as a candidate only, and its \
                 paper brokerage is the account the data terms come with",
            ),
        },
        CatalogueEntry {
            source_id: "coinbase-spot-ticker",
            expected_class: LicensingClass::Internal,
            posture: LicensingPosture::declared(SourceLicense::new(
                "coinbase-exchange-market-data-terms",
                [Usage::Research, Usage::Derive, Usage::Trade],
            )?),
        },
        CatalogueEntry {
            source_id: "ecb-key-interest-rates",
            expected_class: LicensingClass::Public,
            posture: LicensingPosture::declared(SourceLicense::new(
                "ecb-website-copyright-free-use",
                [
                    Usage::Research,
                    Usage::Derive,
                    Usage::Trade,
                    Usage::Redistribute,
                ],
            )?),
        },
        CatalogueEntry {
            source_id: "nyfed-effr",
            expected_class: LicensingClass::Public,
            posture: LicensingPosture::declared(SourceLicense::new(
                "nyfed-terms-of-use-reference-rates",
                [
                    Usage::Research,
                    Usage::Derive,
                    Usage::Trade,
                    Usage::Redistribute,
                ],
            )?),
        },
        CatalogueEntry {
            source_id: "nws-station-observations",
            expected_class: LicensingClass::Public,
            posture: LicensingPosture::declared(SourceLicense::new(
                "nws-public-domain-with-attribution-conditions",
                [
                    Usage::Research,
                    Usage::Derive,
                    Usage::Trade,
                    Usage::Redistribute,
                ],
            )?),
        },
        CatalogueEntry {
            source_id: "frankfurter-ecb-reference-rates",
            expected_class: LicensingClass::Public,
            posture: LicensingPosture::declared(SourceLicense::new(
                "ecb-reference-rates-via-frankfurter",
                [
                    Usage::Research,
                    Usage::Derive,
                    Usage::Trade,
                    Usage::Redistribute,
                ],
            )?),
        },
    ])
}

/// A source whose terms were read and answer the gate's questions **no**.
///
/// The catalogue above holds sources whose terms were read and admit them,
/// and the two ADR 0034 candidates whose terms nobody has read. This is the
/// third state, and until it existed it was indistinguishable from the
/// second: a source evaluated and refused had no entry, so `admit` refused it
/// with "its terms have not been read" — a false sentence about work that
/// was done, and an invitation to do it again. Worse, nothing stopped the
/// next lane from writing a connector and a catalogue entry for it, because
/// the refusal lived in a decision record and not in the code path.
///
/// An entry here is consulted by the gate *before* the catalogue is, so a
/// refused source cannot be admitted by adding an entry: the register has to
/// be edited in the same commit, under review, with the clause that changed.
/// And `no_refused_source_has_a_connector_or_a_catalogue_entry` below holds
/// the other half — a connector on `KNOWN_SOURCES` for a source this
/// register refuses is code the gate can never open, which is ADR 0050's
/// alternative (f), "build the connector first and decide the licensing
/// after", the ordering the data rule forbids.
///
/// Every field is the evaluation's evidence and none is a summary: the clause
/// is verbatim from the document at `terms_url` as it dated itself, so a
/// reader can check the register against the vendor rather than against this
/// file. A vendor's terms changing is a change to the entry, reviewed like
/// code, and no code here will notice it — the same sentence every admitted
/// entry carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefusedSource {
    /// The identifier a connector for this source would carry. Reserved by
    /// this entry: a manifest declaring it is refused by name.
    pub source_id: &'static str,
    /// The publisher, as its own terms name it.
    pub vendor: &'static str,
    /// The document read.
    pub terms_url: &'static str,
    /// The version read, as the document dates itself.
    pub terms_dated: &'static str,
    /// When this platform read it.
    pub evaluated_on: &'static str,
    /// The clause that refuses, verbatim.
    pub clause: &'static str,
    /// Which of the gate's questions the clause answers no to.
    pub refuses: &'static [Usage],
    /// The decision record holding the full evaluation.
    pub record: &'static str,
}

/// The sources evaluated for ADR 0050 — an option-quote source for the
/// volatility surface — and refused on their own terms.
///
/// # What was looked for, and why none of it admits
///
/// ADR 0050 asks `Usage::Derive` and `Usage::Trade` of every source, because
/// deriving is what the loop does and trading is what the loop is. An
/// option-quote source has to grant both. Four publishers' terms were fetched
/// and read on 2026-09-19 — two venues' own feeds (class (a) in the ADR's
/// taxonomy) and two delayed or end-of-day publications (class (d), the class
/// the ADR said to try first) — and every one refuses at least one of the two
/// in a clause quoted below. Two more could not be read at all: Bybit's terms
/// page renders only in a browser, and Binance's answered the fetch with an
/// empty challenge page; neither is refused here because nothing was read,
/// and neither is admissible for the same reason.
///
/// The pattern across all four is the same clause in four house styles:
/// market data is for **personal** use, and **derivative works** — named as
/// "a financial product, service or index" by one of them — need the
/// publisher's written consent. A research and risk desk deriving a
/// volatility surface is the use the clause describes and refuses. There is
/// no reading of "personal use" under which a platform is the person.
///
/// The public-domain route the NWS entry took does not exist for options:
/// no public body publishes option quotes or settlement prices, and the one
/// central bank that publishes option-*implied* series (the Bank of England,
/// under the Open Government Licence) publishes a model output the ADR's
/// requirement 2 refuses as the engine's input regardless of its licence.
///
/// # A fourth source that could not be read: the SEC's EDGAR (2026-09-20)
///
/// The public-domain route *should* exist for §7.1's Corporate class — the
/// SEC's EDGAR filings are a work of the United States Government — and it
/// was tried, on the NWS precedent, as the text producer
/// `qip_market_ingestion::narrative` has never had. The gate before any
/// code is to read the SEC's own terms and fair-access policy from sec.gov,
/// and they could not be read: four attempts between 08:41Z and 08:55Z,
/// three `User-Agent` shapes (never a personal address), every page
/// including `robots.txt`, all answered `403` by the SEC's own edge with a
/// page titled "Request Rate Threshold Exceeded" whose body reads, verbatim,
/// "Automated access to our sites must comply with SEC.gov's Privacy and
/// Security Policy" and points to `www.sec.gov/developer` for "Fair Access
/// guidelines". The session's outbound proxy shares one egress address and
/// that address was over the SEC's threshold before the first request.
///
/// So EDGAR is in the same state as Bybit and Binance above: nothing was
/// read, so it is neither refused here nor admissible, and no connector was
/// written for it — the ordering the data rule demands. Two facts for the
/// retry, from the SEC's own page rather than from its terms: the policy
/// requires a *declared* `User-Agent` carrying company and contact
/// information, which `qip_transport`'s fixed `user-agent` cannot carry
/// (the NWS entry records the same limit, where it was a recommendation
/// rather than a requirement); and the request-rate ceiling, once read, is
/// a manifest fact the connector must refuse to run without.
pub fn refusals() -> Vec<RefusedSource> {
    vec![
        RefusedSource {
            source_id: "deribit-options",
            vendor: "Deribit FZE",
            terms_url: "https://support.deribit.com/hc/en-us/articles/25944532191645",
            terms_dated: "Deribit Exchange Membership Terms, article last updated 2026-08-12",
            evaluated_on: "2026-09-19",
            clause: "The use of market data and/or derived data is for personal use only. You \
                     are not allowed to aggregate, resell, publish, forward or in any other way \
                     process market data and/or derived data (except for personal use) without \
                     prior written approval from us.",
            refuses: &[Usage::Derive, Usage::Trade],
            record: "ADR 0050, amendment of 2026-09-19",
        },
        RefusedSource {
            source_id: "cboe-delayed-option-quotes",
            vendor: "Cboe Global Markets, Inc.",
            terms_url: "https://www.cboe.com/terms",
            terms_dated: "Terms and Conditions for Use of Cboe Websites, last updated \
                          2022-11-16",
            evaluated_on: "2026-09-19",
            clause: "You may view, print and download one copy of the Materials for your \
                     personal non-commercial use in connection with products and services \
                     offered by Cboe [...] You may not otherwise copy, reproduce, alter, store \
                     either in hard copy or in an electronic retrieval system, license, \
                     transmit, display, broadcast, create a derivative work (for example, a \
                     financial product, service or index) from, use to verify or correct other \
                     data or information, publish, rent, sublicense, distribute, or otherwise \
                     use in whole or in part in any other manner the Materials without Cboe's \
                     prior written consent",
            refuses: &[Usage::Derive, Usage::Trade],
            record: "ADR 0050, amendment of 2026-09-19",
        },
        RefusedSource {
            source_id: "okx-options",
            vendor: "OKX",
            terms_url: "https://www.okx.com/help/terms-of-service",
            terms_dated: "OKX Terms of Service, last updated 2026-09-17",
            evaluated_on: "2026-09-19",
            clause: "You agree that you will not copy, transmit, distribute, sell, license, \
                     reverse engineer, modify, publish, or participate in the transfer or sale \
                     of, create derivative works from, or in any other way, exploit any of our \
                     products and Services. [...] You may not use the OKX Platform or the \
                     Services for any commercial purpose unless otherwise explicitly authorized \
                     by OKX.",
            refuses: &[Usage::Derive, Usage::Trade],
            record: "ADR 0050, amendment of 2026-09-19",
        },
        RefusedSource {
            source_id: "hkex-option-daily-reports",
            vendor: "Hong Kong Exchanges and Clearing Limited",
            terms_url: "https://www.hkex.com.hk/Global/Exchange/Terms-of-Use?sc_lang=en",
            terms_dated: "HKEX Website Terms of Use, last updated 2025-08-19",
            evaluated_on: "2026-09-19",
            clause: "Unless HKEX or relevant third-parties has/have given you express written \
                     permission, you are not permitted to, directly or indirectly and whether \
                     or not for gain: [...] (ii) create or compile derivative works (including, \
                     without limitation, through framing or systematic retrieval to create \
                     collections, compilations, databases or directories) from the Information \
                     or any part of it; (iii) use any programmatic, scripted or other mechanical \
                     means to access this Website or any Information",
            refuses: &[Usage::Derive, Usage::Trade],
            record: "ADR 0050, amendment of 2026-09-19",
        },
    ]
}

/// The refusal for a source on the register, worded so an operator goes to
/// the vendor's clause and not looking for a missing catalogue entry.
fn refusal_on_record(refused: &RefusedSource) -> Error {
    let usages = refused
        .refuses
        .iter()
        .map(Usage::as_str)
        .collect::<Vec<_>>()
        .join(" and ");
    Error::denied(format!(
        "{} is refused on {}'s own terms, read on {} ({}, {}): \"{}\". That clause refuses \
         {usages}, and both are required. The terms were read and the answer was no — see {} \
         — so this is not a missing catalogue entry; re-evaluating the source means changing \
         the refusal register under review, with the clause that changed",
        refused.source_id,
        refused.vendor,
        refused.evaluated_on,
        refused.terms_url,
        refused.terms_dated,
        refused.clause,
        refused.record,
    ))
}

/// Admit a source for the loop's use, or refuse it with the reason.
///
/// Called by a composition root before
/// [`qip_market_ingestion::connector_feed::ConnectorFeed::open`], which is
/// what makes the ordering the rule demands — evaluation, then use — a
/// property of the code path rather than of anyone's memory.
pub fn admit(
    source_id: &str,
    manifest_class: LicensingClass,
    now: Timestamp,
) -> Result<LicensingDecision> {
    admit_from(&catalogue()?, source_id, manifest_class, now)
}

/// The same admission against a caller-supplied catalogue.
///
/// Split from [`admit`] so the refusal arms are testable with entries the
/// real catalogue must never contain — a research-only licence, a class
/// disagreement — without weakening the real catalogue to host them.
///
/// Consults [`RegistrationRegistry::shipped`], which declares every known
/// source's requirement and records no registration: a root that calls this
/// can open the keyless sources and nothing that needs an account. A root
/// with the owner's registration records passes them to
/// [`admit_from_registered`] instead.
pub fn admit_from(
    entries: &[CatalogueEntry],
    source_id: &str,
    manifest_class: LicensingClass,
    now: Timestamp,
) -> Result<LicensingDecision> {
    admit_from_registered(
        entries,
        &RegistrationRegistry::shipped(),
        source_id,
        manifest_class,
        now,
    )
}

/// [`admit`] with the owner's registration records.
pub fn admit_registered(
    registrations: &RegistrationRegistry,
    source_id: &str,
    manifest_class: LicensingClass,
    now: Timestamp,
) -> Result<LicensingDecision> {
    admit_from_registered(&catalogue()?, registrations, source_id, manifest_class, now)
}

/// The full gate: the catalogue's licensing evaluation, then the registry's
/// answer to who registered.
///
/// The order is the order the work happens in. The terms are read first —
/// a registration record carries the instant they were read — so a source
/// whose terms are unread is refused for that, and only a source whose terms
/// admit it is then asked who holds its account. An operator paged on the
/// first refusal goes to the terms; on the second, to the runbook.
pub fn admit_from_registered(
    entries: &[CatalogueEntry],
    registrations: &RegistrationRegistry,
    source_id: &str,
    manifest_class: LicensingClass,
    now: Timestamp,
) -> Result<LicensingDecision> {
    // Before the catalogue, on purpose. A source whose terms were read and
    // refuse it is refused by that clause whatever the caller's catalogue
    // says, so writing an entry cannot admit it; only editing the register
    // can, and that edit names the clause that changed.
    if let Some(refused) = refusals()
        .iter()
        .find(|refused| refused.source_id == source_id)
    {
        return Err(refusal_on_record(refused));
    }
    let entry = entries
        .iter()
        .find(|entry| entry.source_id == source_id)
        .ok_or_else(|| {
            Error::denied(format!(
                "{source_id:?} has no licensing evaluation in the catalogue, so its terms have \
                 not been read and it is refused. Evaluate the source's terms, write the entry, \
                 and have it reviewed — the catalogue is code on purpose"
            ))
        })?;
    if entry.expected_class != manifest_class {
        return Err(Error::denied(format!(
            "the manifest for {source_id} declares licensing class `{}` and the catalogue's \
             evaluation was written against `{}`. Two claims about one licence disagree, so \
             neither is treated as current; re-read the terms and update both together",
            format_args!("{manifest_class:?}"),
            format_args!("{:?}", entry.expected_class)
        )));
    }
    for usage in REQUIRED_USAGES {
        entry
            .posture
            .legality_for(usage, now)
            .require_permitted(&format!("{source_id} for {}", usage.as_str()))?;
    }
    // The premise of the whole module, kept honest mechanically: the source
    // being admitted is one the feed can actually open, or the catalogue has
    // an entry for something that cannot exist and the evaluation is
    // decoration.
    if !KNOWN_SOURCES.contains(&source_id) {
        return Err(Error::invalid(format!(
            "{source_id} is catalogued but no connector in this build carries it"
        )));
    }
    // Who registered. A source needing an account with nobody's name on it
    // is refused here by name, and the refusal says the platform will not
    // register on anyone's behalf: the request for a scraper that signs up
    // anonymously ends at this line.
    let registration = registrations.standing(source_id)?;
    let licence = entry
        .posture
        .license()
        .map(|license| license.identifier().to_string())
        .ok_or_else(|| {
            Error::denied(format!(
                "{source_id} passed every usage question without a declared licence, which \
                 cannot happen: a posture with no licence answers every usage question \
                 `unknown`"
            ))
        })?;
    Ok(LicensingDecision {
        source_id: source_id.to_string(),
        licence,
        class: manifest_class,
        usages: REQUIRED_USAGES.to_vec(),
        registration,
        decided_at: now,
        gate: GatePassed(()),
    })
}

/// A shipped connector source the licensing gate admitted, as the finder's
/// reference machinery sees it — the second, honestly-labelled door into the
/// source registry (ADR 0057).
///
/// # Why this is not a `RegisteredSource`
///
/// [`crate::decision::RegisteredSource`] is what the discovery pipeline
/// produces for a previously-unknown URL, and it carries what that pipeline
/// gathered: probe evidence (robots.txt, a HEAD, a payload sample), a scored
/// `Routing`, a `SourceLineage` naming where the candidate was found. None of
/// that ever happened to a connector this platform's own authors wrote an
/// adapter for, and a `RegisteredSource` built for one would either claim
/// evidence nobody gathered or hold defaults that read as findings. This type
/// carries exactly what *did* happen — the catalogue's licensing evaluation,
/// the manifest's declared category and schema — and nothing else. A
/// [`crate::reference::DataReference`] records which door it came through in
/// its [`crate::reference::SourceOrigin`], so the two are never confused
/// downstream.
///
/// # The gate is still the only way in
///
/// The one constructor takes a [`LicensingDecision`], which can only be
/// minted by [`admit_from_registered`] after every usage question has been
/// answered `permitted` — see [`GatePassed`]. There is no path from a
/// catalogue entry whose posture is ambiguous, or from a manifest alone, to a
/// value of this type — and none from the wire either. The type serialises,
/// so a banner or a record can carry it, and deliberately does not
/// deserialise: a `Deserialize` derive is a second constructor that takes
/// any caller's word for the licence, which is exactly the gateless door
/// the private `GatePassed` field exists to close. This does not compile:
///
/// ```compile_fail
/// use qip_data_finder::admission::AdmittedSource;
/// let smuggled: AdmittedSource =
///     serde_json::from_str("{}").expect("there is no deserialiser to run");
/// ```
///
/// while the same type, reached through the gate, serialises as it should
/// — the companion that proves the refusal above is about `Deserialize` and
/// not about a path that does not resolve:
///
/// ```
/// use qip_data_finder::admission::{self, AdmittedSource};
/// use qip_market_ingestion::connectors::FrankfurterRatesConnector;
/// # fn main() -> qip_core::error::Result<()> {
/// let manifest = FrankfurterRatesConnector::shipped_manifest()?;
/// let now = qip_core::Timestamp::from_secs(1_760_000_000);
/// let decision = admission::admit(&manifest.source_id, manifest.licensing, now)?;
/// let admitted = AdmittedSource::from_decision(&decision, &manifest)?;
/// let text = serde_json::to_string(&admitted)?;
/// assert!(text.contains("frankfurter-ecb-reference-rates"));
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AdmittedSource {
    source_id: String,
    category: SourceCategory,
    licence: String,
    class: LicensingClass,
    /// The shape the manifest's schema contract declares, in the finder's own
    /// versioned form, so a reference from this door carries the same kind of
    /// schema a discovered source's does.
    schema: SourceSchema,
    /// The manifest's path and fixed query — the locator prefix every fetch
    /// from this source shares.
    endpoint: String,
    admitted_at: Timestamp,
}

impl AdmittedSource {
    /// Bind the gate's decision to the manifest it was decided about.
    ///
    /// Refuses:
    /// * a decision and a manifest that name different sources — two claims
    ///   about two sources are not one admission;
    /// * a manifest whose licensing class disagrees with the class the
    ///   decision recorded, for the reason [`admit_from_registered`] refuses
    ///   the same disagreement: neither claim is then current;
    /// * a `Synthetic` class — a stream this platform generates is not a
    ///   vendor source and has its own origin
    ///   ([`crate::reference::SourceOrigin::Generated`]);
    /// * a manifest that declares no category. A reference must say what kind
    ///   of source it came from, and for a shipped connector the only honest
    ///   answer is the one its authors wrote down.
    pub fn from_decision(decision: &LicensingDecision, manifest: &SourceManifest) -> Result<Self> {
        if decision.source_id != manifest.source_id {
            return Err(Error::invalid(format!(
                "the licensing decision names `{}` and the manifest names `{}`; an admission \
                 is one decision about one source, and these are two",
                decision.source_id, manifest.source_id
            )));
        }
        if decision.class != manifest.licensing {
            return Err(Error::denied(format!(
                "the manifest for {} declares licensing class `{:?}` and the decision that \
                 admitted it was taken against `{:?}`. Two claims about one licence disagree, \
                 so neither is treated as current; re-run the gate against this manifest",
                manifest.source_id, manifest.licensing, decision.class
            )));
        }
        if manifest.licensing == LicensingClass::Synthetic {
            return Err(Error::invalid(format!(
                "{} declares itself `Synthetic`; a stream this platform generates is not a \
                 vendor source and is referenced under its own origin, never through the \
                 catalogue door",
                manifest.source_id
            )));
        }
        let category = manifest.category.ok_or_else(|| {
            Error::invalid(format!(
                "the manifest for {} declares no §7.6.1 category, so a data reference cannot \
                 say what kind of source it came from; declare `category` in the manifest \
                 rather than have this platform guess",
                manifest.source_id
            ))
        })?;
        Ok(Self {
            source_id: manifest.source_id.clone(),
            category,
            licence: decision.licence.clone(),
            class: manifest.licensing,
            schema: schema_of_contract(&manifest.schema),
            endpoint: endpoint_of(manifest),
            admitted_at: decision.decided_at,
        })
    }

    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    pub fn category(&self) -> SourceCategory {
        self.category
    }

    pub fn schema(&self) -> &SourceSchema {
        &self.schema
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// One line for a banner: the licence, the class and the category the
    /// reference ledger will name for every digest from this source, and the
    /// instant the decision was taken. Printed by both composition roots
    /// beside the standing gate's own line, so an operator can read what the
    /// ledger believes about a source rather than infer it from the feed's.
    pub fn describe(&self) -> String {
        format!(
            "{} admitted to the reference ledger under `{}` (class {:?}) as a {} source, \
             decided at {}",
            self.source_id,
            self.licence,
            self.class,
            self.category.as_str(),
            self.admitted_at.to_rfc3339()
        )
    }
}

/// The manifest's declared contract in the finder's own schema form.
///
/// A declared contract names required fields and their kinds; it does not
/// enumerate the fields inside an object it names, so an object lands as
/// `Object { fields: 0 }` — "a record whose fields the contract does not
/// count" — and an array's element type is `Unknown`. Both read as *declared
/// and unexamined*, which is exactly what they are, rather than as a sampled
/// shape nobody sampled.
fn schema_of_contract(contract: &SchemaContract) -> SourceSchema {
    SourceSchema::from_fields(contract.required_fields.iter().map(|field| {
        let kind = match field.kind {
            FieldKind::String | FieldKind::DecimalString | FieldKind::Timestamp => FieldType::Text,
            FieldKind::Number => FieldType::Number,
            FieldKind::Bool => FieldType::Boolean,
            FieldKind::Object => FieldType::Object { fields: 0 },
            FieldKind::Array => FieldType::Array {
                element: Box::new(FieldType::Unknown),
            },
        };
        (field.path.clone(), kind)
    }))
}

/// The manifest's path and fixed query, spelled the way the transport puts
/// them on the wire — the same string `SourceRequest::target` produces for a
/// fetch with no cursor, so a locator recorded at the runtime seam and one
/// derived here agree byte for byte.
fn endpoint_of(manifest: &SourceManifest) -> String {
    let endpoint = &manifest.endpoint;
    if endpoint.query.is_empty() {
        return endpoint.path.clone();
    }
    let query: Vec<String> = endpoint
        .query
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    format!("{}?{}", endpoint.path, query.join("&"))
}

/// The licensing gate held open for as long as a source is being polled.
///
/// # Why an admission is not a decision taken once
///
/// [`admit`] answers *at an instant*: `legality_for(usage, now)` consults the
/// licence's own effective and expiry instants, so its answer is true of `now`
/// and of nothing else. Every composition root calls it once, at start-up,
/// before opening the connector — which is exactly what the rule demands and
/// is sufficient for a process that lives for a poll.
///
/// It stops being sufficient the moment a process is expected to stream for a
/// week. A licence that expires on the third day of a seven-day run has a gate
/// that already knows it expired and is never asked again; the feed keeps
/// polling, and the records keep arriving stamped with a class the terms no
/// longer grant. No catalogue entry in this build carries an expiry today, so
/// nothing is presently mis-serving — which is the whole reason to close it
/// now rather than after the first entry that does. `SourceLicense::expiring_at`
/// exists, `legality_for` honours it, and `an_expired_licence_stops_granting_what_it_used_to`
/// in `tests/legality.rs` proves the mechanism works. A mechanism that works
/// and is consulted once is a control that cannot fire.
///
/// So this type asks the whole gate again — the catalogue, the class
/// agreement, both usages, and the registration — every time the caller is
/// about to use the source, and refuses when any of them has stopped saying
/// yes. It holds no cached verdict on purpose: a cache is a second claim about
/// one licence, and the point of re-asking is that the first claim may have
/// gone stale.
#[derive(Debug)]
pub struct StandingAdmission {
    /// The catalogue, held rather than rebuilt.
    ///
    /// Not a cached verdict — a cached *input*. The entries are code: a change
    /// to a licence is a change to [`catalogue`], reviewed and redeployed, and
    /// cannot happen inside a running process. What moves between one check
    /// and the next is `now`, and that is exactly what is re-evaluated. Holding
    /// the answer would be the mistake; holding the question is not.
    entries: Vec<CatalogueEntry>,
    source_id: String,
    manifest_class: LicensingClass,
    registrations: RegistrationRegistry,
    opened_at: Timestamp,
    /// The furthest instant the gate has been asked about, whatever it
    /// answered. See [`StandingAdmission::check`] for why the answer must not
    /// govern it.
    horizon: Timestamp,
    /// The last instant at which this source was licensed.
    last_granted: Timestamp,
    checks: u64,
}

impl StandingAdmission {
    /// Run the gate for the first time and keep it open, or refuse.
    ///
    /// The first check is the admission the rule requires *before* the source
    /// is used: this returns an error, and no `StandingAdmission` exists, when
    /// the source is not admissible. A caller therefore cannot hold one of
    /// these for a source that was never admitted.
    pub fn open(
        registrations: RegistrationRegistry,
        source_id: &str,
        manifest_class: LicensingClass,
        now: Timestamp,
    ) -> Result<(Self, LicensingDecision)> {
        Self::over(catalogue()?, registrations, source_id, manifest_class, now)
    }

    /// The same, against a caller-supplied catalogue.
    ///
    /// Split from [`Self::open`] for the reason [`admit_from`] is split from
    /// [`admit`]: the arm worth testing is a licence that expires *between* two
    /// checks, and no entry in the real catalogue expires — so exercising it
    /// through [`catalogue`] would mean putting an expiry into the shipped
    /// evaluation of a real vendor's terms to satisfy a test.
    pub fn over(
        entries: Vec<CatalogueEntry>,
        registrations: RegistrationRegistry,
        source_id: &str,
        manifest_class: LicensingClass,
        now: Timestamp,
    ) -> Result<(Self, LicensingDecision)> {
        let decision =
            admit_from_registered(&entries, &registrations, source_id, manifest_class, now)?;
        Ok((
            Self {
                entries,
                source_id: source_id.to_string(),
                manifest_class,
                registrations,
                opened_at: now,
                horizon: now,
                last_granted: now,
                checks: 1,
            },
            decision,
        ))
    }

    /// Ask the gate again, at the instant the caller is about to poll.
    ///
    /// Refuses an instant earlier than any this gate has already been asked
    /// about. A poll loop's horizon only moves forward, so a backwards instant
    /// is either a clock that has stepped or a caller replaying — and in both
    /// cases the licence question would be answered about a moment that has
    /// passed, which is the one way to make an expired licence keep granting.
    /// Refused rather than clamped: a clamped instant would answer the question
    /// the caller did not ask and say nothing about it.
    ///
    /// # Why the horizon moves on a refusal too
    ///
    /// It did not, in the first version of this type, and the test written to
    /// prove the guard is what found it. With the horizon advanced only on a
    /// grant, a caller whose poll at `T` was refused for expiry could ask again
    /// about the admission instant and be granted — the guard was intact
    /// against a clock stepping back and useless against the only case in which
    /// anyone would want to step it back. So the horizon is the furthest
    /// instant the gate has been *asked* about, and [`Self::last_granted`] is
    /// the last instant it said yes. Two facts, because a gate that refused at
    /// noon and last granted at eleven is a different state from one that has
    /// not been asked since eleven, and one field cannot say which.
    pub fn check(&mut self, now: Timestamp) -> Result<LicensingDecision> {
        if now < self.horizon {
            return Err(Error::invalid(format!(
                "the licensing gate for `{}` has been asked about {} and is now being asked about \
                 {}, which is earlier. A licence question answered about a past instant is how an \
                 expired licence keeps granting; move the caller's horizon forward, or re-open \
                 the admission at the instant it means",
                self.source_id,
                self.horizon.to_rfc3339(),
                now.to_rfc3339()
            )));
        }
        // Before the verdict, and deliberately: a refused question has still
        // been asked, and letting the horizon depend on the answer is what made
        // the guard escapable.
        self.horizon = now;
        let decision = admit_from_registered(
            &self.entries,
            &self.registrations,
            &self.source_id,
            self.manifest_class,
            now,
        )?;
        self.last_granted = now;
        self.checks = self.checks.saturating_add(1);
        Ok(decision)
    }

    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    /// How many times the gate has answered yes, including the admission.
    ///
    /// A refusal does not increment it: a consultation that ended in a refusal
    /// is not a moment at which this source was licensed, and a counter that
    /// rose on refusals would let a feed refused on every poll read as a feed
    /// checked on every poll.
    ///
    /// The number that distinguishes a gate consulted on every poll from one
    /// consulted at start-up. An operator comparing it against the ledger's
    /// poll count is checking the claim this type makes, rather than believing
    /// it.
    pub const fn checks(&self) -> u64 {
        self.checks
    }

    pub const fn opened_at(&self) -> Timestamp {
        self.opened_at
    }

    /// The furthest instant the gate has been asked about, granted or refused.
    pub const fn horizon(&self) -> Timestamp {
        self.horizon
    }

    /// The last instant at which this source was licensed.
    pub const fn last_granted(&self) -> Timestamp {
        self.last_granted
    }

    pub fn describe(&self) -> String {
        format!(
            "licensing for `{}`: opened {}, {} check(s), last granted at {}, asked to {}",
            self.source_id,
            self.opened_at.to_rfc3339(),
            self.checks,
            self.last_granted.to_rfc3339(),
            self.horizon.to_rfc3339()
        )
    }
}

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts instead of returning an error is a bug. A test that
// returns `Result` so it can use `?` on the gate it is exercising still has to
// assert, and the abort is the reporting mechanism rather than a defect.
#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;

    fn now() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    #[test]
    fn the_catalogued_source_is_admitted_with_its_licence_named_and_an_unknown_one_is_refused()
    -> Result<()> {
        // The premise first: the shipped manifest's class is what the gate
        // will be handed in production, so admitting with anything else here
        // would test a path the composition root never takes.
        let class =
            qip_market_ingestion::connector_feed::shipped_class("frankfurter-ecb-reference-rates")?;
        let decision = admit("frankfurter-ecb-reference-rates", class, now())?;
        assert_eq!(decision.licence, "ecb-reference-rates-via-frankfurter");
        assert_eq!(decision.class, LicensingClass::Public);
        assert_eq!(decision.usages, vec![Usage::Derive, Usage::Trade]);
        assert_eq!(decision.decided_at, now());
        // The banner line names the licence and both usages; a decision an
        // operator cannot read is a gate they cannot tell ran.
        let line = decision.describe();
        assert!(
            line.contains("ecb-reference-rates-via-frankfurter")
                && line.contains("derive and trade"),
            "the decision does not say what admitted the source: {line}"
        );

        let refused = admit("some-unevaluated-endpoint", class, now());
        assert!(
            refused.is_err(),
            "a source with no licensing evaluation was admitted, so terms \
             nobody read were treated as read"
        );
        Ok(())
    }

    /// The source §38.3's tolerance formula needed and did not have. It goes
    /// through the *same* gate as every other entry — the shipped manifest's
    /// class, both usage questions, the registration question — and no arm of
    /// that gate was relaxed to admit it. A number that judges whether the
    /// books balance is the last place a source nobody evaluated belongs.
    #[test]
    fn the_ecb_key_interest_rates_are_admitted_for_trade_under_the_ecb_s_own_terms() -> Result<()> {
        // Premise: the class the gate is handed is the shipped manifest's, so
        // this is the call the composition root makes and not a class chosen
        // to make the entry agree.
        let class = qip_market_ingestion::connector_feed::shipped_class("ecb-key-interest-rates")?;
        assert_eq!(class, LicensingClass::Public);
        let decision = admit("ecb-key-interest-rates", class, now())?;
        assert_eq!(decision.licence, "ecb-website-copyright-free-use");
        assert_eq!(decision.usages, vec![Usage::Derive, Usage::Trade]);
        assert_eq!(decision.registration, RegistrationStanding::Keyless);
        // The banner line, which is the only way an operator can tell this
        // gate ran from a gate that never did.
        let line = decision.describe();
        assert!(
            line.contains("ecb-website-copyright-free-use")
                && line.contains("derive and trade")
                && line.contains("keyless; no registration needed"),
            "the decision does not say what admitted the source: {line}"
        );
        Ok(())
    }

    /// The source that gives §38.3's fiat row a rate in the currency the
    /// desk's own cash book is actually denominated in. It goes through the
    /// *same* gate as every other entry — the shipped manifest's class, both
    /// usage questions, the registration question — and no arm of that gate
    /// was relaxed to admit it. The euro entry above proved the machinery and
    /// could not answer for a dollar book; this one answers, and it does so
    /// because the New York Fed's own Terms of Use were read, not because a
    /// dollar number was wanted.
    #[test]
    fn the_new_york_feds_effective_federal_funds_rate_is_admitted_for_trade_under_its_terms_of_use()
    -> Result<()> {
        // Premise: the class the gate is handed is the shipped manifest's, so
        // this is the call the composition root makes and not a class chosen
        // to make the entry agree.
        let class = qip_market_ingestion::connector_feed::shipped_class("nyfed-effr")?;
        assert_eq!(class, LicensingClass::Public);
        let decision = admit("nyfed-effr", class, now())?;
        assert_eq!(decision.licence, "nyfed-terms-of-use-reference-rates");
        assert_eq!(decision.usages, vec![Usage::Derive, Usage::Trade]);
        assert_eq!(decision.registration, RegistrationStanding::Keyless);
        // The banner line, which is the only way an operator can tell this
        // gate ran from a gate that never did.
        let line = decision.describe();
        assert!(
            line.contains("nyfed-terms-of-use-reference-rates")
                && line.contains("derive and trade")
                && line.contains("keyless; no registration needed"),
            "the decision does not say what admitted the source: {line}"
        );
        Ok(())
    }

    /// §7.1's Physical class, through the same gate as every other entry.
    ///
    /// The point of asserting it here rather than trusting the catalogue table
    /// is the domain rule's ordering: licensing posture is evaluated *before*
    /// a source is used. A connector on `KNOWN_SOURCES` with no catalogue
    /// entry is refused by `admit` — correct, but for the wrong reason, and an
    /// operator reading that refusal would go looking for a missing entry
    /// rather than for the terms. This proves the entry exists, that it was
    /// written from terms somebody read, and that no arm of the gate was
    /// relaxed to let a new source class in.
    #[test]
    fn the_weather_services_observations_are_admitted_for_trade_under_a_public_domain_grant()
    -> Result<()> {
        // Premise: the class the gate is handed is the shipped manifest's, so
        // this is the call a composition root makes and not a class chosen to
        // make the entry agree.
        let class =
            qip_market_ingestion::connector_feed::shipped_class("nws-station-observations")?;
        assert_eq!(class, LicensingClass::Public);
        let decision = admit("nws-station-observations", class, now())?;
        assert_eq!(
            decision.licence,
            "nws-public-domain-with-attribution-conditions"
        );
        assert_eq!(decision.usages, vec![Usage::Derive, Usage::Trade]);
        assert_eq!(decision.registration, RegistrationStanding::Keyless);
        // The banner line, which is the only way an operator can tell this
        // gate ran from a gate that never did.
        let line = decision.describe();
        assert!(
            line.contains("nws-public-domain-with-attribution-conditions")
                && line.contains("derive and trade")
                && line.contains("keyless; no registration needed"),
            "the decision does not say what admitted the source: {line}"
        );
        Ok(())
    }

    /// The non-endorsement condition is one the NWS terms state and this
    /// platform can only hold by carrying the words.
    ///
    /// Matched on the load-bearing clauses rather than on a length or one
    /// word: "public domain" alone would be satisfied by a sentence that
    /// claimed the content *as this platform's own*, which is the first thing
    /// the terms forbid.
    #[test]
    fn the_weather_attribution_names_the_publisher_and_disclaims_endorsement() {
        let attribution =
            qip_market_ingestion::connectors::NwsStationObservationsConnector::ATTRIBUTION;
        assert!(
            attribution.contains("National Weather Service"),
            "the attribution does not name the publisher: {attribution}"
        );
        assert!(
            attribution.contains("public domain"),
            "the attribution does not state the content is not this platform's own: {attribution}"
        );
        assert!(
            attribution.contains("not affiliated") && attribution.contains("not endorsed"),
            "the attribution does not disclaim the endorsement or affiliation the terms forbid              implying: {attribution}"
        );
    }

    /// The obligation the Terms attach to *presentation* has to be a thing the
    /// platform holds, not a sentence in this module. The connector owns the
    /// completed notice, and the catalogue entry is the reading that says it is
    /// required; a text that drifted from the Terms' own wording would satisfy
    /// nobody, so the load-bearing clauses are matched here rather than a
    /// length or a substring of one word.
    #[test]
    fn the_reference_rate_notice_the_terms_require_is_carried_verbatim_by_the_connector() {
        let notice = qip_market_ingestion::connectors::NyFedEffrConnector::REFERENCE_RATE_NOTICE;
        // Premise: the notice is a sentence and not an empty constant.
        assert!(notice.len() > 100, "the notice is {notice:?}");
        for clause in [
            "is subject to the Terms of Use posted at newyorkfed.org",
            "is not responsible for publication of the",
            "does not sanction or endorse any particular republication",
            "has no liability for your use",
        ] {
            assert!(
                notice.contains(clause),
                "the notice drops the clause {clause:?}: {notice}"
            );
        }
        // And the brackets the Terms say the publisher must complete are
        // completed. A notice shipped with `[NAME OF PUBLISHER]` still in it
        // would read as compliance and name nobody.
        assert!(
            !notice.contains('[') && !notice.contains(']'),
            "the notice still carries an uncompleted bracket: {notice}"
        );
        assert!(
            notice.contains("Effective Federal Funds Rate (EFFR)")
                && notice.contains("EFFR by the"),
            "the notice does not name the data and the publisher: {notice}"
        );
    }

    #[test]
    fn a_keyless_source_is_admitted_as_before_and_its_decision_says_keyless() -> Result<()> {
        // Premise: the shipped registry declares the source keyless, so an
        // admission below is the keyless arm and not a record nobody made.
        assert_eq!(
            RegistrationRegistry::shipped().requirement("coinbase-spot-ticker"),
            Some(crate::registration::RegistrationRequirement::Keyless)
        );
        let class = qip_market_ingestion::connector_feed::shipped_class("coinbase-spot-ticker")?;
        let decision = admit("coinbase-spot-ticker", class, now())?;
        assert_eq!(decision.registration, RegistrationStanding::Keyless);
        assert!(
            decision
                .describe()
                .contains("keyless; no registration needed"),
            "the banner does not say the source is keyless: {}",
            decision.describe()
        );
        Ok(())
    }

    #[test]
    fn a_source_needing_an_account_is_refused_by_the_gate_until_the_owner_records_a_registration()
    -> Result<()> {
        // A catalogue entry the real catalogue does not yet contain: Alpaca
        // with its terms read and every usage granted. Under that entry the
        // licensing questions all pass, so the refusal below can only be the
        // registration question — and it must name the requirement, name who
        // registers, and say the platform will not do it for them.
        let terms_read = vec![CatalogueEntry {
            source_id: "alpaca-daily-bars",
            expected_class: LicensingClass::Restricted,
            posture: LicensingPosture::declared(SourceLicense::new(
                "alpaca-terms-as-read",
                [Usage::Research, Usage::Derive, Usage::Trade],
            )?),
        }];
        // Premise: the usage questions pass, and the shipped registry says
        // the source needs an account and holds no record for it.
        assert!(
            terms_read[0]
                .posture
                .legality_for(Usage::Trade, now())
                .is_permitted()
        );
        let shipped = RegistrationRegistry::shipped();
        assert!(
            shipped
                .requirement("alpaca-daily-bars")
                .is_some_and(|requirement| requirement.needs_registration())
        );
        assert!(shipped.record("alpaca-daily-bars").is_none());

        let refused = admit_from_registered(
            &terms_read,
            &shipped,
            "alpaca-daily-bars",
            LicensingClass::Restricted,
            now(),
        )
        .expect_err("a source needing an account was admitted with nobody registered");
        let message = refused.message();
        assert!(
            message.contains("`alpaca-daily-bars` requires an account with the venue"),
            "the refusal does not name the source and its requirement: {message}"
        );
        assert!(
            message.contains("owner must register with the venue under their own identity"),
            "the refusal does not say who must register: {message}"
        );
        assert!(
            message.contains(crate::registration::NOT_OFFERED),
            "the refusal does not say anonymous registration is not offered: {message}"
        );

        // With the owner's record, the same entry admits and the decision
        // carries the operator's name for the banner.
        let secret =
            qip_market_ingestion::connector::manifest::SecretRef::new("QIP_ALPACA_API_SECRET_KEY")?;
        let registered = shipped.with_record(crate::registration::RegistrationRecord::new(
            "alpaca-daily-bars",
            "desk-owner",
            now(),
            "https://alpaca.markets/terms-and-conditions",
            secret,
        )?)?;
        let decision = admit_from_registered(
            &terms_read,
            &registered,
            "alpaca-daily-bars",
            LicensingClass::Restricted,
            now(),
        )?;
        match &decision.registration {
            RegistrationStanding::Registered { record } => {
                assert_eq!(record.operator(), "desk-owner");
            }
            RegistrationStanding::Keyless => panic!("an account source was admitted as keyless"),
        }
        assert!(
            decision.describe().contains("registered by desk-owner"),
            "the banner does not name who registered: {}",
            decision.describe()
        );
        Ok(())
    }

    #[test]
    fn a_class_disagreement_between_manifest_and_catalogue_refuses_the_source() {
        // Two claims about one licence. If the manifest is edited to `public`
        // while the catalogue still says `internal` — or the reverse — the
        // safe reading is that neither is current, because whichever edit came
        // second was made without re-reading the terms alongside the other.
        let refused = admit("coinbase-spot-ticker", LicensingClass::Public, now());
        assert!(
            refused.is_err(),
            "a manifest claiming a different licensing class than the \
             catalogue's evaluation was admitted"
        );
    }

    #[test]
    fn a_research_only_licence_never_reaches_the_trading_path() -> Result<()> {
        // The rule's own example, driven through the real gate against an
        // entry the real catalogue must never contain. The terms were read
        // and they grant research; asked about the trading path, the answer
        // is forbidden — not unknown — and the source does not open.
        let research_only = vec![CatalogueEntry {
            source_id: "research-feed",
            expected_class: LicensingClass::Internal,
            posture: LicensingPosture::declared(SourceLicense::new(
                "research-only-terms",
                // Research and derivation granted, trading withheld — so the
                // refusal below can only come from the Trade question, and a
                // gate that stopped asking it goes red here.
                [Usage::Research, Usage::Derive],
            )?),
        }];
        // Premise: everything short of trading is permitted, so the refusal
        // below can only be the Trade question being asked and answered.
        assert!(
            research_only[0]
                .posture
                .legality_for(Usage::Derive, now())
                .is_permitted()
        );
        let refused = admit_from(
            &research_only,
            "research-feed",
            LicensingClass::Internal,
            now(),
        )
        .expect_err("a research-only licence was admitted onto the trading path");
        // The refusal must be the Trade question's answer, by name. A version
        // of this test in another root once accepted any error, and a
        // mutation that deleted the Trade question passed it anyway — the
        // entry also fails the known-sources integrity check, and that
        // refusal was mistaken for this one.
        assert!(
            refused.message().contains("for trade"),
            "the refusal was not about the trading usage: {}",
            refused.message()
        );
        Ok(())
    }

    #[test]
    fn the_adr_0034_candidates_whose_terms_are_unread_are_refused_by_the_real_catalogue()
    -> Result<()> {
        // Not a placeholder catalogue: the shipped one, with the class the
        // shipped manifest declares — the exact call the composition root
        // makes. The refusal must be the Derive question's own answer and
        // must name the terms to read, so an operator paged on it goes to
        // the document and not to this file.
        for source_id in ["kalshi-markets", "alpaca-daily-bars"] {
            // Premise: the source is one the feed can open by name, so the
            // refusal below cannot be the known-sources integrity check.
            assert!(
                qip_market_ingestion::connector_feed::KNOWN_SOURCES.contains(&source_id),
                "{source_id} is not a source the build carries"
            );
            let class = qip_market_ingestion::connector_feed::shipped_class(source_id)?;
            let entry = catalogue()?
                .into_iter()
                .find(|entry| entry.source_id == source_id)
                .unwrap_or_else(|| panic!("{source_id} has no catalogue entry"));
            assert!(
                entry.posture.license().is_none(),
                "{source_id} carries a declared licence, and ADR 0034 says its terms are unread"
            );
            assert_eq!(entry.expected_class, class);

            let refused = admit(source_id, class, now()).expect_err(&format!(
                "{source_id} was admitted, so terms nobody read were treated as read"
            ));
            let message = refused.message();
            assert!(
                message.contains(&format!(
                    "{source_id} for derive may not be collected because its legality is \
                     undetermined"
                )),
                "the refusal is not the Derive question's answer: {message}"
            );
            assert!(
                message.contains("https://"),
                "the refusal does not name the terms to read: {message}"
            );
        }
        Ok(())
    }

    #[test]
    fn an_unevaluated_posture_is_refused_as_undetermined_not_admitted_by_default() -> Result<()> {
        // The gate's third arm: terms nobody found. `Undetermined` answers
        // every usage `unknown`, and unknown is not permission — a source
        // entered in the catalogue as a placeholder must still not open.
        let placeholder = vec![CatalogueEntry {
            source_id: "frankfurter-ecb-reference-rates",
            expected_class: LicensingClass::Public,
            posture: LicensingPosture::Undetermined,
        }];
        let refused = admit_from(
            &placeholder,
            "frankfurter-ecb-reference-rates",
            LicensingClass::Public,
            now(),
        )
        .expect_err("a source with no terms located was admitted");
        // The phrase is the usage question's own answer, from
        // `Legality::require_permitted`, and not merely the word: a first
        // version of this test matched `undetermined` and was satisfied by
        // the backstop refusal below the usage loop when a mutation deleted
        // the loop — the word was a substring of a different refusal.
        assert!(
            refused
                .message()
                .contains("for derive may not be collected because its legality is undetermined"),
            "the refusal is not the Derive question's answer: {}",
            refused.message()
        );
        Ok(())
    }

    /// ADR 0050's evaluation, done: the option-quote sources whose terms
    /// were read are refused **by the clause**, not as unevaluated.
    ///
    /// Before the refusal register a refused source had no entry, and
    /// `admit` said its terms had not been read — a false statement about
    /// work that was done, and the exact sentence that would send the next
    /// lane off to read them again. This asserts the refusal names the
    /// vendor, the document and the clause, and does *not* claim the terms
    /// are unread.
    #[test]
    fn an_option_quote_source_refused_on_its_read_terms_is_refused_by_the_clause_and_not_as_unread()
    {
        // Premise: the source is on the register and nowhere else, so the
        // refusal below can only come from the register.
        let refused = refusals()
            .into_iter()
            .find(|refused| refused.source_id == "deribit-options")
            .expect("the Deribit evaluation is on the refusal register");
        assert!(!qip_market_ingestion::connector_feed::KNOWN_SOURCES.contains(&"deribit-options"));
        let message = admit("deribit-options", LicensingClass::Restricted, now())
            .expect_err("a source whose terms refuse it was admitted")
            .message()
            .to_string();
        // The delimited clause, the vendor and the document — each one
        // something a reader can check against the publisher rather than
        // against this file.
        assert!(
            message.contains("for personal use only"),
            "the refusal does not quote the clause: {message}"
        );
        assert!(
            message.contains(refused.vendor) && message.contains(refused.terms_url),
            "the refusal does not name the vendor and the document: {message}"
        );
        assert!(
            message.contains("refuses derive and trade"),
            "the refusal does not say which of the gate's questions the clause answers: {message}"
        );
        assert!(
            !message.contains("have not been read"),
            "the refusal claims terms that were read are unread: {message}"
        );
    }

    /// The register overrides the catalogue, so a refused source cannot be
    /// admitted by writing an entry for it.
    ///
    /// The failure this prevents is the quiet one: a lane that finds no
    /// catalogue entry for Deribit, writes one under a licence identifier of
    /// its own choosing, and is admitted by a gate that only ever looked at
    /// the catalogue. Here the entry grants everything and the gate still
    /// refuses on the clause. What this proves is the override, not the
    /// order of the two lookups: a mutation moving the register check after
    /// the catalogue lookup left this test passing, because an entry still
    /// could not admit — and failed the test above, whose no-entry path is
    /// where the order shows.
    #[test]
    fn a_refused_source_cannot_be_admitted_by_writing_a_catalogue_entry_for_it() -> Result<()> {
        let entries = vec![CatalogueEntry {
            source_id: "deribit-options",
            expected_class: LicensingClass::Restricted,
            posture: LicensingPosture::declared(SourceLicense::new(
                "an-identifier-nobody-reviewed",
                [
                    Usage::Research,
                    Usage::Derive,
                    Usage::Trade,
                    Usage::Redistribute,
                ],
            )?),
        }];
        // Premise: the entry on its own would pass every usage question.
        for usage in REQUIRED_USAGES {
            assert!(entries[0].posture.legality_for(usage, now()).is_permitted());
        }
        let message = admit_from(
            &entries,
            "deribit-options",
            LicensingClass::Restricted,
            now(),
        )
        .expect_err("a catalogue entry admitted a source the register refuses")
        .message()
        .to_string();
        assert!(
            message.contains("for personal use only"),
            "the refusal is not the register's clause: {message}"
        );
        Ok(())
    }

    /// A refused source has neither a connector nor a catalogue entry, and
    /// every refusal carries evidence a reader can check.
    ///
    /// A connector for a refused source is ADR 0050's alternative (f) — code
    /// the gate can never open — and a catalogue entry for one is a second
    /// claim about a licence the register already answered. Both are held
    /// here so that adding either fails a test naming the register.
    #[test]
    fn no_refused_source_has_a_connector_or_a_catalogue_entry() -> Result<()> {
        let refused = refusals();
        assert!(
            !refused.is_empty(),
            "the register is empty, so this guards nothing"
        );
        let catalogued = catalogue()?;
        for entry in &refused {
            assert!(
                !qip_market_ingestion::connector_feed::KNOWN_SOURCES.contains(&entry.source_id),
                "{} has a connector in this build; its terms refuse it, so the connector is \
                 code the gate can never open",
                entry.source_id
            );
            assert!(
                catalogued
                    .iter()
                    .all(|catalogued| catalogued.source_id != entry.source_id),
                "{} is both refused and catalogued, which is two claims about one licence",
                entry.source_id
            );
            assert!(
                entry.terms_url.starts_with("https://") && !entry.clause.trim().is_empty(),
                "{} carries no checkable evidence",
                entry.source_id
            );
            for usage in REQUIRED_USAGES {
                assert!(
                    entry.refuses.contains(&usage),
                    "{} is on the register without refusing {}, so it is not a refusal",
                    entry.source_id,
                    usage.as_str()
                );
            }
        }
        Ok(())
    }
}
