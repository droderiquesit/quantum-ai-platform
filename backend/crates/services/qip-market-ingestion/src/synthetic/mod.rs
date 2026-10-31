//! The synthetic market environment.
//!
//! Assembles the price processes, order books, trades and narrative into one
//! adapter that can be driven forward a step at a time. Scripted events let a
//! test or a demo inject a specific shock at a specific instant, which is how
//! the end-to-end reference scenario is made reproducible rather than a matter
//! of waiting for the random walk to do something interesting.

pub mod book;
pub mod narrative;
pub mod price;

use qip_core::error::Result;
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::{Decimal, Duration, ObjectId, Timestamp};
use qip_events::Topic;
use qip_financial::quality::LicensingClass;
use qip_market::bar::{Bar, Interval};
use qip_market::quote::Tick;
use serde::{Deserialize, Serialize};

use crate::adapter::{DataAdapter, MarketDataAdapter, SensedRecord, SourceDescriptor};
use book::BookParameters;
use narrative::SyntheticCompany;
use price::{FactorModel, PriceState, ProcessParameters, Regime};

/// The terms of a dated, cash-settled contract the exchange lists on one of
/// its own names.
///
/// Why this exists: the kernel's rule pack carries a contract rule
/// (`r70-contract-expires-before-horizon`, R12-REASON-3057) whose input is a
/// derivative's expiry, and until this type no shipped default path listed a
/// dated instrument — the demo world was four stocks and a bond, so the rule
/// was on the cycle and its input never was. A contract priced by the exchange
/// itself is the honest way to put one there: its price is the underlying's,
/// carried to expiry, rather than a sixth random walk.
#[derive(Clone, Debug)]
pub struct ContractTerms {
    /// The symbol of the instrument the contract settles against. It must be
    /// listed in the same environment, and ahead of the contract, so the
    /// contract reads the spot the same step produced; see
    /// [`SyntheticEnvironment::demo`].
    pub underlying_symbol: String,
    /// The instant the contract stops trading and settles in cash.
    pub expiry: Timestamp,
    /// Units of the underlying one contract represents. A `Decimal` because
    /// it multiplies a price into money downstream, and the crossing from
    /// `f64` would otherwise happen in every reader.
    pub multiplier: Decimal,
    /// Annualised cost of carry the fair value is marked at.
    pub carry_rate: f64,
}

/// One instrument in the synthetic world.
#[derive(Clone, Debug)]
pub struct SyntheticInstrument {
    pub object_id: ObjectId,
    pub symbol: String,
    pub venue: String,
    /// Company this instrument is a claim on, where there is one.
    pub entity_id: Option<String>,
    pub process: ProcessParameters,
    pub book: BookParameters,
    pub state: PriceState,
    /// The contract terms, where this instrument is a dated contract rather
    /// than a name with a price process of its own. `None` for every spot
    /// instrument.
    pub contract: Option<ContractTerms>,
}

impl SyntheticInstrument {
    pub fn new(
        object_id: ObjectId,
        symbol: impl Into<String>,
        venue: impl Into<String>,
        initial_price: f64,
        process: ProcessParameters,
        book: BookParameters,
    ) -> Self {
        let variance = process.long_run_variance;
        Self {
            object_id,
            symbol: symbol.into(),
            venue: venue.into(),
            entity_id: None,
            process,
            book,
            state: PriceState::new(initial_price, variance),
            contract: None,
        }
    }

    pub fn for_entity(mut self, entity_id: impl Into<String>) -> Self {
        self.entity_id = Some(entity_id.into());
        self
    }

    /// A cash-settled future on `underlying`, listed at `listed_at`.
    ///
    /// The contract has no price process of its own: its opening price is the
    /// underlying's spot carried to `terms.expiry` at `terms.carry_rate`, and
    /// every step re-marks it the same way from the spot that step produced
    /// (see [`SyntheticEnvironment::step`]). The underlying's process is
    /// copied only so the contract's opening variance is the spot's rather
    /// than zero, which is what the book generator widens its quote on.
    pub fn cash_settled_future_on(
        underlying: &SyntheticInstrument,
        object_id: ObjectId,
        symbol: impl Into<String>,
        venue: impl Into<String>,
        terms: ContractTerms,
        listed_at: Timestamp,
        book: BookParameters,
    ) -> Self {
        let price = fair_value(underlying.state.price, &terms, listed_at);
        let mut contract = Self::new(
            object_id,
            symbol,
            venue,
            price,
            underlying.process.clone(),
            book,
        );
        contract.contract = Some(terms);
        contract
    }
}

/// The spot carried to expiry: `spot * exp(carry * years_to_expiry)`. At and
/// after expiry the contract is marked at spot, because that is what it
/// settles to and the environment has no delisting; whether a position may be
/// held into that is the kernel's contract rule's business, not the exchange's.
fn fair_value(spot: f64, terms: &ContractTerms, at: Timestamp) -> f64 {
    let remaining = if terms.expiry > at {
        terms.expiry.since(at).as_years_f64()
    } else {
        0.0
    };
    spot * (terms.carry_rate * remaining).exp()
}

/// The contract month codes futures symbols carry, January first.
const MONTH_CODES: [char; 12] = ['F', 'G', 'H', 'J', 'K', 'M', 'N', 'Q', 'U', 'V', 'X', 'Z'];

/// Fewest days a front contract listed by [`SyntheticEnvironment::demo`] has
/// left to run, so a hypothesis horizon of days resolves inside it and the
/// kernel's margin reads positive until a claim really does stretch past the
/// expiry.
const FRONT_CONTRACT_MINIMUM_DAYS: i64 = 60;

/// The expiry of the front contract listed at `listed_at`: the session open
/// (14:30 UTC) of the first third Friday at least
/// [`FRONT_CONTRACT_MINIMUM_DAYS`] after listing.
///
/// Derived from the listing instant rather than fixed, because the demo is
/// started at whatever the deployment's clock says and a fixed date would be
/// already expired on some start and a decade out on another — either makes
/// the margin the contract rule reads a number nobody chose. Third Fridays
/// are at most thirty-five days apart, so the third Friday of the third month
/// after the listing month is never less than seventy days out; the search
/// over the nearer months falls through to that one rather than to a guess.
pub fn front_contract_expiry(listed_at: Timestamp) -> Timestamp {
    let floor = listed_at.saturating_add(Duration::from_days(FRONT_CONTRACT_MINIMUM_DAYS));
    let (year, month, _) = listed_at.civil_date();
    let third_friday = |months_ahead: u32| -> Timestamp {
        let index = (month - 1 + months_ahead) as i32;
        let (y, m) = (
            year + index.div_euclid(12),
            (index.rem_euclid(12) + 1) as u32,
        );
        let first = Timestamp::from_civil(y, m, 1);
        // Monday is weekday 0, so Friday is 4.
        let first_friday = 1 + (4 + 7 - first.weekday()) % 7;
        Timestamp::from_civil(y, m, first_friday + 14)
            .saturating_add(Duration::from_mins(14 * 60 + 30))
    };
    (0..3)
        .map(third_friday)
        .find(|candidate| *candidate >= floor)
        .unwrap_or_else(|| third_friday(3))
}

/// The symbol a contract on `underlying` expiring at `expiry` trades under:
/// the underlying's symbol, the contract month code and the year's last
/// digit, as listed futures are quoted.
pub fn contract_symbol(underlying: &str, expiry: Timestamp) -> String {
    let (year, month, _) = expiry.civil_date();
    let code = MONTH_CODES
        .get((month as usize).saturating_sub(1))
        .copied()
        .unwrap_or('Z');
    format!("{underlying}{code}{}", year.rem_euclid(10))
}

/// A shock injected at a chosen instant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScriptedEventKind {
    /// Move a price by a log return, immediately.
    PriceShock { symbol: String, log_return: f64 },
    /// Multiply an instrument's volatility, decaying back over following steps.
    VolatilitySpike { symbol: String, multiplier: f64 },
    /// Thin an instrument's book and widen its spread.
    LiquidityWithdrawal { symbol: String, multiplier: f64 },
    /// Force the market regime.
    RegimeShift { regime: Regime },
    /// Change how tightly instruments co-move. Below one is a breakdown.
    CorrelationChange { coupling: f64 },
    /// Publish an earnings result with a given surprise, and move the price.
    EarningsSurprise {
        entity_id: String,
        metric: String,
        surprise: f64,
    },
    /// Publish a supply-chain disruption story.
    SupplyDisruption { entity_id: String, severity: f64 },
    /// Publish a macro release that surprised.
    MacroSurprise {
        series_id: String,
        region: String,
        level: f64,
        surprise: f64,
    },
}

/// A scripted event and when it fires.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScriptedEvent {
    pub at: Timestamp,
    pub kind: ScriptedEventKind,
}

impl ScriptedEvent {
    pub fn new(at: Timestamp, kind: ScriptedEventKind) -> Self {
        Self { at, kind }
    }
}

/// Folded into the seed for the contract stream (`SyntheticEnvironment::contract_rng`),
/// so the two streams never coincide for any seed.
const CONTRACT_STREAM_SALT: u64 = 0x434F_4E54_5241_4354; // "CONTRACT"

/// Configuration for the synthetic environment.
#[derive(Clone, Debug)]
pub struct EnvironmentConfig {
    /// How far the world advances per step.
    pub step: Duration,
    /// Publish a full order book every `book_every` steps; quotes every step.
    pub book_every: u32,
    /// Emit a bar every `bar_every` steps.
    pub bar_every: u32,
    pub bar_interval: Interval,
    /// Expected routine news stories per instrument per day.
    pub routine_news_per_day: f64,
    /// Whether to publish trade prints. Off for long backtests, where the
    /// volume is enormous and the bars carry the same information.
    pub emit_trades: bool,
    pub seed: u64,
}

impl Default for EnvironmentConfig {
    fn default() -> Self {
        Self {
            step: Duration::from_secs(60),
            book_every: 5,
            bar_every: 1,
            bar_interval: Interval::Minute,
            routine_news_per_day: 0.4,
            emit_trades: true,
            seed: 20_260_822,
        }
    }
}

/// A complete synthetic market, driven forward a step at a time.
#[derive(Debug)]
pub struct SyntheticEnvironment {
    instruments: Vec<SyntheticInstrument>,
    companies: Vec<SyntheticCompany>,
    factors: FactorModel,
    regime: Regime,
    config: EnvironmentConfig,
    rng: Xoshiro256,
    /// The stream dated contracts draw their books and prints from, kept
    /// apart from `rng` so listing a contract rewrites no spot path: every
    /// seeded trajectory the tests and a replayed demo depend on stays
    /// byte-identical whether or not a contract is listed on it. A contract
    /// draws nothing for its price — that is the spot's, carried — so this
    /// stream feeds only its microstructure.
    contract_rng: Xoshiro256,
    cursor: Timestamp,
    step_index: u64,
    scripted: Vec<ScriptedEvent>,
    /// Bar accumulators, one per instrument.
    open_bars: Vec<Option<BarAccumulator>>,
    fired: Vec<ScriptedEvent>,
}

#[derive(Clone, Debug)]
struct BarAccumulator {
    open_time: Timestamp,
    open: f64,
    high: f64,
    low: f64,
    close: f64,
    volume: f64,
    notional: f64,
    trades: u64,
}

impl SyntheticEnvironment {
    pub fn new(
        instruments: Vec<SyntheticInstrument>,
        companies: Vec<SyntheticCompany>,
        start: Timestamp,
        config: EnvironmentConfig,
    ) -> Self {
        let count = instruments.len();
        Self {
            instruments,
            companies,
            factors: FactorModel::standard(),
            regime: Regime::Normal,
            rng: Xoshiro256::seeded(config.seed),
            contract_rng: Xoshiro256::seeded(config.seed ^ CONTRACT_STREAM_SALT),
            cursor: start,
            step_index: 0,
            scripted: Vec::new(),
            open_bars: vec![None; count],
            fired: Vec::new(),
            config,
        }
    }

    /// The six-instrument demo world: two linked technology names, an energy
    /// producer, a bank, a government bond, and a cash-settled future on the
    /// larger technology name.
    ///
    /// The future is the one dated contract on the shipped default path. It is
    /// listed last, after its underlying, so each step marks it from the spot
    /// that step produced; it has no company, so it narrates no news; and it
    /// draws its book from the contract stream, so the five spot instruments'
    /// paths are the ones they were before it was listed. The derivative desk
    /// holds no permissions by default, so the kernel refuses every order on
    /// it — which is the intended posture: the contract is there to be
    /// reasoned about by the contract rule, not to be traded by the demo.
    pub fn demo(start: Timestamp, config: EnvironmentConfig) -> Self {
        let companies = narrative::demo_companies();
        let vantage = SyntheticInstrument::new(
            ObjectId::from_string("OBJ00000000000000000VNTG"),
            "VNTG",
            "XNYS",
            318.75,
            ProcessParameters::equity(1.10),
            BookParameters::deep(),
        )
        .for_entity("ent-vantage");
        let expiry = front_contract_expiry(start);
        let future = SyntheticInstrument::cash_settled_future_on(
            &vantage,
            ObjectId::from_string("OBJ0000000000000000VNTGF"),
            contract_symbol(&vantage.symbol, expiry),
            "XCME",
            ContractTerms {
                underlying_symbol: vantage.symbol.clone(),
                expiry,
                // One contract is fifty units of the underlying, the size an
                // equity-index contract carries; a multiplier of one would
                // make the contract rule's subject indistinguishable from
                // the stock in every reader but the expiry.
                multiplier: Decimal::from_int(50),
                carry_rate: 0.04,
            },
            start,
            BookParameters::deep(),
        );
        let instruments = vec![
            SyntheticInstrument::new(
                ObjectId::from_string("OBJ00000000000000000NWSC"),
                "NWSC",
                "XNYS",
                142.50,
                ProcessParameters::equity(1.35),
                BookParameters::default(),
            )
            .for_entity("ent-northwind"),
            vantage,
            SyntheticInstrument::new(
                ObjectId::from_string("OBJ00000000000000000MRDN"),
                "MRDN",
                "XNYS",
                88.20,
                ProcessParameters {
                    factor_loadings: vec![0.8, 0.0, 0.7],
                    ..ProcessParameters::equity(0.8)
                },
                BookParameters::default(),
            )
            .for_entity("ent-meridian"),
            SyntheticInstrument::new(
                ObjectId::from_string("OBJ00000000000000000ATFB"),
                "ATFB",
                "XNYS",
                54.10,
                ProcessParameters {
                    factor_loadings: vec![1.1, 0.6, 0.0],
                    ..ProcessParameters::equity(1.05)
                },
                BookParameters::default(),
            )
            .for_entity("ent-atlas"),
            SyntheticInstrument::new(
                ObjectId::from_string("OBJ000000000000000UST10Y"),
                "UST10Y",
                "OTC",
                98.40,
                ProcessParameters::government_bond(8.2),
                BookParameters::illiquid(),
            ),
            future,
        ];
        Self::new(instruments, companies, start, config)
    }

    /// Schedule a shock.
    pub fn schedule(&mut self, event: ScriptedEvent) {
        self.scripted.push(event);
        self.scripted.sort_by_key(|e| e.at.as_nanos());
    }

    pub fn instruments(&self) -> &[SyntheticInstrument] {
        &self.instruments
    }

    pub fn companies(&self) -> &[SyntheticCompany] {
        &self.companies
    }

    pub fn regime(&self) -> Regime {
        self.regime
    }

    pub fn cursor(&self) -> Timestamp {
        self.cursor
    }

    /// Scripted events that have fired so far.
    pub fn fired_events(&self) -> &[ScriptedEvent] {
        &self.fired
    }

    /// Current price for a symbol, for assertions and reporting.
    pub fn price_of(&self, symbol: &str) -> Option<f64> {
        self.instruments
            .iter()
            .find(|i| i.symbol == symbol)
            .map(|i| i.state.price)
    }

    /// Advance one step and return everything that happened.
    pub fn step(&mut self) -> Vec<SensedRecord> {
        let mut records = Vec::new();
        let step_years = self.config.step.as_years_f64();
        let previous = self.cursor;
        self.cursor = self.cursor.saturating_add(self.config.step);
        self.step_index += 1;

        // Fire anything scheduled for the interval just crossed.
        records.extend(self.fire_scripted(previous, self.cursor));

        // The regime is re-drawn roughly daily rather than every step.
        let steps_per_day =
            (Duration::from_days(1).as_nanos() / self.config.step.as_nanos().max(1)).max(1) as u64;
        if self.step_index.is_multiple_of(steps_per_day) {
            self.regime = self.regime.step(&mut self.rng);
        }

        let position = session_position_at(self.cursor);
        let volatility_scale = price::intraday_volatility_scale(position);
        self.factors
            .step(step_years, self.regime, volatility_scale, &mut self.rng);

        let intensity = price::intraday_intensity(position);
        let fraction_of_day = self.config.step.as_days_f64();

        for index in 0..self.instruments.len() {
            records.extend(self.step_instrument(
                index,
                step_years,
                volatility_scale,
                intensity,
                fraction_of_day,
            ));
        }

        // Routine news, at a low rate so most of the stream is noise.
        let expected_stories =
            self.config.routine_news_per_day * fraction_of_day * self.companies.len() as f64;
        let story_count = self.rng.poisson(expected_stories.clamp(0.0, 20.0));
        for _ in 0..story_count {
            if self.companies.is_empty() {
                break;
            }
            let which = self.rng.below(self.companies.len() as u64) as usize;
            let company = self.companies[which].clone();
            records.push(SensedRecord::News(Box::new(narrative::routine_story(
                &company,
                self.cursor,
                &mut self.rng,
            ))));
        }

        records
    }

    /// Advance until `until`, collecting everything produced.
    pub fn run_until(&mut self, until: Timestamp) -> Vec<SensedRecord> {
        let mut records = Vec::new();
        let mut guard = 0u32;
        while self.cursor < until {
            records.extend(self.step());
            guard += 1;
            if guard > 5_000_000 {
                break;
            }
        }
        records
    }

    fn step_instrument(
        &mut self,
        index: usize,
        step_years: f64,
        volatility_scale: f64,
        intensity: f64,
        fraction_of_day: f64,
    ) -> Vec<SensedRecord> {
        let mut records = Vec::new();
        let at = self.cursor;
        let regime = self.regime;

        // A dated contract is marked from its underlying's state rather than
        // stepped: the spot's variance, momentum and shocks become the
        // contract's, so its book widens when the spot's does, and its price
        // is the spot carried to expiry. Resolved before the mutable borrow
        // below because the underlying is another entry of the same list.
        let marked = self.instruments[index].contract.as_ref().and_then(|terms| {
            self.instruments
                .iter()
                .find(|candidate| candidate.symbol == terms.underlying_symbol)
                .map(|underlying| {
                    let mut state = underlying.state.clone();
                    state.price = fair_value(underlying.state.price, terms, at);
                    state
                })
        });

        // Split the borrow: the process needs the factor model and the RNG.
        let instrument = &mut self.instruments[index];
        match marked {
            Some(state) => {
                let previous = instrument.state.price;
                instrument.state = state;
                instrument.state.last_return = (instrument.state.price / previous).ln();
            }
            // A contract whose underlying is not listed keeps its last mark;
            // the environment has no refusal path inside a step, and the
            // constructor takes the underlying by reference so the demo
            // cannot build one.
            None if instrument.contract.is_some() => {}
            None => {
                price::step_process(
                    &mut instrument.state,
                    &instrument.process,
                    &self.factors,
                    regime,
                    step_years,
                    volatility_scale,
                    &mut self.rng,
                );
            }
        }
        let rng = if instrument.contract.is_some() {
            &mut self.contract_rng
        } else {
            &mut self.rng
        };

        let object_id = instrument.object_id.clone();
        let venue = instrument.venue.clone();
        let price = instrument.state.price;
        let state = instrument.state.clone();
        let book_parameters = instrument.book.clone();

        records.push(SensedRecord::Tick(Tick {
            object_id: object_id.clone(),
            venue: venue.clone(),
            at,
            capture_time: at,
            sequence: None,
            price: Decimal::from_f64(price).unwrap_or(Decimal::ONE),
            volume: Decimal::ZERO,
            quality: Default::default(),
        }));

        let book = book::generate_book(
            object_id.clone(),
            &venue,
            at,
            &state,
            &book_parameters,
            regime,
            self.step_index,
            rng,
        );
        if let Some(quote) = book.to_quote() {
            records.push(SensedRecord::Quote(quote));
        }
        if self.config.book_every > 0
            && self
                .step_index
                .is_multiple_of(u64::from(self.config.book_every))
        {
            records.push(SensedRecord::Book(Box::new(book)));
        }

        let mut step_volume = 0.0;
        let mut step_notional = 0.0;
        let mut step_trades = 0u64;
        if self.config.emit_trades {
            let trades = book::generate_trades(
                &object_id,
                &venue,
                at,
                &state,
                &book_parameters,
                regime,
                intensity,
                fraction_of_day,
                self.step_index * 1000 + index as u64,
                rng,
            );
            for trade in &trades {
                step_volume += trade.size.to_f64();
                step_notional += trade.notional().to_f64();
                step_trades += 1;
            }
            records.extend(trades.into_iter().map(SensedRecord::Trade));
        }

        if let Some(bar) =
            self.accumulate_bar(index, at, price, step_volume, step_notional, step_trades)
        {
            records.push(SensedRecord::Bar(Box::new(bar)));
        }
        records
    }

    /// Fold the step into the open bar, emitting it when the bucket closes.
    fn accumulate_bar(
        &mut self,
        index: usize,
        at: Timestamp,
        price: f64,
        volume: f64,
        notional: f64,
        trades: u64,
    ) -> Option<Bar> {
        let interval = self.config.bar_interval;
        let bucket = interval.duration();
        let bucket_start = at.floor_to(bucket);

        let slot = &mut self.open_bars[index];
        let emit = match slot {
            Some(accumulator) if accumulator.open_time != bucket_start => {
                let finished = accumulator.clone();
                *slot = Some(BarAccumulator {
                    open_time: bucket_start,
                    open: price,
                    high: price,
                    low: price,
                    close: price,
                    volume,
                    notional,
                    trades,
                });
                Some(finished)
            }
            Some(accumulator) => {
                accumulator.high = accumulator.high.max(price);
                accumulator.low = accumulator.low.min(price);
                accumulator.close = price;
                accumulator.volume += volume;
                accumulator.notional += notional;
                accumulator.trades += trades;
                None
            }
            None => {
                *slot = Some(BarAccumulator {
                    open_time: bucket_start,
                    open: price,
                    high: price,
                    low: price,
                    close: price,
                    volume,
                    notional,
                    trades,
                });
                None
            }
        };

        let finished = emit?;
        let instrument = &self.instruments[index];
        Some(Bar {
            object_id: instrument.object_id.clone(),
            venue: instrument.venue.clone(),
            interval,
            open_time: finished.open_time,
            open: Decimal::from_f64(finished.open).unwrap_or(Decimal::ONE),
            high: Decimal::from_f64(finished.high).unwrap_or(Decimal::ONE),
            low: Decimal::from_f64(finished.low).unwrap_or(Decimal::ONE),
            close: Decimal::from_f64(finished.close).unwrap_or(Decimal::ONE),
            volume: Decimal::from_f64(finished.volume).unwrap_or(Decimal::ZERO),
            vwap: if finished.volume > 0.0 {
                Decimal::from_f64(finished.notional / finished.volume)
            } else {
                None
            },
            trade_count: finished.trades,
            quality: Default::default(),
        })
    }

    /// Apply any scripted events falling in `(from, until]`.
    fn fire_scripted(&mut self, from: Timestamp, until: Timestamp) -> Vec<SensedRecord> {
        let due: Vec<ScriptedEvent> = self
            .scripted
            .iter()
            .filter(|e| e.at > from && e.at <= until)
            .cloned()
            .collect();
        let mut records = Vec::new();
        for event in due {
            records.extend(self.apply_scripted(&event.kind, until));
            self.fired.push(event);
        }
        self.scripted.retain(|e| e.at > until);
        records
    }

    fn apply_scripted(&mut self, kind: &ScriptedEventKind, at: Timestamp) -> Vec<SensedRecord> {
        let mut records = Vec::new();
        match kind {
            ScriptedEventKind::PriceShock { symbol, log_return } => {
                if let Some(instrument) = self.instruments.iter_mut().find(|i| &i.symbol == symbol)
                {
                    instrument.state.price = (instrument.state.price * log_return.exp()).max(1e-6);
                    instrument.state.last_return = *log_return;
                    instrument.state.momentum += *log_return;
                }
            }
            ScriptedEventKind::VolatilitySpike { symbol, multiplier } => {
                if let Some(instrument) = self.instruments.iter_mut().find(|i| &i.symbol == symbol)
                {
                    instrument.state.volatility_shock = multiplier.max(0.01);
                }
            }
            ScriptedEventKind::LiquidityWithdrawal { symbol, multiplier } => {
                if let Some(instrument) = self.instruments.iter_mut().find(|i| &i.symbol == symbol)
                {
                    instrument.state.liquidity_shock = multiplier.max(0.01);
                }
            }
            ScriptedEventKind::RegimeShift { regime } => {
                self.regime = *regime;
            }
            ScriptedEventKind::CorrelationChange { coupling } => {
                self.factors.coupling = coupling.max(0.0);
            }
            ScriptedEventKind::EarningsSurprise {
                entity_id,
                metric,
                surprise,
            } => {
                if let Some(company) = self
                    .companies
                    .iter()
                    .find(|c| &c.entity_id == entity_id)
                    .cloned()
                {
                    records.push(SensedRecord::Fundamental(Box::new(narrative::fundamental(
                        &company,
                        metric,
                        *surprise,
                        at,
                        &mut self.rng,
                    ))));
                    records.push(SensedRecord::News(Box::new(narrative::earnings_story(
                        &company,
                        *surprise,
                        at,
                        &mut self.rng,
                    ))));
                    // The price responds, partially and immediately — the rest
                    // is left for the market to discover, which is what gives
                    // the platform something to trade.
                    let response = (surprise * 1.8).clamp(-0.25, 0.25);
                    if let Some(instrument) = self
                        .instruments
                        .iter_mut()
                        .find(|i| i.entity_id.as_deref() == Some(entity_id.as_str()))
                    {
                        instrument.state.price =
                            (instrument.state.price * response.exp()).max(1e-6);
                        instrument.state.last_return = response;
                        instrument.state.momentum += response;
                        instrument.state.volatility_shock = 1.0 + surprise.abs() * 4.0;
                    }
                }
            }
            ScriptedEventKind::SupplyDisruption {
                entity_id,
                severity,
            } => {
                if let Some(company) = self
                    .companies
                    .iter()
                    .find(|c| &c.entity_id == entity_id)
                    .cloned()
                {
                    records.push(SensedRecord::News(Box::new(narrative::supply_chain_story(
                        &company,
                        *severity,
                        at,
                        &mut self.rng,
                    ))));
                    if let Some(instrument) = self
                        .instruments
                        .iter_mut()
                        .find(|i| i.entity_id.as_deref() == Some(entity_id.as_str()))
                    {
                        let response = -(severity * 0.6).clamp(0.0, 0.3);
                        instrument.state.price =
                            (instrument.state.price * response.exp()).max(1e-6);
                        instrument.state.last_return = response;
                        instrument.state.momentum += response;
                        instrument.state.volatility_shock = 1.0 + severity * 5.0;
                        instrument.state.liquidity_shock = 1.0 + severity * 3.0;
                    }
                }
            }
            ScriptedEventKind::MacroSurprise {
                series_id,
                region,
                level,
                surprise,
            } => {
                records.push(SensedRecord::Macro(Box::new(narrative::macro_observation(
                    series_id,
                    region,
                    *level,
                    *surprise,
                    at,
                    &mut self.rng,
                ))));
                records.push(SensedRecord::News(Box::new(narrative::macro_story(
                    series_id, region, *surprise, at,
                ))));
                // A macro surprise moves the rates factor, which propagates to
                // every instrument loading on it.
                if self.factors.shocks.len() > 1 {
                    self.factors.shocks[1] += surprise * 0.01;
                }
            }
        }
        records
    }
}

/// Position within the regular session at `at`, or `None` when closed.
fn session_position_at(at: Timestamp) -> Option<f64> {
    // Weekends are closed for every venue in the demo world.
    if at.weekday() >= 5 {
        return None;
    }
    let (hour, minute, _, _) = at.civil_time();
    price::session_position(f64::from(hour * 60 + minute))
}

impl DataAdapter for SyntheticEnvironment {
    fn descriptor(&self) -> SourceDescriptor {
        SourceDescriptor {
            name: "synthetic-exchange".into(),
            provider: "in-tree synthetic market simulator".into(),
            licensing: LicensingClass::Synthetic,
            topics: vec![
                Topic::MarketTick,
                Topic::MarketQuote,
                Topic::MarketOrderBook,
                Topic::MarketTrade,
                Topic::MarketBar,
                Topic::NewsReceived,
                Topic::FundamentalUpdated,
                Topic::MacroUpdated,
                Topic::AlternativeDataReceived,
            ],
            expected_latency: Duration::ZERO,
            production_requirement: Some(
                "Replace with licensed market data (consolidated tape or direct venue feeds), a \
                 news/filings provider, a fundamentals vendor and a macro data source. See \
                 docs/operations/external-dependencies.md."
                    .into(),
            ),
        }
    }

    fn poll(&mut self, until: Timestamp) -> Result<Vec<SensedRecord>> {
        Ok(self.run_until(until))
    }
}

impl MarketDataAdapter for SyntheticEnvironment {
    fn instruments(&self) -> Vec<ObjectId> {
        self.instruments
            .iter()
            .map(|i| i.object_id.clone())
            .collect()
    }
}
