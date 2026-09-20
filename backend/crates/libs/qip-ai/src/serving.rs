//! Serving a trained model in-process, off the hot path (ADR 0083).
//!
//! The platform trains four small forms — two teachers in `qip-training`,
//! two distillates in `qip-strategy` — and until this module nothing outside
//! `qip-training` could score any of them. This is the port: an application
//! names a served model by [`ModelArtifact`] and asks a [`ModelProvider`] for
//! a [`ServedModel`], without naming who serves it or how.
//!
//! What this module deliberately does not do. It does not know the forms'
//! types; the provider does, and the interface carries the serialised form as
//! an opaque [`serde_json::Value`] so that a library below every service can
//! describe an artifact any service fills. It does not reach the hot path:
//! `qip-strategy` gains no dependency on this crate, and a learned function
//! reaches a cell only inline in a compiled plan as `Expr::Model`. And it does
//! not sign anything. [`ModelArtifact::digest`] is in-tree SHA-256 over the
//! canonical payload — integrity, not provenance. Asymmetric signing is ADR
//! 0043's gap and a reader must not take a digest for a signature.
//!
//! The format list is a closed enum without `#[serde(other)]`, so an artifact
//! naming a format outside it is refused by serde at the wire before any
//! provider is asked. That is the price of the two-dependency rule paid on
//! purpose: an open format list is a dependency request waiting for a lane.

use qip_core::error::{Error, Result};
use qip_core::hash::sha256_hex;
use serde::{Deserialize, Serialize};

/// The forms a model artifact may take on this platform. Closed on purpose:
/// an artifact naming a format not listed here is refused at the wire by
/// serde, before any provider is asked, and adding a variant is an ADR.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelFormat {
    /// `qip_training::local::TeacherForm::Linear` with its calibration.
    TeacherLinear,
    /// `qip_training::local::TeacherForm::BoostedStumps` with its calibration.
    TeacherBoostedStumps,
    /// `qip_strategy::model::DistilledModel` in its linear form.
    DistilledLinear,
    /// `qip_strategy::model::DistilledModel` in its tree form.
    DistilledTree,
}

impl ModelFormat {
    /// Every format, in declaration order — the list a refusal quotes.
    pub const ALL: [Self; 4] = [
        Self::TeacherLinear,
        Self::TeacherBoostedStumps,
        Self::DistilledLinear,
        Self::DistilledTree,
    ];

    /// The wire name, identical to the serde form so a log and a payload
    /// agree on what a format is called.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::TeacherLinear => "teacher_linear",
            Self::TeacherBoostedStumps => "teacher_boosted_stumps",
            Self::DistilledLinear => "distilled_linear",
            Self::DistilledTree => "distilled_tree",
        }
    }
}

/// A model as it is stored, published and shipped: the card it belongs to,
/// its format, its serialised form, and the digest of that form.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelArtifact {
    /// `ModelCard::reference()` — name and version.
    pub reference: String,
    pub format: ModelFormat,
    /// The provider's own serialisation of the form (serde_json). The
    /// interface does not know the form's type; the provider does.
    pub payload: serde_json::Value,
    /// SHA-256 of the canonical payload, in-tree hashing (ADR 0002). A
    /// digest, not a signature: it proves the bytes are the bytes, and says
    /// nothing about who produced them (ADR 0043).
    pub digest: String,
}

impl ModelArtifact {
    /// Build an artifact, computing the digest once from the payload it
    /// carries. Providers construct through this so that the digest and the
    /// payload can never be assembled separately and disagree.
    pub fn new(
        reference: impl Into<String>,
        format: ModelFormat,
        payload: serde_json::Value,
    ) -> Self {
        let digest = digest_of(&payload);
        Self {
            reference: reference.into(),
            format,
            payload,
            digest,
        }
    }

    /// Refuse an artifact whose digest is not the digest of its payload,
    /// naming both so an operator can tell a truncated file from a swapped
    /// one. Every provider runs this before deserialising a payload: a
    /// payload that does not match its digest is not the artifact that was
    /// promoted, whatever its reference says.
    pub fn verify_digest(&self) -> Result<()> {
        let recomputed = digest_of(&self.payload);
        if recomputed != self.digest {
            return Err(Error::denied(format!(
                "model artifact `{}` carries digest {} but its payload digests to {}; the \
                 payload is not the one that was promoted — fetch the artifact again or \
                 re-promote the model, and do not serve this one",
                self.reference, self.digest, recomputed
            )));
        }
        Ok(())
    }

    /// Parse an artifact off the wire.
    ///
    /// A format outside [`ModelFormat`] is refused by serde, and this wraps
    /// that refusal in the same sentence a provider would use, so an operator
    /// reading a log sees one refusal rather than two. Any other malformation
    /// is refused as serde describes it.
    pub fn from_json(json: &str) -> Result<Self> {
        let raw: serde_json::Value = serde_json::from_str(json).map_err(|error| {
            Error::invalid(format!("a model artifact could not be parsed: {error}"))
        })?;
        if let Some(format) = raw.get("format").and_then(serde_json::Value::as_str)
            && !ModelFormat::ALL
                .iter()
                .any(|known| known.as_str() == format)
        {
            let reference = raw
                .get("reference")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("<unnamed>");
            return Err(unserved_refusal(
                reference,
                format,
                "this platform",
                &ModelFormat::ALL,
            ));
        }
        serde_json::from_value(raw).map_err(|error| {
            Error::invalid(format!("a model artifact could not be parsed: {error}"))
        })
    }
}

/// The canonical serialisation a digest is taken over.
///
/// `serde_json` here is built without `preserve_order`, so a `Value::Object`
/// is a `BTreeMap` and serialises with its keys sorted; the same payload
/// arriving with its keys in another order therefore digests identically.
/// That property is asserted by a test rather than assumed, because a
/// feature flag on the one JSON crate would silently break it.
pub fn canonical_payload(payload: &serde_json::Value) -> String {
    payload.to_string()
}

/// SHA-256 over [`canonical_payload`], lowercase hex.
pub fn digest_of(payload: &serde_json::Value) -> String {
    sha256_hex(canonical_payload(payload).as_bytes())
}

/// The refusal for a model a provider cannot serve, in the shape ADR 0083
/// fixes: the artifact, its format, who refused, what they do serve, and
/// what to do instead. One function so the wire loader and every provider
/// say the same sentence.
pub fn unserved_refusal(
    reference: &str,
    format: &str,
    provider: &str,
    serves: &[ModelFormat],
) -> Error {
    let served = match serves {
        [] => "nothing".to_string(),
        [only] => format!("`{}`", only.as_str()),
        [head @ .., last] => format!(
            "{} and `{}`",
            head.iter()
                .map(|f| format!("`{}`", f.as_str()))
                .collect::<Vec<_>>()
                .join(", "),
            last.as_str()
        ),
    };
    Error::invalid(format!(
        "model artifact `{reference}` is `{format}`, which {provider} does not serve \
         in-process; it serves {served}. Distil the model with `qip_training::distill` into \
         one of those, or mark its card research-only — a model this platform cannot \
         evaluate is not a model it may act on."
    ))
}

/// A model a provider has loaded and will score. Off the hot path only.
pub trait ServedModel: std::fmt::Debug {
    /// The card reference the artifact named.
    fn reference(&self) -> &str;
    fn format(&self) -> ModelFormat;
    /// Inputs the model reads. `score` refuses any other count.
    fn arity(&self) -> usize;
    /// Worst-case evaluation steps, the unit `DistilledModel::cost` uses.
    fn cost(&self) -> usize;
    /// One score. Refuses — never clamps — an input count other than
    /// `arity()` and any non-finite input; a finite model over finite inputs
    /// returns a finite `f64`, and a provider that cannot promise that is
    /// not conforming. Statistics may be `f64` (core-rust rules); the
    /// crossing into `Decimal` happens in the caller, where money is, and is
    /// commented there.
    fn score(&self, inputs: &[f64]) -> Result<f64>;
}

/// Where a served model comes from.
pub trait ModelProvider: std::fmt::Debug {
    fn name(&self) -> &str;
    /// The formats this provider serves. A caller may check before asking.
    fn serves(&self) -> &[ModelFormat];
    /// Load an artifact, or refuse naming the format and what to do instead.
    /// Refuses: a format outside `serves()`; a digest that does not match
    /// the payload; a payload the form's own constructor refuses (non-finite
    /// weight, empty coefficients, a tree that descends backwards).
    fn serve(&self, artifact: &ModelArtifact) -> Result<Box<dyn ServedModel>>;

    /// The two checks every conforming `serve` runs first, provided here so
    /// a provider cannot forget one: the format is one it serves, and the
    /// payload is the payload the digest names. A provider that served
    /// all four formats would never take the first branch; a provider that
    /// serves fewer would, with the sentence ADR 0083 fixes.
    fn admit(&self, artifact: &ModelArtifact) -> Result<()> {
        if !self.serves().contains(&artifact.format) {
            return Err(unserved_refusal(
                &artifact.reference,
                artifact.format.as_str(),
                self.name(),
                self.serves(),
            ));
        }
        artifact.verify_digest()
    }
}

/// The provider a platform holds when its composition root handed in none.
///
/// It serves nothing and says so with the same sentence every provider
/// uses, naming itself, so a log line from a process assembled without a
/// provider reads "the null provider does not serve `distilled_linear`"
/// rather than a panic or a silent `None`. Fail-closed by construction: a
/// `Platform` built through `Platform::new` promotes and serves no model
/// until a root passes `qip_training::serve::InTreeProvider` through
/// `Platform::with_model_provider`, and every test that does not care about
/// serving gets exactly the platform every root had before ADR 0083.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NoProvider;

impl NoProvider {
    pub const NAME: &'static str = "the null provider";
}

impl ModelProvider for NoProvider {
    fn name(&self) -> &str {
        Self::NAME
    }

    fn serves(&self) -> &[ModelFormat] {
        &[]
    }

    fn serve(&self, artifact: &ModelArtifact) -> Result<Box<dyn ServedModel>> {
        // `admit` refuses every format, since this provider serves none;
        // the `Err` branch is the only way out and the match makes that
        // legible rather than relying on an unreachable `Ok`.
        match self.admit(artifact) {
            Err(refusal) => Err(refusal),
            Ok(()) => Err(unserved_refusal(
                &artifact.reference,
                artifact.format.as_str(),
                Self::NAME,
                &[],
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A provider that serves nothing, which is the only way to reach the
    /// unserved refusal from a provider rather than from the wire, since the
    /// enum is closed.
    #[derive(Debug)]
    struct ServesNothing;

    impl ModelProvider for ServesNothing {
        fn name(&self) -> &str {
            "serves-nothing"
        }
        fn serves(&self) -> &[ModelFormat] {
            &[]
        }
        fn serve(&self, artifact: &ModelArtifact) -> Result<Box<dyn ServedModel>> {
            self.admit(artifact)?;
            Err(Error::invalid("unreachable: this provider admits nothing"))
        }
    }

    fn artifact() -> ModelArtifact {
        ModelArtifact::new(
            "regime@3",
            ModelFormat::DistilledLinear,
            serde_json::json!({"b": 1.0, "a": [2.0, 3.0]}),
        )
    }

    #[test]
    fn a_digest_is_taken_over_the_canonical_form_so_key_order_does_not_change_it() {
        let one = serde_json::json!({"b": 1.0, "a": [2.0, 3.0]});
        let two = serde_json::json!({"a": [2.0, 3.0], "b": 1.0});
        assert_eq!(digest_of(&one), digest_of(&two));
        assert_eq!(
            digest_of(&one).len(),
            64,
            "a SHA-256 hex digest is 64 characters"
        );
        // Premise: a different payload digests differently, or the assertion
        // above would hold of a constant.
        let three = serde_json::json!({"a": [2.0, 3.5], "b": 1.0});
        assert_ne!(digest_of(&one), digest_of(&three));
    }

    #[test]
    fn a_payload_that_does_not_match_its_digest_is_refused_naming_both() {
        let genuine = artifact();
        assert!(genuine.verify_digest().is_ok(), "the premise failed");
        let mut swapped = genuine.clone();
        swapped.payload = serde_json::json!({"b": 1.0, "a": [2.0, 4.0]});
        let error = swapped
            .verify_digest()
            .expect_err("a swapped payload verified");
        assert!(error.message().contains(&genuine.digest), "{error}");
        assert!(
            error.message().contains(&digest_of(&swapped.payload)),
            "{error}"
        );
        assert!(ServesNothing.admit(&swapped).is_err());
    }

    #[test]
    fn an_artifact_naming_a_format_outside_the_enum_is_refused_at_the_wire() {
        let json = r#"{"reference":"regime@3","format":"onnx","payload":{},"digest":"x"}"#;
        let error = ModelArtifact::from_json(json).expect_err("an unknown format parsed");
        let message = error.message();
        assert!(
            message.contains("model artifact `regime@3` is `onnx`"),
            "{message}"
        );
        assert!(message.contains("does not serve in-process"), "{message}");
        assert!(
            message.contains("`teacher_linear`, `teacher_boosted_stumps`, `distilled_linear` and `distilled_tree`"),
            "{message}"
        );
        // Premise: the same body with a known format parses.
        let known = json.replace(r#""onnx""#, r#""distilled_tree""#);
        assert_ne!(known, json);
        assert!(
            ModelArtifact::from_json(&known).is_ok(),
            "the premise failed"
        );
        // And the typed path refuses too, without the wrapper — the closed
        // enum is the guarantee, the wrapper is the sentence.
        assert!(serde_json::from_str::<ModelArtifact>(json).is_err());
    }

    #[test]
    fn a_provider_that_does_not_serve_a_format_refuses_with_the_fixed_sentence() {
        let error = ServesNothing.serve(&artifact()).expect_err("served");
        let message = error.message();
        assert!(
            message.contains(
                "model artifact `regime@3` is `distilled_linear`, which serves-nothing does not \
                 serve in-process; it serves nothing."
            ),
            "{message}"
        );
        assert!(
            message.contains("Distil the model with `qip_training::distill`"),
            "{message}"
        );
    }

    #[test]
    fn the_refusal_lists_the_served_formats_in_the_shape_the_adr_fixes() {
        let error = unserved_refusal("m@1", "onnx", "in-tree", &ModelFormat::ALL);
        assert!(
            error.message().contains(
                "which in-tree does not serve in-process; it serves `teacher_linear`, \
                 `teacher_boosted_stumps`, `distilled_linear` and `distilled_tree`. Distil"
            ),
            "{}",
            error.message()
        );
        let one = unserved_refusal("m@1", "onnx", "p", &[ModelFormat::DistilledTree]);
        assert!(
            one.message().contains("it serves `distilled_tree`."),
            "{}",
            one.message()
        );
    }
}
