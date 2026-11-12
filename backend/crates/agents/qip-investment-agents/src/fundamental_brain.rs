//! The Company/Fundamental Brain (R12-REASON-3013, ADR 0458): the specialist
//! that reads an instrument's issuer — its reported financial statements,
//! its earnings against consensus, its corporate events, management's own
//! guidance, and its credit and capital structure.
//!
//! Before it, the equity analyst read price bars only and the credit analyst
//! read a spread, and the fundamentals the kernel absorbs into the world
//! model (`Platform::observe` through `WorldModel::absorb_fundamental`)
//! reached no specialist. A company that missed its own guidance, or had
//! most of its debt falling due inside a year, reached the chief as nothing.
//!
//! **Five sub-scopes, each from its own source, each missing when that source
//! is.** Statements and earnings are the issuer's fundamental readings in the
//! world model, tagged `world_stream:fundamental` by the absorber — the only
//! tag that says a figure was reported rather than derived. Corporate events,
//! guidance and capital structure are the desk's company slot. A sub-scope
//! with nothing behind it is named in `missing`, never reported as zero: a
//! company with no guidance on record has not met its guidance.
//!
//! The finding states no direction. The issuer's consensus surprise already
//! reaches the forecast as `fundamental_surprise` (R12-WORLD-3044); a vote
//! here from the same reading would be a correlated second opinion from one
//! origin, which ADR 0147 decision 8 refuses for the capital position for the
//! same reason.

use crate::desk::Desk;
use crate::support::{FindingBuilder, computed, no_data, observed_feature};
use qip_agents::finding::{AgentBrief, AgentFinding, Direction, NumericFact};
use qip_agents::manifest::AgentManifest;
use qip_agents::runtime::{Agent, AgentContext};
use qip_core::error::Result;
use qip_core::time::Duration;
use qip_world_model::features::FeatureValue;
use qip_world_model::relationship::RelationshipKind;
use std::sync::Arc;

/// The tag `WorldModel::absorb_fundamental` puts on every reading it
/// records, and the only evidence a feature value is a reported company
/// figure.
pub const FUNDAMENTAL_SOURCE: &str = "world_stream:fundamental";

/// The window "falling due soon" is measured over.
const MATURITY_WINDOW_DAYS: i64 = 365;

/// Reads an instrument's issuer: statements, earnings, corporate events,
/// guidance and capital structure.
#[derive(Debug)]
pub struct CompanyFundamentalAnalyst {
    manifest: AgentManifest,
    desk: Arc<Desk>,
}

impl CompanyFundamentalAnalyst {
    pub fn new(manifest: AgentManifest, desk: Arc<Desk>) -> Self {
        Self { manifest, desk }
    }
}

impl Agent for CompanyFundamentalAnalyst {
    fn manifest(&self) -> &AgentManifest {
        &self.manifest
    }

    fn accepts(&self, brief: &AgentBrief) -> bool {
        !brief.objects.is_empty()
    }

    fn analyse(&self, ctx: &mut AgentContext, brief: &AgentBrief) -> Result<AgentFinding> {
        let Some(subject) = brief.objects.first() else {
            return Ok(no_data(
                ctx,
                brief.as_of,
                "a company view needs an instrument whose issuer it is about",
            ));
        };
        let as_of = brief.as_of;
        let listed_issuer = {
            let market = self.desk.market.get(ctx)?;
            market
                .universe
                .get(subject)
                .and_then(|object| object.issuer.clone())
        };
        let world = self.desk.world.get(ctx)?;
        // The knowledge graph's `Issues` edge is what the kernel writes when it
        // links an issuer to its instruments; the reference record's issuer
        // field is read only where the graph holds none.
        let issuer = world
            .graph()
            .predecessors(
                subject.as_str(),
                Some(RelationshipKind::Issues),
                as_of,
                as_of,
            )
            .into_iter()
            .map(|fact| fact.relationship.from.clone())
            .min()
            .or(listed_issuer);
        let Some(issuer) = issuer else {
            return Ok(no_data(
                ctx,
                as_of,
                format!(
                    "no issuer is on record for {}: neither the knowledge graph nor the reference \
                     record names one, so there is no company to read",
                    subject.as_str()
                ),
            ));
        };

        // Every reported reading for the issuer readable at the as-of, in
        // both time dimensions, by feature name.
        let features = world.features();
        let mut statements: Vec<(&str, &FeatureValue)> = Vec::new();
        let mut surprises: Vec<(&str, &FeatureValue)> = Vec::new();
        for definition in features.definitions() {
            let name = definition.name.as_str();
            let Some(value) = features.value_as_of(name, &issuer, as_of, as_of) else {
                continue;
            };
            if !value.sources.iter().any(|s| s == FUNDAMENTAL_SOURCE) || !value.value.is_finite() {
                continue;
            }
            if name.ends_with("_surprise") {
                surprises.push((name, value));
            } else {
                statements.push((name, value));
            }
        }

        let company = self.desk.company.get(ctx)?;
        let mut facts: Vec<NumericFact> = Vec::new();
        let mut missing: Vec<String> = Vec::new();
        let mut evidence: Vec<String> = Vec::new();
        let mut clauses: Vec<String> = Vec::new();

        // --- financial statements ---
        if statements.is_empty() {
            missing.push(format!(
                "financial statements: no reported figure for {issuer}"
            ));
        } else {
            facts.push(computed(
                ctx,
                "statement_metrics_reported",
                statements.len() as f64,
                "count",
                &[FUNDAMENTAL_SOURCE],
            ));
            for (name, value) in &statements {
                facts.push(observed_feature(
                    features, name, &issuer, name, "reported", value,
                ));
                evidence.push(format!("feature:{name}@{issuer}"));
            }
            clauses.push(format!("{} reported figure(s)", statements.len()));
        }

        // --- earnings against consensus: the newest surprise ---
        let newest = surprises
            .iter()
            .max_by(|a, b| a.1.available_at.cmp(&b.1.available_at).then(b.0.cmp(a.0)));
        match newest {
            None => missing.push(format!(
                "earnings: no figure for {issuer} was released against a consensus"
            )),
            Some((name, value)) => {
                facts.push(observed_feature(
                    features,
                    name,
                    &issuer,
                    "earnings_surprise",
                    "ratio",
                    value,
                ));
                facts.push(computed(
                    ctx,
                    "earnings_surprises_reported",
                    surprises.len() as f64,
                    "count",
                    &[FUNDAMENTAL_SOURCE],
                ));
                evidence.push(format!("feature:{name}@{issuer}"));
                clauses.push(format!(
                    "{name} {:+.1}% against consensus",
                    value.value * 100.0
                ));
            }
        }

        // --- corporate events: the subject's announced actions ---
        if company.corporate_actions_observed {
            let actions: Vec<_> = company
                .corporate_actions
                .iter()
                .filter(|a| a.object_id == *subject && a.announced_at <= as_of)
                .collect();
            facts.push(computed(
                ctx,
                "corporate_actions_pending",
                actions.len() as f64,
                "count",
                &["corporate-actions"],
            ));
            if let Some(next) = actions
                .iter()
                .filter(|a| a.ex_date >= as_of)
                .map(|a| a.ex_date)
                .min()
            {
                facts.push(computed(
                    ctx,
                    "next_ex_date_days",
                    next.since(as_of).as_secs_f64() / 86_400.0,
                    "days",
                    &["corporate-actions"],
                ));
            }
            clauses.push(format!("{} corporate action(s) pending", actions.len()));
        } else {
            missing.push(
                "corporate events: no corporate-action feed has reached the kernel".to_string(),
            );
        }

        // --- management guidance: the latest issued, judged where reported ---
        let latest_guidance = company
            .guidance
            .iter()
            .filter(|g| g.entity_id() == issuer && g.issued_at() <= as_of)
            .max_by(|a, b| {
                a.issued_at()
                    .cmp(&b.issued_at())
                    .then(b.metric().cmp(a.metric()))
            });
        match latest_guidance {
            None => missing.push(format!("management guidance: none on record for {issuer}")),
            Some(guidance) => {
                let record = format!(
                    "guidance:{issuer}:{}:period={}:issued={}",
                    guidance.metric(),
                    guidance.period_end(),
                    guidance.issued_at()
                );
                facts.push(NumericFact::observed(
                    "guidance_midpoint",
                    guidance.midpoint().to_f64(),
                    "reported",
                    guidance.source(),
                    guidance.issued_at(),
                    record.clone(),
                ));
                evidence.push(record);
                // Judged only against a figure for the period guided: a
                // reading for another quarter is not the outcome of this one.
                let outcome = features
                    .value_as_of(guidance.metric(), &issuer, as_of, as_of)
                    .filter(|v| {
                        v.valid_at == guidance.period_end()
                            && v.sources.iter().any(|s| s == FUNDAMENTAL_SOURCE)
                    });
                match outcome.and_then(|v| guidance.gap(v.value)) {
                    Some(gap) => {
                        facts.push(computed(
                            ctx,
                            "guidance_gap",
                            gap,
                            "ratio",
                            &["guidance_midpoint", guidance.metric()],
                        ));
                        clauses.push(format!(
                            "{} {:+.1}% against management's own guidance",
                            guidance.metric(),
                            gap * 100.0
                        ));
                    }
                    None => {
                        facts.push(computed(
                            ctx,
                            "guidance_outstanding",
                            1.0,
                            "count",
                            &["guidance_midpoint"],
                        ));
                        clauses.push(format!(
                            "{} guidance outstanding, not yet reported",
                            guidance.metric()
                        ));
                    }
                }
            }
        }

        // --- credit and capital structure: the latest published ---
        let structure = company
            .capital_structures
            .iter()
            .filter(|c| c.entity_id() == issuer && c.published_at() <= as_of)
            .max_by_key(|c| (c.as_of(), c.published_at()));
        match structure {
            None => missing.push(format!(
                "capital structure: no balance sheet on record for {issuer}"
            )),
            Some(structure) => {
                let record = format!(
                    "capital-structure:{issuer}:as_of={}:published={}",
                    structure.as_of(),
                    structure.published_at()
                );
                // Statistics, not money: the crossing from `Decimal` is here,
                // and nothing below sizes a position.
                facts.push(computed(
                    ctx,
                    "net_debt",
                    structure.net_debt().to_f64(),
                    "currency",
                    &[record.as_str()],
                ));
                if let Some(leverage) = structure.net_leverage() {
                    facts.push(computed(
                        ctx,
                        "net_leverage",
                        leverage,
                        "multiple",
                        &[record.as_str()],
                    ));
                } else {
                    missing.push(format!(
                        "net leverage: {issuer} states no positive EBITDA, so no multiple exists"
                    ));
                }
                let total = structure.total_debt().to_f64();
                if total > 0.0 {
                    let due = structure
                        .due_between(
                            as_of,
                            as_of.saturating_add(Duration::from_days(MATURITY_WINDOW_DAYS)),
                        )
                        .to_f64();
                    facts.push(computed(
                        ctx,
                        "debt_due_within_year",
                        due / total,
                        "ratio",
                        &[record.as_str()],
                    ));
                    clauses.push(format!(
                        "{:.0}% of debt due within a year",
                        due / total * 100.0
                    ));
                }
                evidence.push(record);
            }
        }

        if facts.is_empty() {
            return Ok(no_data(
                ctx,
                as_of,
                format!(
                    "nothing is on record for {issuer}, the issuer of {}: {}",
                    subject.as_str(),
                    missing.join("; ")
                ),
            ));
        }

        let mut builder = FindingBuilder::new(
            ctx,
            as_of,
            format!(
                "{issuer}, issuer of {}: {}",
                subject.as_str(),
                clauses.join("; ")
            ),
        )
        .direction(Direction::Neutral, 0.0)
        .evidence(evidence)
        .caveats(vec![
            "states no direction: the consensus surprise already reaches the forecast as \
             fundamental_surprise, and a second vote from it would be correlated"
                .to_string(),
            "corporate actions already applied are not kept as records; only those awaiting \
             their ex-date are read"
                .to_string(),
        ]);
        for fact in facts {
            builder = builder.fact(fact);
        }
        if !missing.is_empty() {
            builder = builder.missing(missing);
        }
        builder.build()
    }
}
