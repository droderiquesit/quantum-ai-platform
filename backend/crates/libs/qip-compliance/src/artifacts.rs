//! Control 5 — signed artifacts and provenance.
//!
//! Nothing enters the store unless its bytes hash to the digest its
//! [`Provenance`] claims and its signature verifies under the store's key. The
//! two checks catch different failures: the digest catches bytes that changed
//! after signing, the signature catches bytes that were never signed at all.
//! Either alone leaves a hole — a matching digest proves internal consistency
//! of a forgery, and a valid signature over a different digest proves nothing
//! about these bytes.
//!
//! [`ProvenanceChain`] walks an artifact back through the inputs recorded on
//! its provenance until it reaches datasets registered as raw. Where the walk
//! cannot continue it reports the exact digest that is missing and which
//! artifact referenced it, because "provenance incomplete" is not something
//! anybody can act on.
//!
//! The signing limitation is documented on [`crate::signing::SigningKey`] and
//! is real: HMAC proves possession of a shared secret, not identity. Read that
//! before treating a verified artifact as attributable to its named signer.

use crate::signing::SigningKey;
use qip_contracts::governance::Provenance;
use qip_core::error::{Error, Result};
use qip_core::{Timestamp, sha256_hex};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// An artifact the store has accepted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredArtifact {
    pub name: String,
    pub provenance: Provenance,
    pub stored_at: Timestamp,
    pub size_bytes: usize,
}

/// A dataset the platform treats as an origin.
///
/// The terminus of a provenance walk. Registering one is a statement that the
/// platform does not claim to know where it came from — a vendor feed, an
/// exchange drop — so the chain legitimately stops here rather than breaking.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RawDataset {
    pub digest: String,
    pub name: String,
    /// Where it came from, in whatever terms the contract with the source uses.
    pub source: String,
    pub registered_at: Timestamp,
}

/// One artifact on a provenance walk.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChainNode {
    pub digest: String,
    pub name: String,
    pub signer: String,
    /// Steps from the artifact the walk started at.
    pub depth: usize,
    pub inputs: Vec<String>,
}

/// Exactly where a provenance walk could not continue.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChainBreak {
    /// The input digest that is neither a stored artifact nor a raw dataset.
    pub missing: String,
    /// The digest of the artifact that referenced it.
    pub referenced_by: String,
    pub referenced_by_name: String,
}

impl ChainBreak {
    pub fn describe(&self) -> String {
        format!(
            "`{}` ({}) declares input {} which is neither a stored artifact nor a registered \
             raw dataset",
            self.referenced_by_name,
            &self.referenced_by[..16.min(self.referenced_by.len())],
            &self.missing[..16.min(self.missing.len())]
        )
    }
}

/// An artifact's ancestry, back to the raw datasets or to the break.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProvenanceChain {
    root: String,
    nodes: Vec<ChainNode>,
    reached_raw: Vec<RawDataset>,
    breaks: Vec<ChainBreak>,
    /// Digests reached more than once by different paths. Not an error — a
    /// dataset feeding two features and both feeding a model is a diamond, not
    /// a cycle — but recorded so a genuine cycle is visible.
    revisited: Vec<String>,
}

impl ProvenanceChain {
    /// The artifact the walk started from.
    pub fn root(&self) -> &str {
        &self.root
    }

    pub fn nodes(&self) -> &[ChainNode] {
        &self.nodes
    }

    /// The raw datasets the walk reached.
    pub fn raw_datasets(&self) -> &[RawDataset] {
        &self.reached_raw
    }

    pub fn breaks(&self) -> &[ChainBreak] {
        &self.breaks
    }

    pub fn revisited(&self) -> &[String] {
        &self.revisited
    }

    pub fn depth(&self) -> usize {
        self.nodes.iter().map(|n| n.depth).max().unwrap_or(0)
    }

    /// Whether the ancestry is fully accounted for.
    ///
    /// Requires both no breaks *and* at least one raw dataset: an artifact
    /// declaring no inputs at all has an unbroken chain that explains nothing,
    /// and treating that as complete would let a model with no recorded
    /// training data pass as fully traced.
    pub fn is_complete(&self) -> bool {
        self.breaks.is_empty() && !self.reached_raw.is_empty()
    }

    /// The chain, or an error naming exactly where it breaks.
    pub fn require_complete(&self) -> Result<()> {
        if !self.breaks.is_empty() {
            let detail: Vec<String> = self.breaks.iter().map(ChainBreak::describe).collect();
            return Err(Error::not_found(format!(
                "the provenance of {} is broken in {} place(s): {}",
                &self.root[..16.min(self.root.len())],
                self.breaks.len(),
                detail.join("; ")
            )));
        }
        if self.reached_raw.is_empty() {
            return Err(Error::not_found(format!(
                "the provenance of {} reaches no registered raw dataset; it declares no inputs \
                 that lead anywhere, so nothing is known about what produced it",
                &self.root[..16.min(self.root.len())]
            )));
        }
        Ok(())
    }
}

/// Signed, content-addressed artifacts and their ancestry.
///
/// Keyed by digest rather than by name: two artifacts with the same bytes are
/// the same artifact however they were named, and storing the same content
/// twice is idempotent rather than a conflict.
#[derive(Debug)]
pub struct ArtifactStore {
    key: SigningKey,
    artifacts: BTreeMap<String, (StoredArtifact, Vec<u8>)>,
    raw: BTreeMap<String, RawDataset>,
    rejections: Vec<(Timestamp, String, String)>,
}

impl ArtifactStore {
    /// A store that will only accept artifacts signed under `key`.
    pub fn new(key: SigningKey) -> Self {
        Self {
            key,
            artifacts: BTreeMap::new(),
            raw: BTreeMap::new(),
            rejections: Vec::new(),
        }
    }

    pub fn key_id(&self) -> &str {
        self.key.key_id()
    }

    pub fn len(&self) -> usize {
        self.artifacts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.artifacts.is_empty()
    }

    /// Artifacts the store refused, with the reason.
    pub fn rejections(&self) -> &[(Timestamp, String, String)] {
        &self.rejections
    }

    /// The bytes a provenance's signature is taken over.
    ///
    /// Digest, signer, build time and every declared input. A signature that
    /// did not cover the inputs would leave the lineage editable after the
    /// fact, which is the part of the record an investigation depends on.
    pub fn signing_payload(provenance: &Provenance) -> String {
        format!(
            "{}|{}|{}|{}",
            provenance.digest(),
            provenance.signer(),
            provenance.built_at().as_nanos(),
            provenance.inputs().join(",")
        )
    }

    /// Sign content with this store's key, producing a [`Provenance`].
    ///
    /// For producers inside the trust boundary. An artifact signed elsewhere
    /// arrives with its own provenance and goes through [`ArtifactStore::store`]
    /// unchanged — the store's checks do not care which of the two happened.
    pub fn seal(
        &self,
        content: &[u8],
        signer: impl Into<String>,
        built_at: Timestamp,
        inputs: Vec<String>,
    ) -> Result<Provenance> {
        let signer = signer.into();
        let unsigned = Provenance::sign(content, signer.clone(), String::new(), built_at, inputs)?;
        let signature = self.key.sign(&Self::signing_payload(&unsigned));
        Provenance::sign(
            content,
            signer,
            signature,
            built_at,
            unsigned.inputs().to_vec(),
        )
    }

    /// Accept an artifact, or refuse it and say why.
    ///
    /// Storing content that is already present is idempotent: the digest
    /// addresses the bytes, so a re-upload of identical content is the same
    /// artifact. Content that hashes differently is a different artifact and
    /// gets a different key, so nothing is ever overwritten.
    pub fn store(
        &mut self,
        name: impl Into<String>,
        bytes: Vec<u8>,
        provenance: Provenance,
        at: Timestamp,
    ) -> Result<String> {
        let name = name.into();
        match self.check(&name, &bytes, &provenance) {
            Ok(()) => {}
            Err(error) => {
                self.rejections
                    .push((at, name, error.message().to_string()));
                return Err(error);
            }
        }
        let digest = provenance.digest().to_string();
        let size_bytes = bytes.len();
        self.artifacts.entry(digest.clone()).or_insert((
            StoredArtifact {
                name,
                provenance,
                stored_at: at,
                size_bytes,
            },
            bytes,
        ));
        Ok(digest)
    }

    fn check(&self, name: &str, bytes: &[u8], provenance: &Provenance) -> Result<()> {
        if !provenance.matches(bytes) {
            return Err(Error::denied(format!(
                "the bytes of `{name}` hash to {} but its provenance claims {}; the content \
                 changed after it was signed",
                &sha256_hex(bytes)[..16],
                &provenance.digest()[..16.min(provenance.digest().len())]
            )));
        }
        self.key.require(
            &format!("artifact `{name}`"),
            &Self::signing_payload(provenance),
            provenance.signature(),
        )
    }

    /// Register a dataset as an origin, ending provenance walks that reach it.
    pub fn register_raw_dataset(
        &mut self,
        name: impl Into<String>,
        content: &[u8],
        source: impl Into<String>,
        at: Timestamp,
    ) -> Result<String> {
        let source = source.into();
        if source.trim().is_empty() {
            return Err(Error::invalid(
                "a raw dataset must name its source; an origin nobody can name is not an origin",
            ));
        }
        let digest = sha256_hex(content);
        self.raw.insert(
            digest.clone(),
            RawDataset {
                digest: digest.clone(),
                name: name.into(),
                source,
                registered_at: at,
            },
        );
        Ok(digest)
    }

    pub fn raw_dataset(&self, digest: &str) -> Option<&RawDataset> {
        self.raw.get(digest)
    }

    pub fn raw_datasets(&self) -> impl Iterator<Item = &RawDataset> {
        self.raw.values()
    }

    pub fn get(&self, digest: &str) -> Option<&StoredArtifact> {
        self.artifacts.get(digest).map(|(a, _)| a)
    }

    pub fn iter(&self) -> impl Iterator<Item = &StoredArtifact> {
        self.artifacts.values().map(|(a, _)| a)
    }

    /// The bytes, re-checked against the digest they are stored under.
    ///
    /// The re-check costs a hash and catches the case a content-addressed
    /// store is supposed to catch: bytes that no longer match their address.
    pub fn bytes(&self, digest: &str) -> Result<&[u8]> {
        let (artifact, bytes) = self
            .artifacts
            .get(digest)
            .ok_or_else(|| Error::not_found(format!("no artifact stored as {digest}")))?;
        if !artifact.provenance.matches(bytes) {
            return Err(Error::guard(format!(
                "the stored bytes of `{}` no longer hash to {digest}",
                artifact.name
            )));
        }
        Ok(bytes)
    }

    /// Every stored artifact whose bytes no longer match their digest, or
    /// whose signature no longer verifies.
    ///
    /// Nothing should ever appear here while the store is in memory. It exists
    /// for the deployment where the store is rehydrated from disk or object
    /// storage, where "nothing can have changed" stops being true.
    pub fn integrity_failures(&self) -> Vec<String> {
        self.artifacts
            .iter()
            .filter(|(_, (artifact, bytes))| {
                !artifact.provenance.matches(bytes)
                    || !self.key.verifies(
                        &Self::signing_payload(&artifact.provenance),
                        artifact.provenance.signature(),
                    )
            })
            .map(|(digest, _)| digest.clone())
            .collect()
    }

    /// Walk an artifact back through its inputs.
    ///
    /// Breadth-first with a visited set, so a diamond is traversed once and a
    /// cycle terminates instead of hanging. Both are recorded rather than
    /// silently collapsed.
    pub fn provenance_chain(&self, digest: &str) -> Result<ProvenanceChain> {
        if !self.artifacts.contains_key(digest) {
            return Err(Error::not_found(format!(
                "no artifact stored as {digest}, so it has no provenance to walk"
            )));
        }
        let mut nodes = Vec::new();
        let mut reached_raw = Vec::new();
        let mut breaks = Vec::new();
        let mut revisited = Vec::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut queue: VecDeque<(String, usize)> = VecDeque::new();
        queue.push_back((digest.to_string(), 0));
        seen.insert(digest.to_string());

        while let Some((current, depth)) = queue.pop_front() {
            let Some((artifact, _)) = self.artifacts.get(&current) else {
                continue;
            };
            nodes.push(ChainNode {
                digest: current.clone(),
                name: artifact.name.clone(),
                signer: artifact.provenance.signer().to_string(),
                depth,
                inputs: artifact.provenance.inputs().to_vec(),
            });
            for input in artifact.provenance.inputs() {
                if let Some(raw) = self.raw.get(input) {
                    if seen.insert(input.clone()) {
                        reached_raw.push(raw.clone());
                    }
                    continue;
                }
                if self.artifacts.contains_key(input) {
                    if seen.insert(input.clone()) {
                        queue.push_back((input.clone(), depth + 1));
                    } else {
                        revisited.push(input.clone());
                    }
                    continue;
                }
                breaks.push(ChainBreak {
                    missing: input.clone(),
                    referenced_by: current.clone(),
                    referenced_by_name: artifact.name.clone(),
                });
            }
        }

        reached_raw.sort_by(|a, b| a.digest.cmp(&b.digest));
        revisited.sort();
        revisited.dedup();
        Ok(ProvenanceChain {
            root: digest.to_string(),
            nodes,
            reached_raw,
            breaks,
            revisited,
        })
    }
}
