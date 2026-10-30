//! ADR 0100 §8: re-drives `run_pass` from recorded exogenous inputs — tape
//! events, control frames and clock ticks — against a `ManualClock`, never a
//! recording layer wrapped around the venue. SLICE-39.
//!
//! **The re-drive is not built yet; read the line above as the module's
//! purpose, not its contents.** Until 2026-10-25 this file held only those
//! three lines, and a triage read them as a replayer that existed. What is
//! here is the step in front of any re-drive (R12-CONTRACT-7073, ADR 0155
//! section 16): turning the pass markers a node spooled into
//! [`ReplayManifest`]s that state what the run assumed, and refusing a
//! marker recorded under a configuration other than the one the replay
//! assumes.
//!
//! # Why the refusal is on the digest
//!
//! A marker proves which inputs a pass applied. The assumptions a manifest
//! states — latency, queue, fill, impact — are rendered from the gateway
//! settings the *replaying* side believes the run used, and nothing in the
//! marker names those settings except `config_digest`, which the node widens
//! to cover them on the simulated path (`gateway::widen_config_digest`). So
//! the digest is the only evidence that the assumptions written beside a
//! marker are the run's. A manifest built without comparing it would state
//! the default fill model for a run under another, and its reproducibility
//! hash would certify that statement.
//!
//! # What this reads, and what it refuses to read
//!
//! [`markers_in`] opens the spool through [`SegmentLog::open`], which is the
//! writer's open: it recovers a torn tail. So it copies the spool's files to
//! a scratch directory and opens the copy, and a running node's spool is
//! never truncated by a replay read (the in-process guard that refuses a
//! second open of one directory does not reach across processes). It
//! refuses a directory that does not exist rather than creating one and
//! reporting no markers. A copy taken mid-append may end in a partial
//! batch, which recovery drops from the copy only.

use crate::event_fabric::writer::{self, SpooledRecord};
use crate::gateway::{SimulatedGateway, assumptions_of, widen_config_digest};
use qip_brokers::exchange::ExchangeSettings;
use qip_contracts::replay::{PassMarker, ReplayManifest};
use qip_core::error::{Error, Result};
use qip_core::{Clock, ManualClock, Timestamp};
use qip_events::Topic;
use qip_storage::segment::log::{SegmentLog, SegmentLogConfig};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// The manifest for `marker`, stating the assumptions of a simulated gateway
/// built with `settings` on a node whose cell configuration digests to
/// `cell_digest`.
///
/// Refused when the marker's `config_digest` is not that pair widened: the
/// pass ran under another configuration or other gateway settings, and the
/// assumptions this would state are not the run's.
pub fn manifest_for(
    marker: PassMarker,
    cell_digest: &str,
    settings: &ExchangeSettings,
) -> Result<ReplayManifest> {
    let expected = widen_config_digest(cell_digest, settings);
    if marker.config_digest != expected {
        return Err(Error::invalid(format!(
            "pass {} of cell {} was recorded under configuration digest {}, and the replay \
             assumes {expected}; the pass ran under another configuration or other gateway \
             settings, so the assumptions a manifest would state are not the run's. Replay \
             under the configuration the node printed at start-up",
            marker.pass, marker.cell, marker.config_digest
        )));
    }
    ReplayManifest::with_assumptions(
        marker,
        assumptions_of(settings),
        SimulatedGateway::replay_model_versions(),
    )
}

/// The manifest of every marker in `markers`, refused as a whole on the first
/// one [`manifest_for`] refuses, and refused when there are none — an empty
/// replay agrees with every configuration.
pub fn manifests(
    markers: Vec<PassMarker>,
    cell_digest: &str,
    settings: &ExchangeSettings,
) -> Result<Vec<ReplayManifest>> {
    if markers.is_empty() {
        return Err(Error::invalid(
            "the spool holds no pass markers, so there is nothing to replay; a node writes one \
             per pass only while QIP_VENUE_FEED=simulated runs passes",
        ));
    }
    markers
        .into_iter()
        .map(|marker| manifest_for(marker, cell_digest, settings))
        .collect()
}

/// The pass markers among `records`, in spool order.
pub fn markers(records: &[SpooledRecord]) -> Result<Vec<PassMarker>> {
    records
        .iter()
        .filter(|record| record.event.topic == Topic::ReflexPassMarked)
        .map(|record| writer::pass_marker(&record.event))
        .collect()
}

/// `qip-edge-node replay-manifests --spool <dir> --cell-digest <hex>`: one
/// manifest per spooled pass marker, as one JSON line each, under the
/// simulated gateway's default settings — the settings the node runs the
/// simulator with ([`SimulatedGateway::default_settings`]). The cell digest
/// is the one the node prints at start-up; it is taken rather than
/// recomputed because recomputing it would need the whole node
/// configuration, and a replay that guessed at it would refuse every run.
pub fn run(arguments: &[String]) -> Result<Vec<String>> {
    let flag = |name: &str| {
        arguments
            .iter()
            .position(|argument| argument == name)
            .and_then(|index| arguments.get(index + 1))
            .ok_or_else(|| {
                Error::invalid(format!(
                    "qip-edge-node replay-manifests --spool <dir> --cell-digest <hex>: {name} \
                     is required"
                ))
            })
    };
    let markers = markers_in(Path::new(flag("--spool")?))?;
    manifests(
        markers,
        flag("--cell-digest")?,
        &SimulatedGateway::default_settings(),
    )?
    .iter()
    .map(|manifest| {
        serde_json::to_string(manifest)
            .map_err(|error| Error::invalid(format!("the manifest: {error}")))
    })
    .collect()
}

/// The pass markers in the spool at `directory`. See the module
/// documentation for why the directory must exist and must not be a running
/// node's.
pub fn markers_in(directory: &Path) -> Result<Vec<PassMarker>> {
    if !directory.is_dir() {
        return Err(Error::invalid(format!(
            "{} is not a spool directory; name the directory the node printed as its spool",
            directory.display()
        )));
    }
    // Opened on a copy, never in place: `SegmentLog::open` is the writer's
    // open and truncates a torn tail, which on a running node's spool is the
    // batch its writer is appending. The copy is the only thing recovery can
    // touch, and it is removed on the way out whatever the read found.
    static COPIES: AtomicU64 = AtomicU64::new(0);
    let copy = std::env::temp_dir().join(format!(
        "qip-replay-spool-{}-{}",
        std::process::id(),
        COPIES.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&copy);
    let read = copy_spool(directory, &copy).and_then(|()| {
        // The log's clock only stamps segment rolls, and a read rolls nothing.
        let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(Timestamp::from_secs(0)));
        let log = SegmentLog::open(&copy, SegmentLogConfig::new(clock))?;
        markers(&writer::records(&log)?)
    });
    let _ = std::fs::remove_dir_all(&copy);
    read
}

/// Copy the spool's files and directories (its segments and its manifest
/// directory) into `to`. Symbolic links are not followed: a spool holds
/// none, and following one would copy whatever it pointed at.
fn copy_spool(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_spool(&entry.path(), &to.join(entry.file_name()))?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), to.join(entry.file_name()))?;
        }
    }
    Ok(())
}
