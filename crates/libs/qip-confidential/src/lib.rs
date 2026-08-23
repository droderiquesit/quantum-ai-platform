//! `qip-confidential` — sharing what the cells know without sharing what the
//! cells hold.
//!
//! Seven regional cells each see their own order flow, positions and fills.
//! The aggregate is worth having: how crowded a name is across the platform,
//! what the mean exposure looks like, how many cells are above a limit. The raw
//! rows are exactly what must not move, for regulatory reasons and because a
//! cell's book identifies its clients.
//!
//! This crate is the mechanism for releasing the first without releasing the
//! second. It is **statistical disclosure control**: a cohort threshold, a
//! privacy budget, calibrated noise, and a gate against differencing attacks.
//! Each cell reduces its own book to one number; the numbers are aggregated;
//! the aggregate is noised and released, and the ledger remembers what that
//! cost.
//!
//! # This is not confidential computing. Read this part.
//!
//! The architecture calls this layer "Confidential Computing & Data
//! Collaboration". This crate is the second half of that phrase and **none of
//! the first**:
//!
//! * There is **no enclave**. No SGX, no SEV, no TDX, no Confidential VM.
//! * There is **no attestation**. Nothing here can prove to a cell what code is
//!   running on the other side, or that it is the code anyone agreed to.
//! * There is **no hardware isolation** and **no encryption of any kind** —
//!   not at rest, not in use, and not in flight. This build has no TLS at all;
//!   `qip-transport` says the same about itself and means it.
//! * There is **no cryptographic protocol**. No secret sharing, no homomorphic
//!   encryption, no secure multi-party computation. ADR 0009 names hand-rolled
//!   in-tree crypto as the worst of the available options, and a home-made
//!   secret-sharing scheme guarding real client positions would be exactly
//!   that. What is here instead is arithmetic, published and checkable.
//!
//! **The aggregation process sees every raw contribution in its own memory.**
//! That is the shape of the thing: cells send numbers to an aggregator, and the
//! aggregator adds them up. The defence is on what comes *out*, not on what
//! goes in.
//!
//! ## So what does it defend, exactly
//!
//! | Adversary | Defended? |
//! |---|---|
//! | An honest-but-curious cell, or any consumer of releases, trying to infer another cell's contribution from the aggregates it is given | **Yes**, to the degree the budget prices: a cohort threshold on every question and every difference of questions, plus Laplace noise whose total across all releases touching a cell is bounded |
//! | A caller repeating one question to average the noise away | **Yes**, structurally: the noise is a function of the question, so the second ask returns the first answer |
//! | A caller varying a filter, a threshold or epsilon to get fresh draws | **Bounded, not prevented**: every variation spends budget from every cell in it, and the questions stop when the budget does |
//! | Two or more cells colluding | **No.** Four cells in a cohort of five subtract their own numbers and read the fifth's, to within the noise. Any threshold `k` is defeated by `k−1` colluders. The threshold assumes contributors do not compare notes |
//! | A malicious operator with host access to the aggregator | **No.** They read the contributions out of memory. There is no enclave and nothing here would notice |
//! | Anyone on the network path | **No.** Plaintext, unauthenticated transport |
//! | Anyone holding the seed | **No.** The noise is reproducible by construction, so the seed reconstructs the true value of every release exactly. Treat it as key material; this crate has no key management |
//! | A three-query tracker (ask the whole set, then two halves of everything but one cell) | **No.** The differencing gate is pairwise. `tests/differencing.rs` performs this attack and shows it working |
//! | An individual client inside a contributing cell | **Not addressed.** The unit of protection is a cell's contribution. If a cell's number is one client's position, that client is protected exactly as well as the cell is and no better |
//! | Someone learning *which* cells contributed | **No, by design.** The caller names the cohort; membership is an input, not a secret. A cell that declines is visible by its absence |
//!
//! [`NOT_DEFENDED_AGAINST`] carries that second column as data, so a start-up
//! check or an operator console can render the whole list rather than a
//! deployment discovering it one incident at a time.
//!
//! # The mechanisms
//!
//! | Mechanism | Where | What it does, exactly |
//! |---|---|---|
//! | Cohort threshold | [`release::ReleaseGate`] | An aggregate over fewer than [`release::Policy::min_contributors`] cells is **refused**, not returned with a caveat |
//! | Differencing gate | [`release::ReleaseGate`] | Two contributor sets whose symmetric difference is below the threshold cannot both be answered — the threshold applies to differences a caller can compute, not only to questions a caller can ask |
//! | Privacy budget | [`budget::PrivacyLedger`] | Per **cell**, monotone, no reset and no refund; releases stop when a cell is spent |
//! | Calibrated noise | [`noise`] | `Laplace(0, sensitivity/ε)` from a seeded [`qip_core::Xoshiro256`], keyed on the question so a repeat cannot average it away |
//! | Bounded sensitivity | [`query::Bounds`] | Contributions are clamped into a range declared by policy, never derived from the data |
//!
//! # The formal statement, and the four ways it is weaker than it sounds
//!
//! Each release is the Laplace mechanism. For a query whose sensitivity is `Δ`
//! — the most one cell's contribution can move the statistic — a draw from
//! `Laplace(0, Δ/ε)` added to the true value satisfies ε-differential privacy
//! with respect to the neighbour relation in [`query`]: **one cell's value
//! replaced, cohort membership fixed**. Releases compose by basic sequential
//! composition, so what a cell is exposed to across a gate's whole history is
//! bounded by the sum of the epsilons of the releases it appeared in — which is
//! precisely the figure [`budget::PrivacyLedger`] refuses to let past
//! [`budget::Budget`]. A repeated identical question costs nothing because its
//! answer is post-processing of the first one: the same bits, already released.
//!
//! Four qualifications, each of which narrows that statement:
//!
//! 1. **The randomness is a seeded PRG, not randomness.** The draw is a
//!    function of the seed and the question. The guarantee therefore holds
//!    against an adversary who does not hold the seed — it is differential
//!    privacy instantiated with a pseudo-random generator, not the
//!    information-theoretic statement. Against the seed-holder there is no
//!    guarantee at all, and `tests/boundary.rs` recovers a true value that way.
//! 2. **Floating point.** The theorem is about real arithmetic. In `f64` the
//!    low bits of a released value carry information the theorem does not
//!    account for; [`noise::snap`] implements the rounding half of the
//!    published mitigation and no more.
//! 3. **The neighbour relation is the narrow one.** Nothing here protects
//!    *membership*: add-or-remove-a-cell neighbours are outside the model, by
//!    the deliberate choice described in [`query`].
//! 4. **Composition stops at the gate.** Basic composition is the conservative
//!    accounting and that is the safe direction, but it holds within one
//!    [`release::ReleaseGate`]. Across observation periods nothing composes,
//!    so a quantity that barely changes between periods is one secret measured
//!    many times at full price each time.
//!
//! # What a released number means
//!
//! It is the true statistic plus a Laplace draw of scale `b`, which the release
//! reports. Quote [`release::Release::standard_deviation`] with it. For a sum
//! over five cells at ε = 1 with a range of 100, `b` is 100 — the noise is
//! comparable to the total. That is not a defect in the implementation; it is
//! what honest privacy costs at five contributors, and a smaller number would
//! mean a weaker guarantee rather than a better mechanism.
//!
//! # No ambient anything
//!
//! Consistent with `qip-core`'s first rule: the noise is a seeded
//! [`qip_core::Xoshiro256`], there is no clock, and the same seed and the same
//! questions produce byte-identical releases on any machine.
//!
//! ```
//! use qip_confidential::{
//!     Bounds, CellId, CohortId, Contribution, ContributionSet, Epsilon, Policy, Query,
//!     ReleaseGate, Statistic,
//! };
//! use qip_core::error::Result;
//!
//! # fn main() -> Result<()> {
//! let mut contributions = ContributionSet::new();
//! for (cell, exposure) in [
//!     ("emea-1", 12.0),
//!     ("emea-2", 31.5),
//!     ("apac-1", 8.25),
//!     ("apac-2", 44.0),
//!     ("amer-1", 19.75),
//! ] {
//!     contributions.insert(Contribution::new(CellId::new(cell)?, exposure)?)?;
//! }
//!
//! let mut gate = ReleaseGate::new(Policy::default(), 20_260_823);
//! let bounds = Bounds::new(0.0, 100.0)?;
//! let cohort = CohortId::new("global-net-exposure")?;
//!
//! let mean = Query::new(cohort.clone(), Statistic::Mean, bounds, Epsilon::new(0.5)?)?;
//! let released = gate.release(&mean, &contributions)?;
//! assert_eq!(released.contributors(), 5);
//! // Sensitivity of a mean is the range over the contributor count: 100/5 = 20,
//! // so b = 20/0.5 = 40.
//! assert!((released.noise_scale() - 40.0).abs() < 1e-9);
//!
//! // The same question again is the same answer, charged once.
//! assert_eq!(gate.release(&mean, &contributions)?.value(), released.value());
//! assert!((gate.ledger().spent(&CellId::new("emea-1")?).get() - 0.5).abs() < 1e-12);
//!
//! // Four of the five cells is a question the fabric will not answer: it is
//! // below the threshold, and its difference from the release above is one cell.
//! let mut four = ContributionSet::new();
//! for (cell, exposure) in [("emea-1", 12.0), ("emea-2", 31.5), ("apac-1", 8.25), ("apac-2", 44.0)] {
//!     four.insert(Contribution::new(CellId::new(cell)?, exposure)?)?;
//! }
//! assert!(gate.release(&mean, &four).is_err());
//! # Ok(())
//! # }
//! ```

pub mod budget;
pub mod contribution;
pub mod noise;
pub mod query;
pub mod release;

pub use budget::{Budget, Epsilon, PrivacyLedger, SpendReport, SpentEpsilon};
pub use contribution::{CellId, CohortId, Contribution, ContributionSet};
pub use noise::{NoiseScale, Sensitivity, noise_for, snap};
pub use query::{Bounds, Fingerprint, Query, Statistic};
pub use release::{Policy, Release, ReleaseGate, ReleaseId, ReleaseRecord};

/// The second column of the table above, as data.
///
/// Written down here rather than only in prose so that a deployment can print
/// it: an operator console, a start-up banner, or the compliance plane's record
/// of what this control does and does not cover. A limitation that lives only
/// in a doc comment is a limitation the person who needed it did not read.
pub const NOT_DEFENDED_AGAINST: [&str; 8] = [
    "collusion: k-1 cells in a cohort of k subtract their own contributions and recover the last",
    "a malicious operator with host access: the aggregator holds every raw contribution in memory",
    "the network: this build has no TLS and no peer authentication",
    "seed disclosure: the noise is reproducible, so the seed recovers every true value exactly",
    "multi-query trackers: the differencing gate is pairwise and a three-query tracker defeats it",
    "clients inside a cell: the unit of protection is a cell's contribution, not a client's record",
    "cohort membership: which cells contributed is an input, not a secret",
    "repeated observation over time: a slowly-varying quantity re-asked each period is one secret \
     measured many times, and each period gets a fresh budget",
];
