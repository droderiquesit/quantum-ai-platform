//! The capital-envelope trust root, and refusing to trade on a guessable one.
//!
//! The platform assembles its central plane with a signing secret derived
//! from the configured seed. That is the right default for simulation — a
//! replay of the same configuration must produce the same signatures — and it
//! is documented in the kernel as useless for production: anyone who knows
//! the seed can mint an envelope, and an envelope is permission to put
//! capital at risk. The kernel names the gap; this module is where a binary
//! finally closes it, because for a long time nothing did.
//!
//! `QIP_CAPITAL_ENVELOPE_KEY` is the same variable the edge node reads and
//! verifies grants against, byte for byte. The centre signing with anything
//! else produces grants no cell accepts — so one variable, one trust root,
//! and a rotation is one secret rolled in one place.
//!
//! The refusal is the part that must hold: a platform whose autonomy ceiling
//! permits live trading while its envelope key is still seed-derived must not
//! start. A warning would be read once and scrolled away; a process that is
//! not running is the only message that reliably arrives.

use qip_core::error::{Error, Result};
use qip_kernel::central::CentralPlane;
use qip_kernel::platform::Platform;

/// The variable both ends of the mesh read. One name, one trust root.
pub const ENVELOPE_KEY_VARIABLE: &str = "QIP_CAPITAL_ENVELOPE_KEY";

/// Where this process's envelope-signing key came from. Printed in the
/// banner, because an operator deciding whether a cluster may trade needs
/// this line more than most of the others.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyProvenance {
    /// The deployment supplied a key. Grants from this process are ones only
    /// this deployment could have minted.
    Operator,
    /// Derived from the seed. Reproducible, replayable, and mintable by
    /// anyone who knows the configuration. Paper trading only.
    SeedDerived,
}

impl KeyProvenance {
    pub const fn describe(&self) -> &'static str {
        match self {
            Self::Operator => "operator-supplied via QIP_CAPITAL_ENVELOPE_KEY",
            Self::SeedDerived => {
                "SEED-DERIVED — reproducible, so anyone who knows the seed can mint a grant; \
                 live trading is refused under this key"
            }
        }
    }
}

/// Install the operator key when one is configured, and refuse the
/// combination that must not run.
///
/// Called immediately after `Platform::new` and before anything is served, so
/// the swap happens while the plane is empty — a plane replaced later would
/// discard registered strategies along with the key.
///
/// The plane itself enforces a floor of 32 bytes on an operator key and its
/// refusal names why — shorter is searchable offline — so a key that would be
/// weaker than the seed-derived default it replaces never installs. That
/// check is deliberately not duplicated here: one authority on key strength,
/// and this module would drift from it.
pub fn harden_central(
    platform: &mut Platform,
    configured_key: Option<&str>,
) -> Result<KeyProvenance> {
    let provenance = match configured_key {
        Some(key) if key.trim().is_empty() => {
            return Err(Error::invalid(format!(
                "{ENVELOPE_KEY_VARIABLE} is set and empty. An empty key would \
                 sign every envelope with nothing, which is weaker than the seed-derived \
                 default it replaces; unset it to accept the seed-derived key (paper trading \
                 only), or set the key the cells verify against"
            )));
        }
        Some(key) => {
            let central = CentralPlane::new(key.as_bytes(), platform.config().central.clone())?;
            platform.set_central(central);
            KeyProvenance::Operator
        }
        None => KeyProvenance::SeedDerived,
    };

    if platform.is_live_capable() && platform.central().signing_key_is_reproducible() {
        return Err(Error::denied(format!(
            "this platform's autonomy ceiling permits live trading and its capital-envelope \
             key is derived from the configured seed, which anyone who knows the seed can \
             reproduce — a guessable key on a live-capable platform is a mint for real \
             capital grants. Set {ENVELOPE_KEY_VARIABLE} to the key the cells verify \
             against, or lower the ceiling to paper trading"
        )));
    }

    Ok(provenance)
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;
    use qip_core::error::Result;
    use qip_core::{Context, Timestamp};
    use qip_financial::universe::Universe;
    use qip_kernel::config::PlatformConfig;
    use qip_observability::Telemetry;
    use qip_risk::limits::LimitSet;
    use std::sync::Arc;

    fn platform() -> Result<Platform> {
        let config = PlatformConfig::default();
        let (context, _clock) =
            Context::deterministic(Timestamp::from_secs(1_760_000_000), config.seed);
        let _ = Arc::new(());
        Platform::new(
            config,
            context,
            Telemetry::silent(),
            Universe::new(),
            LimitSet::conservative_default(),
        )
    }

    #[test]
    fn the_default_key_is_reproducible_and_an_operator_key_is_not() -> Result<()> {
        // The premise first: the platform as assembled really does carry the
        // guessable key, or the swap below would be proving nothing.
        let mut platform = platform()?;
        assert!(
            platform.central().signing_key_is_reproducible(),
            "the seed-derived default is no longer reproducible; this module's reason to \
             exist has changed and its docs are now wrong"
        );
        assert_eq!(
            harden_central(&mut platform, None)?,
            KeyProvenance::SeedDerived
        );

        assert_eq!(
            harden_central(&mut platform, Some("a-real-key-from-the-secret-store"))?,
            KeyProvenance::Operator
        );
        assert!(
            !platform.central().signing_key_is_reproducible(),
            "an operator key was installed and the plane still reports a reproducible one"
        );
        Ok(())
    }

    #[test]
    fn a_short_key_is_refused_by_the_plane_and_the_refusal_says_why() -> Result<()> {
        // 19 bytes: long enough to look like a key in a config review, short
        // enough to search. The floor is the plane's, not this module's, and
        // this test pins that the refusal actually propagates — the module's
        // own first smoke run passed only because its test key happened to be
        // exactly 32 bytes, which is luck, not design.
        let mut platform = platform()?;
        let error = harden_central(&mut platform, Some("test-rotation-key-1"))
            .expect_err("a 19-byte envelope key was installed");
        assert!(
            error.message().contains("32"),
            "the refusal does not name the floor: {}",
            error.message()
        );
        assert!(platform.central().signing_key_is_reproducible());
        Ok(())
    }

    #[test]
    fn an_empty_key_is_refused_rather_than_signing_with_nothing() -> Result<()> {
        let mut platform = platform()?;
        let error = harden_central(&mut platform, Some("   "))
            .expect_err("an empty envelope key was accepted");
        assert!(error.message().contains(ENVELOPE_KEY_VARIABLE));
        // And the plane is untouched: a refused hardening must not leave a
        // half-swapped trust root behind it.
        assert!(platform.central().signing_key_is_reproducible());
        Ok(())
    }
}
