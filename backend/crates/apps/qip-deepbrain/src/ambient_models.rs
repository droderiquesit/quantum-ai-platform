//! The always-on models of the ambient mesh (R12-AMBIENT-1180, ADR 0478).
//!
//! The mesh's other producers are event-triggered: a NOW surprise, a forecast
//! the lattice resolved outside its interval, a precursor run, a limit being
//! approached. Each waits for something else to have happened first. The
//! blueprint's scope (v11.6 p8 §3) asks for models that *predict*, *simulate
//! paths* and *scan weak signals* without being asked, and nothing did: a
//! drift that no single detector called an anomaly, or a tail that only shows
//! up once a path is walked forward, reached no specialist until it was an
//! event in its own right.
//!
//! Three models run here on every research sweep, each raising its signals
//! under a [`Trigger::Schedule`] that names it, so an attention event says
//! which model woke it and that nothing asked:
//!
//! * [`FORECAST_MODEL`] — an AR(1) one-step forecast of each subject's log
//!   return, fitted on the window *before* the newest return and scored
//!   against it. A miss of [`AmbientModelConfig::forecast_miss_sigmas`] or
//!   more residual deviations is a surprise. The next step's forecast is
//!   reported beside it, so the reader sees what the model now expects.
//! * [`PATH_MODEL`] — a seeded bootstrap of each subject's own returns walked
//!   [`AmbientModelConfig::path_steps`] steps forward over
//!   [`AmbientModelConfig::path_count`] paths. The share of paths whose
//!   drawdown from today reaches [`AmbientModelConfig::path_drawdown`] is a
//!   risk review once it reaches [`AmbientModelConfig::path_alarm_bp`].
//! * [`WEAK_SIGNAL_MODEL`] — the newest return of every subject, in deviations
//!   of its own window, read *below* the anomaly bound. One subject a sigma
//!   and a half out is noise; most of the book that far out on the same side
//!   is a weak signal nobody's detector reports, raised as research.
//!
//! Every threshold is deterministic and stated in the config. What the models
//! cannot measure they say so, by subject and reason, in
//! [`AmbientModelReport::unmeasured`]; nothing is clamped into a figure.
//! A non-positive or non-finite price, a history shorter than the window, or
//! a window with no dispersion is unmeasurable, never a zero.
//!
//! Nothing here acts. Every output is an [`AmbientSignal`], and the attention
//! router decides what, if anything, it wakes.

use qip_contracts::ambient::{
    AmbientSignal, Detection, MAX_SEVERITY_BP, Pathway, SignalClass, Trigger,
};
use qip_core::error::{Error, Result};
use qip_core::rng::{Rng, Xoshiro256};
use qip_core::{Decimal, Duration, Timestamp};
use std::collections::BTreeMap;

/// The forecast model's trigger name.
pub const FORECAST_MODEL: &str = "ambient-forecast";
/// The path-simulation model's trigger name.
pub const PATH_MODEL: &str = "ambient-path-simulation";
/// The weak-signal scanner's trigger name.
pub const WEAK_SIGNAL_MODEL: &str = "ambient-weak-signal";
/// Every scheduled model, in the order a sweep runs them.
pub const MODELS: [&str; 3] = [FORECAST_MODEL, PATH_MODEL, WEAK_SIGNAL_MODEL];

/// The thresholds the three models judge by. Validated by
/// [`AmbientModelConfig::validate`]; an invalid one is refused rather than
/// corrected.
#[derive(Clone, Debug, PartialEq)]
pub struct AmbientModelConfig {
    /// Returns each fit reads. At least [`MIN_WINDOW`].
    pub window: usize,
    /// Residual deviations a one-step forecast must miss by to be a surprise.
    pub forecast_miss_sigmas: f64,
    /// Paths the simulation walks per subject. At least one.
    pub path_count: u32,
    /// Steps each path walks. At least one.
    pub path_steps: u32,
    /// How long one price step is, so a simulated path's horizon is stated
    /// in time. Positive.
    pub path_step: Duration,
    /// The drawdown from today's price, as a fraction in (0, 1), a path must
    /// reach to count.
    pub path_drawdown: f64,
    /// The share of paths, in basis points, at which the simulation raises a
    /// risk review. Above zero and at most the scale.
    pub path_alarm_bp: u32,
    /// The simulation's seed. Mixed with the subject's name, so two subjects
    /// draw different paths and the same subject draws the same ones on a
    /// replay.
    pub seed: u64,
    /// The smallest deviation, in sigmas of a subject's own window, the
    /// weak-signal scan counts.
    pub weak_low_sigmas: f64,
    /// The deviation at which a reading is the anomaly path's, not a weak
    /// signal; the scan counts readings below it.
    pub weak_high_sigmas: f64,
    /// Subjects that must lean the same way, and subjects that must be
    /// measurable, before the scan says anything.
    pub weak_min_subjects: usize,
    /// The share of measurable subjects, in basis points, leaning the same
    /// way that makes a weak signal. Above half the scale: a split book is
    /// not a direction.
    pub weak_breadth_bp: u32,
}

/// The fewest returns a fit may read: fewer, and the residual deviation an
/// AR(1) leaves has too few degrees of freedom to be a scale.
pub const MIN_WINDOW: usize = 8;

impl Default for AmbientModelConfig {
    fn default() -> Self {
        Self {
            window: 32,
            forecast_miss_sigmas: 4.0,
            path_count: 256,
            path_steps: 10,
            path_step: Duration::from_days(1),
            path_drawdown: 0.15,
            path_alarm_bp: 2_500,
            seed: 0x00A1_1180,
            weak_low_sigmas: 1.5,
            weak_high_sigmas: 3.0,
            weak_min_subjects: 4,
            weak_breadth_bp: 7_500,
        }
    }
}

impl AmbientModelConfig {
    /// Refuses a window below [`MIN_WINDOW`], a non-positive or non-finite
    /// sigma bound, a weak band that is empty, no paths or steps, a drawdown
    /// outside (0, 1), an alarm or breadth outside its range, and fewer than
    /// two subjects for a breadth.
    pub fn validate(&self) -> Result<()> {
        if self.window < MIN_WINDOW {
            return Err(Error::invalid(format!(
                "an ambient model window of {} returns is below {MIN_WINDOW}; widen it",
                self.window
            )));
        }
        let positive = |name: &str, v: f64| {
            if v.is_finite() && v > 0.0 {
                Ok(())
            } else {
                Err(Error::invalid(format!(
                    "{name} is {v}; state a finite positive number of sigmas"
                )))
            }
        };
        positive("forecast_miss_sigmas", self.forecast_miss_sigmas)?;
        positive("weak_low_sigmas", self.weak_low_sigmas)?;
        positive("weak_high_sigmas", self.weak_high_sigmas)?;
        if self.weak_low_sigmas >= self.weak_high_sigmas {
            return Err(Error::invalid(format!(
                "the weak-signal band [{}, {}) is empty; its floor must sit below its ceiling",
                self.weak_low_sigmas, self.weak_high_sigmas
            )));
        }
        if self.path_count == 0 || self.path_steps == 0 {
            return Err(Error::invalid(
                "a path simulation needs at least one path of at least one step",
            ));
        }
        if self.path_step <= Duration::ZERO {
            return Err(Error::invalid(
                "a path step must be a positive duration; state how long one price step is",
            ));
        }
        if !(self.path_drawdown.is_finite() && self.path_drawdown > 0.0 && self.path_drawdown < 1.0)
        {
            return Err(Error::invalid(format!(
                "a path drawdown of {} is not a fraction in (0, 1)",
                self.path_drawdown
            )));
        }
        if self.path_alarm_bp == 0 || self.path_alarm_bp > MAX_SEVERITY_BP {
            return Err(Error::invalid(format!(
                "a path alarm of {} bp is outside (0, {MAX_SEVERITY_BP}]; zero would raise on every subject",
                self.path_alarm_bp
            )));
        }
        if self.weak_min_subjects < 2 {
            return Err(Error::invalid(
                "a weak-signal breadth needs at least two subjects; one subject is a reading, not a breadth",
            ));
        }
        if self.weak_breadth_bp <= MAX_SEVERITY_BP / 2 || self.weak_breadth_bp > MAX_SEVERITY_BP {
            return Err(Error::invalid(format!(
                "a weak-signal breadth of {} bp is not a majority of the book; state one above 5000",
                self.weak_breadth_bp
            )));
        }
        Ok(())
    }
}

/// What a model could not measure, and why. Reported, never raised.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Unmeasured {
    pub model: &'static str,
    pub subject: String,
    pub reason: &'static str,
}

/// The next step a subject's forecast now expects, as log returns.
#[derive(Clone, Debug, PartialEq)]
pub struct StepForecast {
    pub subject: String,
    pub expected: f64,
    pub lower: f64,
    pub upper: f64,
}

/// One subject's simulated drawdown share.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathReading {
    pub subject: String,
    /// The share of paths reaching the drawdown, in basis points.
    pub breach_bp: u32,
}

/// What one scheduled pass of the three models found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AmbientModelReport {
    /// The signals raised, in model order then subject order.
    pub signals: Vec<AmbientSignal>,
    /// What each subject's forecast now expects for its next step.
    pub forecasts: Vec<StepForecast>,
    /// Every subject's simulated drawdown share.
    pub paths: Vec<PathReading>,
    /// What could not be measured, by model, subject and reason.
    pub unmeasured: Vec<Unmeasured>,
}

/// A subject's window: the fit returns and the newest, held out.
struct Window {
    fit: Vec<f64>,
    newest: f64,
}

/// The window a subject's prices give, or why there is none.
fn window_of(prices: &[f64], window: usize) -> std::result::Result<Window, &'static str> {
    if prices.iter().any(|p| !p.is_finite() || *p <= 0.0) {
        return Err("non_positive_or_non_finite_price");
    }
    // `window` fit returns plus the newest need `window + 2` prices.
    if prices.len() < window + 2 {
        return Err("history_shorter_than_window");
    }
    let tail = &prices[prices.len() - (window + 2)..];
    let returns: Vec<f64> = tail.windows(2).map(|w| (w[1] / w[0]).ln()).collect();
    let (newest, fit) = match returns.split_last() {
        Some((newest, fit)) => (*newest, fit.to_vec()),
        None => return Err("history_shorter_than_window"),
    };
    Ok(Window { fit, newest })
}

fn mean(xs: &[f64]) -> f64 {
    xs.iter().sum::<f64>() / xs.len() as f64
}

fn sample_sd(xs: &[f64]) -> f64 {
    let m = mean(xs);
    (xs.iter().map(|x| (x - m).powi(2)).sum::<f64>() / (xs.len() as f64 - 1.0)).sqrt()
}

/// An AR(1) on `returns`: intercept, slope and residual deviation. `None`
/// when the regressor or the residuals have no dispersion.
fn ar1(returns: &[f64]) -> Option<(f64, f64, f64)> {
    let x = &returns[..returns.len() - 1];
    let y = &returns[1..];
    let (mx, my) = (mean(x), mean(y));
    let sxx: f64 = x.iter().map(|v| (v - mx).powi(2)).sum();
    if sxx <= 0.0 || !sxx.is_finite() {
        return None;
    }
    let sxy: f64 = x.iter().zip(y).map(|(a, b)| (a - mx) * (b - my)).sum();
    let slope = sxy / sxx;
    let intercept = my - slope * mx;
    let sse: f64 = x
        .iter()
        .zip(y)
        .map(|(a, b)| (b - (intercept + slope * a)).powi(2))
        .sum();
    // Two parameters fitted, so two degrees of freedom spent.
    let sd = (sse / (x.len() as f64 - 2.0)).sqrt();
    (sd > 0.0 && sd.is_finite()).then_some((intercept, slope, sd))
}

/// FNV-1a over the subject, mixed into the seed so subjects draw apart.
fn subject_seed(seed: u64, subject: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in subject.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    seed ^ hash
}

fn decimal(value: f64) -> Result<Decimal> {
    Decimal::from_f64(value)
        .ok_or_else(|| Error::numeric("an ambient model figure has no decimal form"))
}

/// Statistic to integer: a deviation in sigmas to severity, a thousand basis
/// points per sigma, the scale's top at ten sigmas or more — the scale's
/// meaning, "as severe as it gets", with the exact size carried as the
/// observed deviation.
fn sigma_severity(sigmas: f64) -> u32 {
    (sigmas.abs() * 1_000.0)
        .round()
        .min(f64::from(MAX_SEVERITY_BP)) as u32
}

struct Raise<'a> {
    model: &'static str,
    class: SignalClass,
    pathway: Pathway,
    subject: String,
    severity: u32,
    observed: f64,
    baseline: f64,
    horizon: Duration,
    entities: Vec<String>,
    evidence: Vec<String>,
    issued: &'a mut u64,
}

fn raise(r: Raise<'_>, now: Timestamp, ttl: Duration) -> Result<AmbientSignal> {
    *r.issued += 1;
    AmbientSignal::new(
        format!("ambient-{}", r.issued),
        r.class,
        r.subject,
        r.severity,
        Trigger::Schedule(r.model.to_string()),
        now,
        Detection {
            observed_deviation: decimal(r.observed)?,
            expected_baseline: decimal(r.baseline)?,
            horizon: r.horizon,
            novelty_bp: r.severity,
            affected_entities: r.entities,
            urgency_bp: r.severity,
            wake_targets: vec![r.pathway],
            evidence_ids: r.evidence,
            expiry: now.saturating_add(ttl),
        },
    )
}

/// Run the three models over `prices` once. `ttl` is how long a raised
/// signal stays live; `issued` numbers the signals, shared with the
/// sweep's other producers so ids stay unique.
pub fn run(
    config: &AmbientModelConfig,
    prices: &BTreeMap<String, Vec<f64>>,
    now: Timestamp,
    ttl: Duration,
    issued: &mut u64,
) -> Result<AmbientModelReport> {
    config.validate()?;
    let mut report = AmbientModelReport::default();
    let mut windows: BTreeMap<&str, Window> = BTreeMap::new();
    for (subject, series) in prices {
        match window_of(series, config.window) {
            Ok(w) => {
                windows.insert(subject.as_str(), w);
            }
            Err(reason) => {
                for model in MODELS {
                    report.unmeasured.push(Unmeasured {
                        model,
                        subject: subject.clone(),
                        reason,
                    });
                }
            }
        }
    }
    let evidence = |subject: &str| {
        vec![
            format!("price_history:{subject}"),
            format!("window:{}", config.window),
        ]
    };

    // --- predict: the one-step forecast, scored on the held-out return ----
    for (subject, w) in &windows {
        let Some((intercept, slope, sd)) = ar1(&w.fit) else {
            report.unmeasured.push(Unmeasured {
                model: FORECAST_MODEL,
                subject: (*subject).to_string(),
                reason: "no_dispersion",
            });
            continue;
        };
        let last_fit = w.fit.last().copied().unwrap_or(0.0);
        let predicted = intercept + slope * last_fit;
        let miss = (w.newest - predicted) / sd;
        let next = intercept + slope * w.newest;
        let band = config.forecast_miss_sigmas * sd;
        report.forecasts.push(StepForecast {
            subject: (*subject).to_string(),
            expected: next,
            lower: next - band,
            upper: next + band,
        });
        if miss.abs() >= config.forecast_miss_sigmas {
            report.signals.push(raise(
                Raise {
                    model: FORECAST_MODEL,
                    class: SignalClass::Surprise,
                    pathway: Pathway::SpecialistActivation,
                    subject: format!("forecast-miss:{subject}"),
                    severity: sigma_severity(miss),
                    observed: miss,
                    baseline: config.forecast_miss_sigmas,
                    horizon: Duration::ZERO,
                    entities: vec![(*subject).to_string()],
                    evidence: evidence(subject),
                    issued,
                },
                now,
                ttl,
            )?);
        }
    }

    // --- simulate paths: a seeded bootstrap of the subject's own returns --
    let floor = (1.0 - config.path_drawdown).ln();
    for (subject, w) in &windows {
        let mut pool = w.fit.clone();
        pool.push(w.newest);
        if sample_sd(&pool) <= 0.0 {
            report.unmeasured.push(Unmeasured {
                model: PATH_MODEL,
                subject: (*subject).to_string(),
                reason: "no_dispersion",
            });
            continue;
        }
        let mut rng = Xoshiro256::seeded(subject_seed(config.seed, subject));
        let mut breached: u32 = 0;
        for _ in 0..config.path_count {
            let mut level = 0.0;
            for _ in 0..config.path_steps {
                let pick = rng.below(pool.len() as u64) as usize;
                level += pool.get(pick).copied().unwrap_or(0.0);
                if level <= floor {
                    breached += 1;
                    break;
                }
            }
        }
        // Integer arithmetic: the share of paths in basis points.
        let breach_bp = (u64::from(breached) * u64::from(MAX_SEVERITY_BP)
            / u64::from(config.path_count)) as u32;
        report.paths.push(PathReading {
            subject: (*subject).to_string(),
            breach_bp,
        });
        if breach_bp >= config.path_alarm_bp {
            let horizon = Duration::from_millis(
                config
                    .path_step
                    .as_millis()
                    .saturating_mul(i64::from(config.path_steps)),
            );
            let mut proof = evidence(subject);
            proof.push(format!(
                "paths:{}x{} seed:{}",
                config.path_count, config.path_steps, config.seed
            ));
            report.signals.push(raise(
                Raise {
                    model: PATH_MODEL,
                    class: SignalClass::Risk,
                    pathway: Pathway::RiskReview,
                    subject: format!("path-drawdown:{subject}"),
                    severity: breach_bp,
                    observed: f64::from(breach_bp),
                    baseline: f64::from(config.path_alarm_bp),
                    horizon,
                    entities: vec![(*subject).to_string()],
                    evidence: proof,
                    issued,
                },
                now,
                ttl,
            )?);
        }
    }

    // --- scan weak signals: the book leaning one way below the anomaly bar
    let mut leaning: BTreeMap<&'static str, Vec<(&str, f64)>> = BTreeMap::new();
    let mut measurable = 0usize;
    for (subject, w) in &windows {
        let sd = sample_sd(&w.fit);
        if sd <= 0.0 || !sd.is_finite() {
            report.unmeasured.push(Unmeasured {
                model: WEAK_SIGNAL_MODEL,
                subject: (*subject).to_string(),
                reason: "no_dispersion",
            });
            continue;
        }
        measurable += 1;
        let z = (w.newest - mean(&w.fit)) / sd;
        let side = if z >= 0.0 { "up" } else { "down" };
        if z.abs() >= config.weak_low_sigmas && z.abs() < config.weak_high_sigmas {
            leaning.entry(side).or_default().push((subject, z));
        }
    }
    if measurable < config.weak_min_subjects {
        report.unmeasured.push(Unmeasured {
            model: WEAK_SIGNAL_MODEL,
            subject: "book".to_string(),
            reason: "fewer_measurable_subjects_than_a_breadth",
        });
    } else {
        for (side, members) in leaning {
            // Integer arithmetic: the leaning share in basis points.
            let breadth =
                (members.len() as u64 * u64::from(MAX_SEVERITY_BP) / measurable as u64) as u32;
            if members.len() < config.weak_min_subjects || breadth < config.weak_breadth_bp {
                continue;
            }
            let mean_z = members.iter().map(|(_, z)| z).sum::<f64>() / members.len() as f64;
            let mut proof: Vec<String> =
                members.iter().map(|(s, z)| format!("{s}:{z:.3}")).collect();
            proof.push(format!("measurable:{measurable}"));
            report.signals.push(raise(
                Raise {
                    model: WEAK_SIGNAL_MODEL,
                    class: SignalClass::Opportunity,
                    pathway: Pathway::ResearchTask,
                    subject: format!("weak-signal:breadth:{side}"),
                    severity: breadth,
                    observed: mean_z,
                    baseline: config.weak_low_sigmas,
                    horizon: Duration::ZERO,
                    entities: members.iter().map(|(s, _)| (*s).to_string()).collect(),
                    evidence: proof,
                    issued,
                },
                now,
                ttl,
            )?);
        }
    }
    report.unmeasured.sort();
    Ok(report)
}
