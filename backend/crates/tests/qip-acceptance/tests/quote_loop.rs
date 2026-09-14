//! The quote loop's boundary, which no single crate can assert.
//!
//! Blueprint §29.1 is the most dangerous row in the delivery register to
//! close, because **in every real venue quoting is order submission**. A quote
//! loop that grew a venue seam would not fail any test in
//! `qip-execution-engine` or `qip-kernel`: each of those crates would still be
//! internally consistent, and the property that was lost is a property of the
//! relationship between them.
//!
//! So the four claims here are claims about the seam:
//!
//! 1. no production line of the quoting arithmetic or the origination gate
//!    reaches a broker, an order or a venue;
//! 2. neither does the kernel module that composes them;
//! 3. a real platform, driven through a real pass, prices a quote and creates
//!    no order and no fill;
//! 4. an origination mandate cannot be decoded into existence, so §29.3's five
//!    gates are the only door to one.
//!
//! `.claude/rules/01-security-and-safety.md` names three layers of the
//! paper-trading boundary and says a request for a live path has never yet
//! been legitimate. None of them is weakened by anything this suite covers,
//! and this suite is what would notice if a later lane weakened them here.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable.
#![allow(clippy::panic_in_result_fn)]

use qip_acceptance::read;
use qip_core::ids::ObjectId;
use qip_core::time::Timestamp;
use qip_core::{Context, Decimal, dec};
use qip_financial::asset_class::{InstrumentType, Sector};
use qip_financial::object::FinancialObject;
use qip_financial::quality::Provenance;
use qip_financial::universe::Universe;
use qip_kernel::config::PlatformConfig;
use qip_kernel::cycle::{Stage, StageOutcome};
use qip_kernel::platform::Platform;
use qip_kernel::quote_loop::{QuoteLoopReview, review};
use qip_market::book::{BookLevel, OrderBook};
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::Telemetry;
use qip_risk::limits::{Limit, LimitKind, LimitSet};

/// Source with comments and every `#[cfg(test)]` item removed, and
/// **everything after one kept**.
///
/// Both removals are load-bearing and neither is a convenience. The doc
/// comments in these files *describe* the boundary at length — they say the
/// words "broker", "venue" and "order" repeatedly, because a reader standing
/// there needs to know what the module must never do — so a scan that read
/// them would find the forbidden token in the very prose that forbids it. And
/// the kernel module's own tests call `Platform::submit_order` deliberately,
/// to prove that a limit order the platform holds is located in the book; that
/// is a test driving a production door, not the quote loop reaching one.
///
/// **The "and everything after" is not pedantry, and this helper got it wrong
/// first.** It truncated the file at `#[cfg(test)]`, which is a hole a
/// mutation walked straight through: a `#[cfg(test)] mod tests` block is not
/// required to be the last item in a Rust file, and production code written
/// below one was invisible to every scan here. A test module is matched by
/// brace depth and skipped, and the rest of the file is read.
///
/// The truncation is asserted rather than assumed at each call site below: a
/// helper that silently returned an empty string would make every scan here
/// pass forever.
fn production_source(relative: &str) -> String {
    let source = read(relative);
    let mut kept: Vec<&str> = Vec::new();
    let mut lines = source.lines();
    while let Some(line) = lines.next() {
        if line.trim_start().starts_with("//") {
            continue;
        }
        if line.trim() != "#[cfg(test)]" {
            kept.push(line);
            continue;
        }
        // Skip the attributed item by brace depth. Rust requires braces to
        // balance outside string literals, and no test module in the files
        // this suite reads carries an unbalanced brace in a literal — which
        // is asserted rather than trusted: if the depth never closes, the
        // whole tail is dropped and the call sites' length premise fails.
        let mut depth = 0usize;
        let mut opened = false;
        for body in lines.by_ref() {
            depth += body.matches('{').count();
            opened |= depth > 0;
            depth = depth.saturating_sub(body.matches('}').count());
            if opened && depth == 0 {
                break;
            }
        }
    }
    kept.join("\n")
}

/// Tokens that would mean the quote loop had grown a path to a market.
///
/// Each is matched as a delimited fragment with its own punctuation — `Broker`
/// bare would match the word inside `SimulatedBrokerage` and `submit` bare
/// would match `submitted_at`. Substring matching is a trap this workspace has
/// already been bitten by, in a test that passed a mutation deleting the exact
/// value it protected.
const VENUE_REACHING: &[&str] = &[
    "dyn Broker",
    "Broker>",
    "SimulatedBroker::",
    "LiveBroker::",
    "OrderManager::",
    "Order::new(",
    ".submit(",
    "submit_order(",
    "place_order(",
];

#[test]
fn no_production_line_of_the_quoting_arithmetic_can_reach_a_broker_an_order_or_a_venue() {
    // The property the whole lane rests on. A `QuotePair` carries two prices
    // and a size and nothing a venue adapter could act on; this asserts that
    // the modules that produce one hold no seam to the ones that could.
    let files = [
        "backend/crates/services/qip-execution-engine/src/quoting.rs",
        "backend/crates/services/qip-execution-engine/src/origination.rs",
    ];
    for relative in files {
        let source = production_source(relative);
        // The premise, in two halves. A file that had been renamed away or an
        // over-eager comment strip would leave an empty body, and an empty
        // body contains no forbidden token — the scan would pass while
        // checking nothing.
        assert!(
            source.len() > 500,
            "{relative} stripped to {} bytes of production source; the scan below would pass \
             while checking nothing",
            source.len()
        );
        assert!(
            source.contains("pub fn ") || source.contains("pub struct "),
            "{relative} has no public surface left after stripping; the scan is not looking at \
             code"
        );
        for token in VENUE_REACHING {
            assert!(
                !source.contains(token),
                "{relative} names `{token}` in production code. Quoting is order submission in \
                 every real venue, and this platform is paper trading only: a quote here is a \
                 priced intent and must have no path to a market."
            );
        }
    }
}

#[test]
fn the_kernels_quote_loop_composition_reaches_no_broker_and_builds_no_order() {
    // The same claim one layer up, where it is easier to lose: the kernel is
    // the crate that holds a `Broker` and an `OrderManager` together, so a
    // module here is one line away from being able to send what it priced.
    let relative = "backend/crates/runtime/qip-kernel/src/quote_loop.rs";
    let whole = read(relative);
    let source = production_source(relative);
    // The premise: the test module really was cut, and what is left is code.
    assert!(
        whole.len() > source.len() + 1_000,
        "the test module was not removed from {relative}; the scan would be reading its \
         deliberate `submit_order` calls"
    );
    assert!(
        source.contains("pub fn review("),
        "{relative} no longer exposes the entry point a stage calls"
    );
    for token in VENUE_REACHING {
        assert!(
            !source.contains(token),
            "{relative} names `{token}` in production code; the kernel's quote loop composes a \
             price and must compose nothing that sends it"
        );
    }
}

#[test]
fn a_real_platform_pass_prices_a_quote_and_creates_no_order_and_no_fill() {
    // The behavioural half. The two scans above would still pass if the loop
    // reached a market through a type alias nobody spelled out, and they would
    // also pass if the loop had stopped working entirely. This drives a real
    // `Platform` through a real pass and asserts both directions: something was
    // priced, and nothing was sent.
    let mut platform = platform();
    assert_eq!(
        platform.observe(vec![book()]),
        1,
        "the premise: the platform absorbed a two-sided book to price against"
    );
    // And the premise on the other side: nothing had been ordered before.
    assert_eq!(platform.orders().orders().count(), 0);

    let found = QuoteLoopReview::of(&platform, start());
    assert_eq!(found.considered, 1);
    assert_eq!(
        found.quoted.len(),
        1,
        "the pass priced nothing, so the assertion below about sending nothing is vacuous: \
         {:?} / {:?}",
        found.withheld,
        found.unpriceable
    );
    let pair = &found.quoted[0];
    assert!(
        pair.bid < pair.fair_value && pair.fair_value < pair.ask && pair.size.is_positive(),
        "the pass produced something that is not a two-sided quote: {}",
        pair.describe()
    );

    assert_eq!(
        platform.orders().orders().count(),
        0,
        "a quote-loop pass created an order. A quote in this platform is priced and never sent, \
         and this is the assertion that says so."
    );
    assert!(
        platform.orders().fills().is_empty(),
        "a quote-loop pass produced a fill"
    );
    assert!(
        !platform.is_live_capable(),
        "the platform became live-capable; layer two of the paper-trading boundary refuses a \
         live ceiling at start-up and nothing in a quote loop may reach past it"
    );

    // And the stage seam: the one line a stage calls carries the pass into the
    // outcome, so the result cannot be computed and dropped.
    let before = StageOutcome::ran(Stage::Act, 0, "0 order(s) released");
    let after = review(&platform, start(), before.clone());
    assert_ne!(after.detail, before.detail);
    assert!(
        after.detail.contains("quote loop priced"),
        "{}",
        after.detail
    );
}

#[test]
fn an_origination_mandate_cannot_be_decoded_into_existence() {
    // Blueprint §29.3's five gates are worth nothing if a mandate can arrive
    // over a wire. `OriginationMandate` has private fields and one
    // constructor, and this asserts the third leg: it does not derive
    // `Deserialize`, so no payload anywhere — a policy frame, a config file, a
    // cell report — can produce one without going through `admit`.
    let source = read("backend/crates/services/qip-execution-engine/src/origination.rs");
    let at = source
        .find("\npub struct OriginationMandate {")
        .expect("the mandate type is still declared in this file");
    let derive_line = source[..at]
        .lines()
        .next_back()
        .expect("the declaration has a line above it");
    // The premise: the line found really is the derive attribute, not a blank
    // line or a comment. Without this the assertion below passes on any line
    // that happens not to contain the word.
    assert!(
        derive_line.starts_with("#[derive(") && derive_line.contains("Serialize"),
        "the line above the mandate declaration is not its derive attribute: {derive_line}"
    );
    assert!(
        !derive_line.contains("Deserialize"),
        "`OriginationMandate` derives `Deserialize`, so a mandate can be decoded out of a payload \
         and §29.3's five gates are no longer the only door to one: {derive_line}"
    );
}

// --- fixtures ---------------------------------------------------------------

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

fn object() -> ObjectId {
    ObjectId::from_string("obj-AAA")
}

fn platform() -> Platform {
    let config = PlatformConfig::default();
    let (context, _clock) = Context::deterministic(start(), config.seed);
    let mut universe = Universe::new();
    universe
        .insert(
            FinancialObject::builder(
                object(),
                "AAA",
                InstrumentType::CommonStock,
                // Stated rather than defaulted: `LiquidityProfile` has no
                // `Default`, because the one it had asserted a ten-basis-point
                // quote for any instrument at all and two vetoes read exactly
                // that figure.
                qip_financial::costs::LiquidityProfile::listed(dec!("5000000"), 3.0),
            )
            .venue("XNYS")
            .sector(Sector::InformationTechnology)
            .price(dec!("100"))
            .provenance(Provenance::synthetic("test", start()))
            .build(start())
            .expect("a valid object"),
        )
        .expect("insertable");
    let limits = LimitSet::new("quote-loop-acceptance").with(
        Limit::new("max-leverage", LimitKind::MaxLeverage { limit: 2.0 })
            .with_rationale("gross exposure is capped at 2x equity"),
    );
    Platform::new(config, context, Telemetry::silent(), universe, limits).expect("a platform")
}

fn book() -> SensedRecord {
    SensedRecord::Book(Box::new(OrderBook::from_levels(
        object(),
        "XNYS",
        start(),
        vec![BookLevel::new(dec!("99.95"), Decimal::from_int(100))],
        vec![BookLevel::new(dec!("100.05"), Decimal::from_int(100))],
    )))
}

#[test]
fn the_act_stage_carries_the_quote_loop_so_the_wiring_and_not_only_the_module_is_proven() {
    // Every other test in this file reaches `review` directly, or builds a
    // `StageOutcome` by hand and passes it in. Both prove the module and
    // neither says anything about whether a deployed process ever calls it —
    // which is the entire difference between `UNREACHED` and `REACHED` here.
    //
    // This was not a hypothetical gap: disconnecting the `stage_act` call site
    // left every other test in this suite green, so §29.1 and §29.3 would have
    // been recorded as reached on the strength of a suite that could not tell.
    let mut platform = platform();
    let report = platform.run_cycle(start());
    let act = report
        .stage(Stage::Act)
        .expect("the premise failed: the cycle did not run the ACT stage");

    assert!(
        act.detail.contains("quote loop"),
        "the ACT stage does not carry the quote loop, so `quote_loop::review` is reached only \
         by tests and §29.1/§29.3 are UNREACHED whatever this suite's other tests say: {}",
        act.detail
    );

    // And the boundary still holds on the path that now runs it: pricing a
    // quote inside ACT must not have sent anything.
    assert_eq!(
        platform.orders().orders().count(),
        0,
        "the quote loop ran inside ACT and an order exists"
    );
}
