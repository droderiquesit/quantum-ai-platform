//! Which derivative types a trading entity may trade, in which jurisdiction.
//!
//! The registry can represent a variance swap long before anyone is licensed
//! to deal in one. Without a separate gate, technical ability silently becomes
//! authority to trade (blueprint section 13: derivatives dealing is enabled
//! only through appropriate entities and jurisdictional permissions). So the
//! table starts empty, an empty table refuses every derivative type, and a
//! grant is exact: one entity, one jurisdiction, one type. Research and
//! simulation never ask this table; only the creation of an order does.

use crate::asset_class::InstrumentType;
use crate::constraints::Jurisdiction;
use qip_core::error::{Error, Result};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DerivativePermissions {
    grants: BTreeSet<(String, Jurisdiction, InstrumentType)>,
}

impl DerivativePermissions {
    /// No permissions: every derivative type is refused.
    pub fn none() -> Self {
        Self::default()
    }

    /// Permit `entity` to trade `instrument_type` in `jurisdiction`.
    ///
    /// Refuses a type that is not a derivative, because a grant for an equity
    /// would read as a control that gates something it never sees.
    pub fn grant(
        &mut self,
        entity: &str,
        jurisdiction: Jurisdiction,
        instrument_type: InstrumentType,
    ) -> Result<()> {
        if !instrument_type.is_derivative() {
            return Err(Error::invalid(format!(
                "{instrument_type:?} is not a derivative type; only derivative types are granted here"
            )));
        }
        if entity.trim().is_empty() {
            return Err(Error::invalid(
                "a derivative permission names the trading entity it is granted to",
            ));
        }
        self.grants
            .insert((entity.to_string(), jurisdiction, instrument_type));
        Ok(())
    }

    /// Refuse unless an order for `instrument_type` may be created.
    ///
    /// Types outside the derivative class pass: this gate is not their
    /// authority and must not pretend to be.
    pub fn authorize_order(
        &self,
        entity: &str,
        jurisdiction: Jurisdiction,
        instrument_type: InstrumentType,
    ) -> Result<()> {
        if !instrument_type.is_derivative() {
            return Ok(());
        }
        if self
            .grants
            .contains(&(entity.to_string(), jurisdiction, instrument_type))
        {
            return Ok(());
        }
        Err(Error::denied(format!(
            "{entity} holds no permission to trade {instrument_type:?} in {}; it remains available \
             to research and simulation, and a grant in DerivativePermissions admits the order",
            jurisdiction.as_str()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_derivative_is_refused_with_no_permissions_admitted_once_granted_and_never_to_another_entity()
     {
        let mut permissions = DerivativePermissions::none();
        let ty = InstrumentType::VarianceSwap;
        // Premise: the type really is a derivative, or the refusal proves nothing.
        assert!(ty.is_derivative());
        let us = Jurisdiction::UnitedStates;
        assert!(permissions.authorize_order("desk-a", us, ty).is_err());
        permissions.grant("desk-a", us, ty).unwrap();
        assert!(permissions.authorize_order("desk-a", us, ty).is_ok());
        assert!(permissions.authorize_order("desk-b", us, ty).is_err());
        assert!(
            permissions
                .authorize_order("desk-a", Jurisdiction::Japan, ty)
                .is_err()
        );
        assert!(
            permissions
                .authorize_order("desk-a", us, InstrumentType::Option)
                .is_err()
        );
    }

    #[test]
    fn an_equity_needs_no_derivative_permission_and_cannot_be_granted_one() {
        let mut permissions = DerivativePermissions::none();
        let us = Jurisdiction::UnitedStates;
        assert!(
            permissions
                .authorize_order("desk-a", us, InstrumentType::CommonStock)
                .is_ok()
        );
        assert!(
            permissions
                .grant("desk-a", us, InstrumentType::CommonStock)
                .is_err()
        );
    }
}
