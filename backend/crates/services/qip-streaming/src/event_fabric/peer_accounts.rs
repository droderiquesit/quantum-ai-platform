//! Certificate SAN to GCP service account to grant identity (ADR 0112;
//! R12-SEC-8100 clause "broker identities map to GCP service accounts").
//!
//! A bearer token is a standing secret: whoever copies it is the producer.
//! On a mutual-TLS link the certificate's SAN is the workload's identity and
//! the CA pool's issuance IAM decides who may hold such a certificate, so the
//! SAN can stand in for the token. [`PeerAccounts`] is the committed table
//! that says which service account each admitted SAN is, and which grant
//! identity the catalogue's ACLs are written against.
//!
//! Fails closed: with a table configured a peer that is absent or unmapped is
//! refused on every route, and the bearer token is never consulted for it.

use qip_core::error::{Error, Result};
use std::collections::BTreeMap;

/// What one admitted workload is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerAccount {
    /// The GCP service account the certificate was issued to, for audit.
    pub service_account: String,
    /// The name the catalogue's grants are written against.
    pub identity: String,
}

/// SAN to account, built from committed configuration.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PeerAccounts {
    by_san: BTreeMap<String, PeerAccount>,
}

impl PeerAccounts {
    /// `(san, service_account, identity)` rows. Refuses a duplicate SAN, a
    /// service account that is not `name@project.iam.gserviceaccount.com`,
    /// an empty table (it would refuse everyone), and two SANs mapped to one
    /// identity (the log could not say which workload acted).
    pub fn new(rows: impl IntoIterator<Item = (String, String, String)>) -> Result<Self> {
        let mut by_san = BTreeMap::new();
        let mut identities = BTreeMap::new();
        for (san, service_account, identity) in rows {
            let well_formed = service_account.split_once('@').is_some_and(|(name, host)| {
                !name.is_empty()
                    && host.ends_with(".iam.gserviceaccount.com")
                    && host.len() > ".iam.gserviceaccount.com".len()
            });
            if !well_formed {
                return Err(Error::invalid(format!(
                    "`{service_account}` is not a service account email \
                     (name@project.iam.gserviceaccount.com)"
                )));
            }
            if san.trim().is_empty() || identity.trim().is_empty() {
                return Err(Error::invalid("a SAN and an identity must both be named"));
            }
            if identities.insert(identity.clone(), san.clone()).is_some() {
                return Err(Error::invalid(format!(
                    "identity `{identity}` is mapped from two SANs; one identity, one workload"
                )));
            }
            if by_san
                .insert(
                    san.clone(),
                    PeerAccount {
                        service_account,
                        identity,
                    },
                )
                .is_some()
            {
                return Err(Error::invalid(format!("SAN `{san}` is mapped twice")));
            }
        }
        if by_san.is_empty() {
            return Err(Error::invalid(
                "the peer-account table is empty, so every workload would be refused",
            ));
        }
        Ok(Self { by_san })
    }

    /// The account a verified peer SAN is, or the refusal.
    pub fn resolve(&self, peer: Option<&str>) -> Result<&PeerAccount> {
        let san = peer.filter(|name| !name.trim().is_empty()).ok_or_else(|| {
            Error::denied(
                "this listener authorises by certificate and the request carried no verified \
                     peer identity; a bearer token is not accepted here",
            )
        })?;
        self.by_san.get(san).ok_or_else(|| {
            Error::denied(format!(
                "certificate name `{san}` is not mapped to a service account on this broker"
            ))
        })
    }
}
