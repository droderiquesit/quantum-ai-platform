//! Layer 2 of the paper-trading boundary holds on the deserialisation path.
//!
//! `.claude/rules/01-security-and-safety.md` names three independent layers
//! that refuse a live autonomy ceiling: a `terraform plan` validation, the
//! composition roots' `AutonomyLevel::deployable` call, and the type system.
//! The second of those was reachable only if the caller remembered to make
//! it. `PlatformConfig::autonomy_ceiling` is an `AutonomyLevel` with an
//! ordinary derived `Deserialize`, so a stored configuration naming
//! `autonomous_live` decoded straight into a config carrying a live ceiling,
//! which `Platform::new` hands to `AutonomyController::with_live_ceiling`
//! unexamined.
//!
//! It was not hypothetical. `qip-api`, `qip-fastbrain` and `qip-deepbrain`
//! each call `deployable` on their own configured string; `qip-cli`'s
//! `configuration` reads a whole `PlatformConfig` off disk with
//! `serde_json::from_str` and calls it on nothing, so
//! `qip … --config <file>` with a live ceiling in the file assembled a
//! platform at that ceiling. A boundary that holds because three callers out
//! of four remembered it is one caller away from being no boundary.
//!
//! Both halves are asserted here. A gate that refuses everything is not a
//! working gate, and a single-sided test cannot tell the two apart.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts a `Result`-returning function is a bug. In a test the
// assertion is the deliverable, and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_core::error::Result;
use qip_kernel::config::PlatformConfig;
use qip_risk_engine::autonomy::AutonomyLevel;

/// The shipped configuration as JSON, with `autonomy_ceiling` set to `level`.
///
/// Built by serialising a real config and editing the one field, so the rest
/// of the document is whatever the current schema requires. A hand-written
/// fixture would start failing for missing fields and be "fixed" by relaxing
/// the assertion, which is how a boundary test stops testing the boundary.
fn stored_with_ceiling(level: &str) -> Result<serde_json::Value> {
    let mut stored = serde_json::to_value(PlatformConfig::default())
        .map_err(|error| qip_core::error::Error::schema(error.to_string()))?;
    // The premise every case below rests on: the field is there to edit, and
    // it is there under this name. A typo would silently add a field serde
    // ignores, and every refusal below would then be testing nothing.
    assert_eq!(
        stored["autonomy_ceiling"],
        serde_json::Value::String("paper_trading".to_string()),
        "the stored form does not carry the ceiling under `autonomy_ceiling`, so editing it \
         proves nothing"
    );
    stored["autonomy_ceiling"] = serde_json::Value::String(level.to_string());
    Ok(stored)
}

#[test]
fn a_stored_configuration_naming_a_live_ceiling_is_not_a_configuration() -> Result<()> {
    for level in [
        AutonomyLevel::SupervisedLive,
        AutonomyLevel::LimitedAutonomousLive,
        AutonomyLevel::AutonomousLive,
    ] {
        // Premise: this really is one of the levels the platform refuses to
        // start at, so the refusal below is layer 2 and not a decoder that
        // failed to recognise the name.
        assert!(level.is_live());
        assert!(AutonomyLevel::parse(level.as_str()).is_ok());

        let stored = stored_with_ceiling(level.as_str())?;
        let error = serde_json::from_value::<PlatformConfig>(stored).expect_err(&format!(
            "a configuration naming '{}' deserialised into a platform configuration; the ceiling \
             then reaches `AutonomyController::with_live_ceiling` with nothing between",
            level.as_str()
        ));
        let message = error.to_string();

        // Matched on the quoted token `deployable` formats, not on the bare
        // name. `contains("autonomous_live")` is true of
        // `"limited_autonomous_live"`, and that trap has already caught a
        // test in this repository once; the surrounding quote is what makes
        // the three levels distinguishable.
        assert!(
            message.contains(&format!("'{}'", level.as_str())),
            "the refusal does not name the ceiling that caused it: {message}"
        );
        assert!(
            message.contains("paper-trading only"),
            "the refusal does not say why, so an operator cannot tell it from a schema error: \
             {message}"
        );
        assert!(
            message.contains("Set the ceiling to"),
            "the refusal does not name what to do instead: {message}"
        );
    }

    // The delimiter does the work it was chosen for: the message refusing the
    // narrowest live level is not satisfied by the token of the widest.
    let narrow = serde_json::from_value::<PlatformConfig>(stored_with_ceiling(
        AutonomyLevel::LimitedAutonomousLive.as_str(),
    )?)
    .expect_err("a limited live ceiling is refused")
    .to_string();
    assert!(
        !narrow.contains("'autonomous_live'"),
        "the refusal for the limited level carries the unlimited level's token, so the two \
         cannot be told apart: {narrow}"
    );
    Ok(())
}

#[test]
fn a_stored_configuration_naming_a_permitted_ceiling_is_read_as_that_ceiling() -> Result<()> {
    // The half that makes the refusal a gate rather than an outage. A
    // deserialiser that rejected every ceiling would satisfy the test above
    // exactly as well, and would stop every deployment that reads a config
    // file — including the three that are the platform's own shipped
    // postures.
    for level in [
        AutonomyLevel::Observation,
        AutonomyLevel::Advisory,
        AutonomyLevel::PaperTrading,
    ] {
        // Premise: not live, so an admission here is the gate letting a good
        // value through rather than the gate being absent.
        assert!(!level.is_live());

        let config: PlatformConfig = serde_json::from_value(stored_with_ceiling(level.as_str())?)
            .map_err(|error| {
            qip_core::error::Error::schema(format!(
                "a configuration at '{}' was refused: {error}",
                level.as_str()
            ))
        })?;
        // Read back as exactly what was written, not lowered to the default.
        // A ceiling silently corrected is an operator believing something
        // false about their own deployment, which is the failure `deployable`
        // exists to avoid on the refusing side too.
        assert_eq!(
            config.autonomy_ceiling, level,
            "a permitted ceiling was not read back as itself"
        );
    }
    Ok(())
}

#[test]
fn the_shipped_configuration_still_round_trips() -> Result<()> {
    // The regression this guards: a validating deserialiser that refused the
    // platform's own `Default` would break every stored config and every
    // replay, and would do it at the one moment nobody is reading test
    // output.
    let config = PlatformConfig::default();
    assert_eq!(
        config.autonomy_ceiling,
        AutonomyLevel::PaperTrading,
        "the shipped default is no longer paper trading"
    );
    let text = serde_json::to_string(&config)
        .map_err(|error| qip_core::error::Error::schema(error.to_string()))?;
    let read: PlatformConfig = serde_json::from_str(&text)
        .map_err(|error| qip_core::error::Error::schema(error.to_string()))?;
    assert_eq!(
        read, config,
        "the shipped configuration does not round trip"
    );
    Ok(())
}
