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

    pub fn encode(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(self).map_err(|e| Error::invalid(format!("pack does not encode: {e}")))
    }

    /// Refuses an unknown field, an unknown kind and any other schema version.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let pack: Self = serde_json::from_slice(bytes).map_err(|e| {
            Error::schema(format!(
                "not a knowledge pack of this schema: {e}; re-derive it"
            ))
        })?;
        if pack.schema_version != SCHEMA_VERSION {
            return Err(Error::schema(format!(
                "pack schema version {} is not {SCHEMA_VERSION}; migrate it explicitly",
                pack.schema_version
            )));
        }
        Ok(pack)
    }
}
