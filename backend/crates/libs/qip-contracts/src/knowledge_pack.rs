//! Knowledge packs (DATA-013): the compact, versioned, schema-checked artifact
//! derived knowledge travels and is stored in, never the world content it was
//! derived from.
//!
//! Encoding: canonical `serde_json`, the only serialisation this workspace
//! permits (ADR 0002, ADR 0009). The blueprint names protobuf/Parquet; that
//! wire form and compression need a dependency record and are not claimed here.
//! What is claimed is the part that guards DATA-001: every field of every pack
//! kind is named, and a pack carrying one outside its schema is refused, so a
//! raw document body cannot ride along inside a pack by being an extra field.

use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The schema version this build writes and the only one it reads.
pub const SCHEMA_VERSION: u32 = 1;

/// The longest any one string in a pack may be, in bytes (R12-DATA-1041).
///
/// `deny_unknown_fields` stops a body riding along as an extra field, but a
/// summary's `text`, a fact's value or a lesson is free text, and nothing
/// stopped a writer putting the whole document there. A summary of this
/// length is still a summary; past it, it is an excerpt, and the durable
/// layer keeps derived knowledge, not the source (ADR 0180 decision 4).
pub const MAX_PACK_TEXT_BYTES: usize = 2_048;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PackBody {
    /// A derived summary, never an excerpt of the source.
    Summary {
        subject: String,
        text: String,
    },
    Embedding {
        subject: String,
        values: Vec<f64>,
    },
    WorldStateSnapshot {
        as_of_ms: u64,
        facts: BTreeMap<String, String>,
    },
    WorldModelDelta {
        from_version: u64,
        to_version: u64,
        added: Vec<String>,
        removed: Vec<String>,
    },
    ModelArtifact {
        name: String,
        artifact_sha256: String,
    },
    EpisodicMemory {
        episode: String,
        lessons: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnowledgePack {
    pub schema_version: u32,
    pub id: String,
    pub created_ms: u64,
    pub body: PackBody,
}

impl KnowledgePack {
    pub fn new(id: impl Into<String>, created_ms: u64, body: PackBody) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            id: id.into(),
            created_ms,
            body,
        }
    }

    /// Every string the pack carries, for the bound above.
    fn texts(&self) -> Vec<&str> {
        let mut texts = vec![self.id.as_str()];
        match &self.body {
            PackBody::Summary { subject, text } => texts.extend([subject.as_str(), text.as_str()]),
            PackBody::Embedding { subject, .. } => texts.push(subject.as_str()),
            PackBody::WorldStateSnapshot { facts, .. } => {
                texts.extend(facts.iter().flat_map(|(k, v)| [k.as_str(), v.as_str()]));
            }
            PackBody::WorldModelDelta { added, removed, .. } => {
                texts.extend(added.iter().chain(removed).map(String::as_str));
            }
            PackBody::ModelArtifact {
                name,
                artifact_sha256,
            } => texts.extend([name.as_str(), artifact_sha256.as_str()]),
            PackBody::EpisodicMemory { episode, lessons } => {
                texts.push(episode.as_str());
                texts.extend(lessons.iter().map(String::as_str));
            }
        }
        texts
    }

    /// Refuses a pack carrying any string longer than
    /// [`MAX_PACK_TEXT_BYTES`]: an excerpt is not derived knowledge.
    pub fn check_compact(&self) -> Result<()> {
        if let Some(long) = self
            .texts()
            .into_iter()
            .find(|t| t.len() > MAX_PACK_TEXT_BYTES)
        {
            return Err(Error::invalid(format!(
                "pack `{}` carries a string of {} bytes, past the {MAX_PACK_TEXT_BYTES}-byte \
                 bound; a knowledge pack keeps what was derived from a document, not the \
                 document — summarise it, or cite it by manifest",
                self.id,
                long.len()
            )));
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        self.check_compact()?;
        serde_json::to_vec(self).map_err(|e| Error::invalid(format!("pack does not encode: {e}")))
    }

    /// Refuses an unknown field, an unknown kind and any other schema version.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let pack: Self = serde_json::from_slice(bytes).map_err(|e| {
            Error::schema(format!(
                "not a knowledge pack of this schema: {e}; re-derive it"
            ))
        })?;
        pack.check_compact()?;
        if pack.schema_version != SCHEMA_VERSION {
            return Err(Error::schema(format!(
                "pack schema version {} is not {SCHEMA_VERSION}; migrate it explicitly",
                pack.schema_version
            )));
        }
        Ok(pack)
    }
}
