//! The JSON shapes of the cognition read surface: `/cognition/self-model`
//! and `/cognition/precedents`.
//!
//! The contract these serialise to is written out in `ROUTES-COGNITION.md`
//! beside the crate manifest, and a page is built against that file rather
//! than against this one. Keep the two exact: a key renamed here and not
//! there is a panel that renders blank with no error anywhere.
//!
//! Two properties are structural rather than asserted:
//!
//! * Nothing here names a learning-engine type. The application layer does
//!   not depend on `qip-learning-engine`, so every row is built by calling
//!   methods on what the kernel hands over — the self-model it holds, the
//!   estimate each component keeps — and the API cannot construct, record
//!   into or re-grade a component of its own.
//! * An accuracy is reported only where the engine reports one. The row's
//!   `accuracy` and `calibrated` are both read off one call to the engine's
//!   own `estimate`, which refuses below its minimum sample, so the body
//!   cannot show a number the engine declined to compute. A `0.5` written
//!   for an unmeasured component would read as measured indifference, which
//!   is the failure the engine exists to prevent.
//!
//! The minimum sample the body states is a copy of the engine's constant,
//! because the API cannot name the original. It is not trusted: [`self_model`]
//! checks every row against it and refuses to serve a body in which the two
//! disagree, so the day the engine's threshold moves this route answers 500
//! naming the drift rather than a body whose `minimum_sample` a page would
//! use to explain a `null` that is really something else.

use qip_kernel::Platform;
use qip_kernel::platform::HypothesisPrecedent;
use serde::Serialize;

/// Outcomes a component needs before the engine reports an accuracy.
///
/// A copy of `qip_learning_engine::self_model::MINIMUM_SAMPLE`, held here
/// because the API does not depend on that crate. Checked, not trusted: see
/// the module comment and [`self_model`].
pub const MINIMUM_SAMPLE: usize = 10;

// --- /cognition/self-model --------------------------------------------------

/// One component the platform has graded, as a page renders it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ComponentView {
    /// The component kind, in `snake_case`: `detector`, `analyst`, `rung`
    /// or `strategy`.
    pub kind: String,
    /// The component's own id within its kind.
    pub key: String,
    /// Graded outcomes in the component's window.
    pub samples: usize,
    /// The engine's estimated accuracy as decimal text, or `null` where
    /// `samples` is below `minimum_sample` and the engine refused to
    /// estimate. A statistic, so `f64` in the engine; rendered as its exact
    /// shortest round-trip text rather than a JSON number so a page shows
    /// what the engine computed and does not re-round it.
    pub accuracy: Option<String>,
    /// Whether an accuracy was reported. Always `accuracy.is_some()`.
    pub calibrated: bool,
}

/// The body of `GET /cognition/self-model`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SelfModelView {
    /// Sorted by `(kind, key)`, both lexicographic.
    pub components: Vec<ComponentView>,
    pub minimum_sample: usize,
}

/// Build `/cognition/self-model` from the platform.
///
/// Read at request time, nothing cached: a thesis that resolved since the
/// last read is in the next body rather than the next restart.
///
/// Refuses, rather than serves, a body in which a row's `calibrated` flag
/// disagrees with `samples < minimum_sample`. Both facts come from the
/// engine — the flag from its refusal to estimate, the count from its
/// window — and the only way they can disagree is the constant above having
/// drifted from the engine's. A page would then explain every `null` with
/// the wrong number.
pub fn self_model(platform: &Platform) -> Result<SelfModelView, String> {
    let mut components = Vec::with_capacity(platform.self_model().len());
    for (key, estimate) in platform.self_model().iter() {
        let samples = estimate.sample_count();
        // One call decides both fields, so they cannot disagree with each
        // other; the check below is against the stated minimum.
        let accuracy = estimate
            .estimate()
            .ok()
            .map(|capability| capability.accuracy.to_string());
        let calibrated = accuracy.is_some();
        if calibrated != (samples >= MINIMUM_SAMPLE) {
            return Err(format!(
                "the engine {} an accuracy for {key} at {samples} sample(s), but this route \
                 states a minimum of {MINIMUM_SAMPLE}; the copied constant has drifted from \
                 the engine's and must be brought back before the body is served",
                if calibrated { "reported" } else { "withheld" }
            ));
        }
        components.push(ComponentView {
            kind: key.kind.as_str().to_string(),
            key: key.id.clone(),
            samples,
            accuracy,
            calibrated,
        });
    }
    // The engine's map orders by its kind enum, which a page cannot know;
    // the contract promises lexicographic `(kind, key)`, so sort to that.
    components.sort_by(|a, b| (&a.kind, &a.key).cmp(&(&b.kind, &b.key)));
    Ok(SelfModelView {
        components,
        minimum_sample: MINIMUM_SAMPLE,
    })
}

// --- /cognition/precedents --------------------------------------------------

/// The body of `GET /cognition/precedents`: the kernel's own precedent
/// records, in the order it holds them (oldest first, most recent last),
/// serialised as the kernel serialises them.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PrecedentsView<'a> {
    pub precedents: &'a [HypothesisPrecedent],
}

/// Build `/cognition/precedents` from the platform.
///
/// A borrow rather than a copy: the kernel's slice is bounded already, and
/// the route adds nothing to a record it did not make.
pub fn precedents(platform: &Platform) -> PrecedentsView<'_> {
    PrecedentsView {
        precedents: platform.precedents(),
    }
}
