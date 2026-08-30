//! Cross-provider instrument identity.
//!
//! No single identifier covers every asset class: ISIN exists for listed
//! securities but not for an OTC swap; FIGI covers more but not private funds.
//! An object therefore carries a bag of identifiers, and entity resolution
//! matches on the strongest one available.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// A recognised identifier namespace.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentifierKind {
    /// International Securities Identification Number (12 chars, checksummed).
    Isin,
    /// North American security identifier (9 chars, checksummed).
    Cusip,
    /// UK/Ireland security identifier (7 chars, checksummed).
    Sedol,
    /// Financial Instrument Global Identifier (12 chars).
    Figi,
    /// Legal Entity Identifier of the issuer (20 chars).
    Lei,
    /// Reuters Instrument Code.
    Ric,
    /// Bloomberg ticker.
    BloombergTicker,
    /// SEC Central Index Key of the issuer.
    Cik,
    /// Exchange-local ticker.
    ExchangeSymbol,
    /// Blockchain contract address.
    ContractAddress,
    /// Provider-specific key, disambiguated by `Identifiers::provider_keys`.
    ProviderKey,
    /// Identifier minted by this platform when nothing external exists.
    Internal,
}

impl IdentifierKind {
    /// Ranking used by entity resolution: a match on a stronger identifier
    /// beats a match on a weaker one.
    ///
    /// Ticker matches are deliberately weak — tickers are reused across venues
    /// and reassigned over time, and treating them as authoritative is how two
    /// unrelated companies end up merged in a knowledge graph.
    pub fn confidence(&self) -> f64 {
        match self {
            Self::Isin | Self::Figi => 1.0,
            Self::Cusip | Self::Sedol => 0.95,
            Self::ContractAddress => 0.95,
            Self::Lei | Self::Cik => 0.9,
            Self::Internal => 0.85,
            Self::Ric | Self::BloombergTicker => 0.6,
            Self::ProviderKey => 0.55,
            Self::ExchangeSymbol => 0.4,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Isin => "isin",
            Self::Cusip => "cusip",
            Self::Sedol => "sedol",
            Self::Figi => "figi",
            Self::Lei => "lei",
            Self::Ric => "ric",
            Self::BloombergTicker => "bloomberg_ticker",
            Self::Cik => "cik",
            Self::ExchangeSymbol => "exchange_symbol",
            Self::ContractAddress => "contract_address",
            Self::ProviderKey => "provider_key",
            Self::Internal => "internal",
        }
    }

    /// Structural validity for the kinds that define a format.
    ///
    /// Checksums are verified where the standard defines one, so a transposed
    /// digit in an upstream feed is caught at ingestion rather than silently
    /// creating a phantom instrument.
    pub fn is_well_formed(&self, value: &str) -> bool {
        match self {
            Self::Isin => is_valid_isin(value),
            Self::Cusip => is_valid_cusip(value),
            Self::Sedol => is_valid_sedol(value),
            Self::Figi => value.len() == 12 && value.bytes().all(|b| b.is_ascii_alphanumeric()),
            Self::Lei => value.len() == 20 && value.bytes().all(|b| b.is_ascii_alphanumeric()),
            Self::Cik => !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()),
            Self::ContractAddress => {
                value.len() >= 10
                    && value
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'x')
            }
            _ => !value.trim().is_empty(),
        }
    }
}

impl fmt::Display for IdentifierKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The identifiers known for one instrument.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identifiers {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    values: BTreeMap<IdentifierKind, String>,
    /// Keys that are only meaningful alongside the provider that issued them.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub provider_keys: BTreeMap<String, String>,
}

impl Identifiers {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, kind: IdentifierKind, value: impl Into<String>) -> Self {
        self.set(kind, value);
        self
    }

    pub fn set(&mut self, kind: IdentifierKind, value: impl Into<String>) {
        let value = value.into().trim().to_string();
        if !value.is_empty() {
            self.values.insert(kind, value);
        }
    }

    pub fn with_provider_key(
        mut self,
        provider: impl Into<String>,
        key: impl Into<String>,
    ) -> Self {
        self.provider_keys.insert(provider.into(), key.into());
        self
    }

    pub fn get(&self, kind: IdentifierKind) -> Option<&str> {
        self.values.get(&kind).map(String::as_str)
    }

    pub fn contains(&self, kind: IdentifierKind) -> bool {
        self.values.contains_key(&kind)
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty() && self.provider_keys.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (IdentifierKind, &str)> + '_ {
        self.values.iter().map(|(k, v)| (*k, v.as_str()))
    }

    /// The strongest identifier present, used as the resolution key.
    pub fn primary(&self) -> Option<(IdentifierKind, &str)> {
        self.values
            .iter()
            .max_by(|a, b| {
                a.0.confidence()
                    .partial_cmp(&b.0.confidence())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(k, v)| (*k, v.as_str()))
    }

    /// Identifiers that fail their format or checksum check.
    pub fn malformed(&self) -> Vec<(IdentifierKind, String)> {
        self.values
            .iter()
            .filter(|(kind, value)| !kind.is_well_formed(value))
            .map(|(kind, value)| (*kind, value.clone()))
            .collect()
    }

    /// Merge another set in, keeping existing values on conflict.
    ///
    /// First-write-wins so a late, lower-quality provider cannot overwrite an
    /// identifier already established by a better source.
    pub fn merge(&mut self, other: &Self) {
        for (kind, value) in &other.values {
            self.values.entry(*kind).or_insert_with(|| value.clone());
        }
        for (provider, key) in &other.provider_keys {
            self.provider_keys
                .entry(provider.clone())
                .or_insert_with(|| key.clone());
        }
    }

    /// Strength of the evidence that two identifier sets denote the same
    /// instrument: the confidence of the strongest agreeing identifier, or 0.0
    /// when any strong identifier actively disagrees.
    pub fn match_strength(&self, other: &Self) -> f64 {
        let mut best = 0.0_f64;
        for (kind, value) in &self.values {
            if let Some(theirs) = other.values.get(kind) {
                if theirs.eq_ignore_ascii_case(value) {
                    best = best.max(kind.confidence());
                } else if kind.confidence() >= 0.9 {
                    // Two different ISINs is positive evidence of *different*
                    // instruments, not merely absence of evidence.
                    return 0.0;
                }
            }
        }
        best
    }
}

/// ISIN: two-letter country code, nine alphanumerics, one Luhn check digit.
pub fn is_valid_isin(value: &str) -> bool {
    if value.len() != 12 {
        return false;
    }
    let bytes = value.as_bytes();
    if !bytes[0].is_ascii_alphabetic() || !bytes[1].is_ascii_alphabetic() {
        return false;
    }
    if !bytes.iter().all(|b| b.is_ascii_alphanumeric()) {
        return false;
    }
    // Expand letters to two-digit numbers, then apply Luhn over the digits.
    let mut digits: Vec<u32> = Vec::with_capacity(24);
    for b in &bytes[..11] {
        if b.is_ascii_digit() {
            digits.push(u32::from(b - b'0'));
        } else {
            let v = u32::from(b.to_ascii_uppercase() - b'A') + 10;
            digits.push(v / 10);
            digits.push(v % 10);
        }
    }
    let check = match bytes[11] {
        b if b.is_ascii_digit() => u32::from(b - b'0'),
        _ => return false,
    };
    luhn_check_digit(&digits) == check
}

/// CUSIP: eight alphanumerics plus a modulus-10 check digit.
pub fn is_valid_cusip(value: &str) -> bool {
    if value.len() != 9 {
        return false;
    }
    let bytes = value.as_bytes();
    if !bytes
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || *b == b'*' || *b == b'@' || *b == b'#')
    {
        return false;
    }
    let mut sum = 0u32;
    for (i, b) in bytes[..8].iter().enumerate() {
        let mut v = match b {
            b if b.is_ascii_digit() => u32::from(b - b'0'),
            b if b.is_ascii_alphabetic() => u32::from(b.to_ascii_uppercase() - b'A') + 10,
            b'*' => 36,
            b'@' => 37,
            b'#' => 38,
            _ => return false,
        };
        if i % 2 == 1 {
            v *= 2;
        }
        sum += v / 10 + v % 10;
    }
    let check = (10 - (sum % 10)) % 10;
    bytes[8].is_ascii_digit() && u32::from(bytes[8] - b'0') == check
}

/// SEDOL: six alphanumerics (no vowels) plus a weighted check digit.
pub fn is_valid_sedol(value: &str) -> bool {
    if value.len() != 7 {
        return false;
    }
    let bytes = value.as_bytes();
    const WEIGHTS: [u32; 6] = [1, 3, 1, 7, 3, 9];
    let mut sum = 0u32;
    for (i, b) in bytes[..6].iter().enumerate() {
        let v = match b {
            b if b.is_ascii_digit() => u32::from(b - b'0'),
            b if b.is_ascii_alphabetic() => {
                let upper = b.to_ascii_uppercase();
                if matches!(upper, b'A' | b'E' | b'I' | b'O' | b'U') {
                    return false; // vowels are excluded by the standard
                }
                u32::from(upper - b'A') + 10
            }
            _ => return false,
        };
        sum += v * WEIGHTS[i];
    }
    let check = (10 - (sum % 10)) % 10;
    bytes[6].is_ascii_digit() && u32::from(bytes[6] - b'0') == check
}

/// Luhn check digit over already-expanded decimal digits.
fn luhn_check_digit(digits: &[u32]) -> u32 {
    let mut sum = 0u32;
    // Doubling starts from the rightmost digit and alternates leftwards.
    for (i, d) in digits.iter().rev().enumerate() {
        let mut v = *d;
        if i % 2 == 0 {
            v *= 2;
            if v > 9 {
                v -= 9;
            }
        }
        sum += v;
    }
    (10 - (sum % 10)) % 10
}
