//! R12-WORLD-9023 under ADR 0195: the reflex forecast is a promoted,
//! calibrated distillate a rule names as its expected edge, evaluated in the
//! cell's own pass, and it keeps running when the centre does not answer.
//!
//! The failure this closes is a forecast that exists only while the centre
//! is reachable. A model consulted over the network, or one a cell unloads
//! when its policy ages, is a forecast the cell loses at the moment it is
//! deciding alone, which is the moment ADR 0008 exists for.

// In a test the assertion is the deliverable; the workspace denies
// `panic_in_result_fn` for production code, where it would be a bug.
#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::feature::FeatureValue;
use qip_contracts::message::{BookSide, MarketMessage, MessageBody};
use qip_contracts::policy::{ModelManifest, PolicyPayload, Slot};
use qip_contracts::signal::{SignalKind, StrategyId};
use qip_contracts::venue::{Origin, VenueClass, VenueId, VenueStatus};
use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, ObjectId, Timestamp, dec};
use qip_edge::cell::{Cell, CellConfig, Placer, PricingPolicy, WorkReport};
use qip_edge::envelope::{VerifiedEnvelope, sign_payload};
use qip_edge::feasibility::{Granularity, VenueModel};
use qip_edge::policy::VerifiedPolicy;
use qip_feature_dag::engine::FeatureEngine;
use qip_feature_dag::features::BookPressure;
use qip_feature_dag::state::MarketState;
use qip_orderbook::venue::VenueState;
use qip_protocols::FeedKey;
use qip_protocols::decoder::{Decoder, Diagnostics};
use qip_strategy::catalogue::FeatureCatalogue;
use qip_strategy::compile::{CompiledStrategy, StrategyCompiler};
use qip_strategy::ir::{Expr, Rule, StrategySpec, Type};
use qip_strategy::model::{DistilledModel, ModelCalibration};
use qip_strategy::program::Program;
use std::collections::BTreeMap;

const CELL: &str = "london-1";
const REGION: &str = "europe-west2";
const XLON: &str = "XLON";
const FEED: &str = "feed-a";
const INSTRUMENT: &str = "obj-ACME";
const ENVELOPE_KEY: &[u8] = b"a-cell-envelope-key-for-reflex-forecast-tests";
const POLICY_KEY: &[u8] = b"a-cell-policy-key-for-reflex-forecast-tests";
/// The policy's own lifetime: past this, nothing the centre signed is
/// current, which is what an unreachable centre looks like to a cell.
const POLICY_LIFE: Duration = Duration::from_secs(300);

fn t(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn object() -> ObjectId {
    ObjectId::from_string(INSTRUMENT)
}

fn venue() -> VenueId {
    VenueId::new(XLON)
}

fn d(literal: &str) -> Decimal {
    Decimal::parse(literal).expect("a decimal literal")
}

/// One level per line, `sequence ⇥ object ⇥ B|A ⇥ price ⇥ size`.
#[derive(Debug)]
struct LineDecoder {
    consumed: usize,
    diagnostics: Diagnostics,
}

impl Decoder for LineDecoder {
    fn decode(&mut self, bytes: &[u8], captured_at: Timestamp) -> Result<Vec<MarketMessage>> {
        let text = std::str::from_utf8(bytes).map_err(|e| Error::invalid(e.to_string()))?;
        let mut messages = Vec::new();
        for line in text.lines() {
            let fields: Vec<&str> = line.split('\t').collect();
            let [sequence, name, side, price, size] = fields[..] else {
                return Err(Error::invalid("a line has five fields"));
            };
            let side = match side {
                "B" => BookSide::Bid,
                "A" => BookSide::Ask,
                other => return Err(Error::invalid(format!("side {other} is neither B nor A"))),
            };
            let sequence: u64 = sequence
                .parse()
                .map_err(|_| Error::invalid("a sequence is a number"))?;
            messages.push(MarketMessage::new(
                ObjectId::from_string(name),
                Origin::new(venue(), FEED, 0, sequence),
                MessageBody::LevelSet {
                    side,
                    price: d(price),
                    quantity: d(size),
                    order_count: None,
                },
                captured_at,
                captured_at,
            ));
        }
        self.consumed = bytes.len();
        Ok(messages)
    }

    fn protocol(&self) -> &str {
        "line"
    }

    fn consumed(&self) -> usize {
        self.consumed
    }

    fn diagnostics(&self) -> &Diagnostics {
        &self.diagnostics
    }
}

/// A gateway that accepts every order, on paper.
#[derive(Debug)]
struct PaperGateway;

impl Placer for PaperGateway {
    fn is_simulated(&self) -> bool {
        true
    }

    fn place(
        &mut self,
        _order_id: &str,
        _object_id: &ObjectId,
        _venue: &VenueId,
        _side: BookSide,
        _quantity: Decimal,
        _price: Decimal,
        _at: Timestamp,
    ) -> Result<()> {
        Ok(())
    }
}

fn signed_envelope(strategy: &str) -> Result<VerifiedEnvelope> {
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(strategy),
            CELL,
            dec!("1000000"),
            dec!("100000"),
            dec!("50000"),
            vec![venue()],
            t(0),
            t(7200),
            "alice@example.com",
            signature,
        )
    };
    let unsigned = build("unsigned")?;
    let signature = sign_payload(ENVELOPE_KEY, &unsigned.signing_payload());
    VerifiedEnvelope::verify(build(&signature)?, ENVELOPE_KEY, CELL, t(1))
}

/// The reflex forecast: book pressure over one level, scaled to basis
/// points of edge, with a calibration the centre fitted and promoted. The
/// calibration is part of the digest, so a recalibration is a new model.
fn forecast(name: &str, scale: f64) -> Result<DistilledModel> {
    DistilledModel::linear(name, 0.0, vec![100.0])?
        .with_calibration(ModelCalibration::new(scale, 0.0)?)
}

/// A strategy that always wants in, and states the forecast as its edge.
fn forecasting_strategy(id: &str, model: DistilledModel) -> Result<(CompiledStrategy, Program)> {
    let pressure = BookPressure::key(&object(), 1);
    let catalogue = FeatureCatalogue::new().declaring([(pressure.clone(), Type::Statistic)])?;
    let mut compiler = StrategyCompiler::new(catalogue);
    let rule = Rule::new(
        "always",
        SignalKind::Enter,
        Expr::Flag(true),
        // Eight, so that the narrowing a stale policy applies (0.375) leaves
        // a whole number of lots: an off-lot order would be refused by the
        // lot grid, which is the cell working and not the forecast failing.
        Expr::Exact(d("8")),
        Expr::Statistic(0.5),
        10,
    )
    .with_expected_edge(Expr::Model {
        model,
        inputs: vec![Expr::feature(pressure)],
    });
    let spec =
        StrategySpec::new(StrategyId::new(id), object(), Duration::from_secs(30)).with_rule(rule);
    let compiled = compiler.compile(&spec)?;
    Ok((compiled, compiler.into_program()))
}

/// A cell on one venue charging a fee floor of 1, so on the three lots a
/// stale policy narrows an order of eight to, priced near 100, the fixed
/// cost is about 33 basis points; armed with a 5 bp net-edge margin; holding a manifest that names `models`; and running one
/// forecasting strategy per model.
fn forecasting_cell(models: &[(&str, DistilledModel)]) -> Result<Cell> {
    let fees = VenueModel::new(
        VenueClass::Exchange,
        Granularity::new(dec!("1"), dec!("0.01"), Decimal::ZERO)?,
        dec!("1"),
        None,
    )?;
    let config = CellConfig::new(CELL, REGION)
        .with_venue(venue())
        .with_feasibility(&venue(), fees)
        .with_net_edge_margin_bp(dec!("5"))?;
    let features = FeatureEngine::new(MarketState::default(), Duration::from_secs(5));
    let mut cell = Cell::new(config, features)?;
    cell.protocols_mut().register(
        venue(),
        FEED,
        Box::new(LineDecoder {
            consumed: 0,
            diagnostics: Diagnostics::default(),
        }),
    )?;
    cell.track(VenueState::aggregated(object(), venue(), VenueStatus::Open));
    cell.register_features(vec![Box::new(BookPressure::new(object(), 1))])?;
    let mut payload = PolicyPayload::unproduced(1, CELL, t(5));
    payload.valid_for = POLICY_LIFE;
    payload.trained_models = Slot::produced(
        ModelManifest::unreleased(
            models
                .iter()
                .map(|(_, model)| (model.name().to_string(), model.digest()))
                .collect::<BTreeMap<_, _>>(),
        ),
        t(5),
    );
    cell.apply_policy(
        VerifiedPolicy::verify(payload.signed(POLICY_KEY)?, POLICY_KEY, CELL, t(5))?,
        t(5),
    )?;
    for (id, model) in models {
        let (compiled, program) = forecasting_strategy(id, model.clone())?;
        cell.deploy_with_pricing(
            compiled,
            program,
            signed_envelope(id)?,
            PricingPolicy::Marketable,
        )?;
    }
    Ok(cell)
}

/// A book whose bid outweighs its ask: pressure (500 - 400) / 900.
fn feed_book(cell: &mut Cell, sequence: u64, now: Timestamp) -> Result<usize> {
    let lines = format!(
        "{}\t{INSTRUMENT}\tB\t99\t500\n{}\t{INSTRUMENT}\tA\t101\t400",
        sequence,
        sequence + 1
    );
    cell.on_bytes(&FeedKey::new(venue(), FEED), lines.as_bytes(), now)
}

fn edge_of(report: &WorkReport, strategy: &str) -> Option<f64> {
    report
        .signals
        .iter()
        .find(|signal| signal.strategy.as_str() == strategy)
        .and_then(|signal| signal.expected_edge_bp)
}

fn net_edge_refusals(report: &WorkReport) -> Vec<&str> {
    report
        .refusals
        .iter()
        .filter(|(gate, _)| gate == "net_edge")
        .map(|(_, reason)| reason.as_str())
        .collect()
}

/// R12-WORLD-9023's integration acceptance. The cell evaluates a promoted,
/// calibrated model in its own pass, with no network call (this cell has no
/// link to anything; the gateway is paper), and the model's forecast is the
/// signal's edge. The pass runs after the policy's lifetime has elapsed,
/// which is what an unreachable centre looks like to a cell, and the
/// forecast still runs. Its one effect is the net-edge gate: the forecast
/// below cost plus margin is refused, and the one above it is not.
///
/// Mutations: drop the calibration from the model's evaluation; take the
/// expected edge from anything but the rule's model; disarm the net-edge
/// gate. That the cell was deciding alone is a premise the test asserts (a
/// policy past its life narrows the order of eight to three), not a mutant.
#[test]
fn a_cell_evaluates_its_promoted_calibrated_forecast_in_the_pass_and_keeps_it_when_the_centre_is_gone()
-> Result<()> {
    let rich = forecast("micro-edge-rich", 6.0)?;
    let thin = forecast("micro-edge-thin", 0.5)?;
    // Premise: the calibration is part of what was promoted.
    assert_ne!(
        rich.digest(),
        DistilledModel::linear("micro-edge-rich", 0.0, vec![100.0])?.digest(),
        "a calibrated model shares its uncalibrated form's digest"
    );
    let mut cell = forecasting_cell(&[("rich", rich), ("thin", thin)])?;

    // Well past the policy's lifetime: nothing the centre signed is current.
    let alone = t(5)
        .saturating_add(POLICY_LIFE)
        .saturating_add(Duration::from_secs(600));
    feed_book(&mut cell, 1, alone)?;
    let report = cell.work(alone, &mut PaperGateway)?;
    let pressure = (500.0 - 400.0) / 900.0;
    assert_eq!(
        cell.features().value(&BookPressure::key(&object(), 1)),
        Some(FeatureValue::Statistic(pressure)),
        "premise: the feature the forecast reads was computed in the pass"
    );

    let rich_edge = edge_of(&report, "rich").expect("the rich forecast reached its signal");
    let thin_edge = edge_of(&report, "thin").expect("the thin forecast reached its signal");
    assert!(
        (rich_edge - 6.0 * 100.0 * pressure).abs() < 1e-9,
        "the forecast is not the calibrated model's output: {rich_edge}"
    );
    assert!(
        (thin_edge - 0.5 * 100.0 * pressure).abs() < 1e-9,
        "{thin_edge}"
    );

    let refused = net_edge_refusals(&report);
    assert_eq!(
        refused.len(),
        1,
        "only the thin forecast is refused: {report:?}"
    );
    assert!(refused[0].contains("thin"), "{}", refused[0]);
    let rich_orders: Vec<_> = report
        .orders
        .iter()
        .filter(|order| {
            order
                .contributors
                .iter()
                .any(|c| c.strategy.as_str() == "rich")
        })
        .collect();
    assert_eq!(
        rich_orders.len(),
        1,
        "the forecast above cost did not trade: {report:?}"
    );
    // Premise: the cell really was deciding alone. A policy past its life
    // narrows sizing, so the order of eight went out as three.
    assert_eq!(
        rich_orders[0].quantity,
        d("3"),
        "the cell did not treat the policy as stale, so this proved nothing about the centre \
         being gone: {report:?}"
    );
    Ok(())
}
