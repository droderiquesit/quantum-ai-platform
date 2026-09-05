//! `qip-data-finder` — the autonomous data finder.
//!
//! The audit calls this the single largest gap in Layer 1: the mesh knows what
//! datasets exist and where they came from, and nothing decides what *should*
//! exist. This crate runs one lifecycle over candidate sources — discover,
//! classify, probe, assess legality, score, route, register, monitor, detect
//! drift, find a replacement — and produces a [`decision::RegistrationDecision`]
//! per source carrying the reasoning that produced it.
//!
//! Four properties hold structurally rather than by convention, which is to
//! say the compiler or a private field enforces them:
//!
//! * **Unknown is not permitted.** [`legal::Legality`] is three-valued and
//!   combines only through [`legal::Legality::and`], which takes the least
//!   permissive. A missing robots.txt and an unreadable licence both produce
//!   `Unknown`, and `Unknown` never collects.
//! * **Legality is not a score.** [`scoring::Routing`] has private fields and
//!   one constructor, whose first argument is the legality verdict. A source
//!   that scores 1.0 on everything and is forbidden is rejected, and there is
//!   no path through the type that reaches any other class.
//! * **A claim is not evidence.** [`source::SourceCandidate`] and
//!   [`source::Source`] are different types, and the second cannot be built
//!   without [`probe::ProbeEvidence`]. A function that must not run on hearsay
//!   takes the second.
//! * **A decision without a reason cannot exist.**
//!   [`decision::RegistrationDecision::new`] and
//!   [`decision::RegistrationDecision::registered`] both refuse empty
//!   reasoning, and they are the only constructors.
//! * **A source cannot be registered without its legality assessment.**
//!   [`decision::Registration`] has private fields and no public constructor;
//!   the `Registered` outcome is built only by
//!   [`decision::RegistrationDecision::registered`], which takes the
//!   [`legal::LegalAssessment`] and refuses one that did not permit
//!   collection. A catalogue entry therefore cannot exist for a source whose
//!   licence was not read.
//! * **A source's web tier is classified, never assumed, and the dark web
//!   has no fetch path.** [`tier::SourceTier::classify`] refuses on
//!   insufficient evidence rather than defaulting to the surface web;
//!   [`tier::DeepWebAdapter::admissible`] refuses every access mode for the
//!   dark web by name, and refuses the rendered and bulk modes without a
//!   named [`tier::DiscoveryEnclave`].
//!
//! Everything else — rate limits, denylists, drift severity — is advisory in
//! the sense that a caller could compute the same numbers differently. The
//! five above are not.
//!
//! # What this phase does not do
//!
//! It opens no sockets. [`probe::SourceProbe`] is the port;
//! [`probe::InMemoryProbe`] answers from a script and
//! [`probe::NetworkProbe`] reports [`qip_core::Error::Unavailable`] naming
//! what production has to supply. The discovery logic is complete and the
//! transport is not, which is the same shape `qip-storage` and `qip-mesh` use
//! for the services they cannot reach.

pub mod admission;
pub mod coverage;
pub mod decision;
pub mod endpoint;
pub mod finder;
pub mod health;
pub mod ingestion;
pub mod legal;
pub mod probe;
pub mod quality;
pub mod replacement;
pub mod robots;
pub mod schema;
pub mod scoring;
pub mod source;
pub mod tier;

pub use admission::{CatalogueEntry, LicensingDecision, admit, admit_from};
pub use coverage::{CoverageGap, CoverageMatch, SourceCoverage, SourceRegion, UpdateFrequency};
pub use decision::{
    DecisionOutcome, LifecycleStage, ReasonStep, Reasoning, RegisteredSource, RegistrationDecision,
};
pub use endpoint::{
    AccessMechanism, AuthRequirement, Delivery, FeedFormat, FileFormat, McpTarget, PollPlan,
    Scheme, SourceEndpoint,
};
pub use finder::{DataFinder, FinderConfig, MonitorOutcome};
pub use health::{HealthObservation, ObservationOutcome, SourceHealth};
pub use ingestion::{IngestionPlan, descriptor_for, plan_for};
pub use legal::{
    HostRules, LegalAssessment, Legality, LicensingPosture, RateLimit, SourceLicense, SourcePolicy,
};
pub use probe::{
    HeadResponse, InMemoryProbe, NetworkProbe, PayloadSample, ProbeEvidence, RobotsFetch,
    SourceProbe,
};
pub use quality::{SourceCost, SourceQuality};
pub use replacement::{PartialCandidate, RankedReplacement, ReplacementOutcome};
pub use robots::{PathVerdict, RobotsGroup, RobotsPolicy, RobotsRule};
pub use schema::{DriftSeverity, FieldRetype, FieldType, SchemaDrift, SourceSchema};
pub use scoring::{Routing, RoutingClass, SourceScores};
pub use source::{Source, SourceCandidate, SourceIdentity, SourceLineage};
pub use tier::{
    AccessMode, BulkCadence, CredentialReference, DeepWebAdapter, DefensiveMonitoring,
    DiscoveryEnclave, Interface, RenderingBudget, RobotsPosture, SourceTier, TierEvidence,
};
