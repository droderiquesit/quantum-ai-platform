//! The mirror worker's links (ADR 0119 §1, R12-FABRIC-8078): the client to
//! the source region's fabric, the producer into the target region's
//! region-qualified stream, a durable checkpoint, and the loop that runs a
//! [`crate::mirror::Mirror`] over them.
//!
//! # Why these are separate from `Mirror`
//!
//! `Mirror` decides what is copied and measures lag; it holds no socket. The
//! three types here are the sockets and the disk, so a test can put the real
//! HTTP transport and a real second broker under the same `Mirror` that the
//! in-process suite drives, and the two cannot differ in what they copy.
//!
//! # What is still not built
//!
//! * **No binary starts a [`MirrorWorker`].** ADR 0119 §3 deploys it only at
//!   a second region, which ADR 0035 does not authorise; the worker is a
//!   library role the tests run over two listeners on one host.
//! * **The checkpoint is a file, not the regional Spanner row ADR 0119 names.**
//!   It survives a restart of the mirror on the same disk; it does not survive
//!   the loss of that disk. [`RemoteTarget`] is therefore also the second
//!   line: before producing a source offset it asks the target whether it
//!   already holds one there, so a checkpoint older than the target (a crash
//!   between the target's acknowledgement and the save) does not duplicate a
//!   record.
//! * **mTLS is the transport's, not this module's.** [`RemoteSource`] takes
//!   any [`FabricTransport`]; `HttpTransport::over` with a mutual-TLS link
//!   is the deployment's choice and is exercised by the transport's own tests.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration as StdDuration;

use qip_core::error::{Error, Result};
use qip_events::event_fabric::codec::{Batch, DecodeOutcome};
use qip_streaming::event_fabric::managed::CommittedBatch;
use qip_streaming::event_fabric::mirror::MirrorCheckpoint;
use qip_transport::event_fabric::producer::Producer;
use qip_transport::event_fabric::protocol::{
    FetchRequest, MetadataRequest, Request, Response, Route,
};
use qip_transport::event_fabric::transport::{FabricTransport, Timeouts};

use crate::mirror::{Mirror, MirrorReport, MirrorSource, MirrorTarget, committed_rows};
use crate::telemetry::FabricdTelemetry;

fn refused(route: Route, refusal: &qip_transport::event_fabric::protocol::Refusal) -> Error {
    Error::io(format!(
        "the {route:?} call was refused by the peer fabric: {refusal:?}"
    ))
}

/// The source region's fabric, read over a transport. Reads only what the
/// broker serves, and a fetch is bounded by the high watermark the broker
/// reports, so nothing past the committed bound is ever offered to a copy.
pub struct RemoteSource {
    transport: Mutex<Box<dyn FabricTransport + Send>>,
    timeouts: Timeouts,
}

impl RemoteSource {
    pub fn new(transport: Box<dyn FabricTransport + Send>, timeouts: Timeouts) -> Self {
        Self {
            transport: Mutex::new(transport),
            timeouts,
        }
    }

    fn call(&self, request: Request) -> Result<Response> {
        let mut transport = self
            .transport
            .lock()
            .map_err(|_| Error::io("the source transport's lock was poisoned"))?;
        transport.call(request, self.timeouts)
    }
}

impl MirrorSource for RemoteSource {
    fn committed_through(&self, stream: &str, partition: u32) -> Result<u64> {
        match self.call(Request::Metadata(MetadataRequest {
            stream: stream.to_string(),
            partition,
        }))? {
            Response::Metadata(metadata) => Ok(metadata.high_watermark()),
            Response::Refused(refusal) => Err(refused(Route::Metadata, &refusal)),
            other => Err(Error::io(format!(
                "the source answered a metadata call with {other:?}"
            ))),
        }
    }

    fn fetch_committed(
        &self,
        stream: &str,
        partition: u32,
        from: u64,
        max_bytes: u32,
    ) -> Result<Vec<CommittedBatch>> {
        match self.call(Request::Fetch(FetchRequest {
            stream: stream.to_string(),
            partition,
            offset: from,
            max_bytes,
        }))? {
            Response::Fetch(fetched) => committed_rows(stream, partition, fetched.batches()),
            Response::Refused(refusal) => Err(refused(Route::Fetch, &refusal)),
            other => Err(Error::io(format!(
                "the source answered a fetch call with {other:?}"
            ))),
        }
    }
}

/// Builds the producer for one target `(stream, partition)`. Supplied by the
/// composition root, which owns the identity, the retry budget and the link.
pub type ProducerFactory = dyn Fn(&str, u32) -> Result<Producer> + Send + Sync;

/// The target region's fabric: one producer per region-qualified lane, and a
/// second transport used only to ask what the target already holds.
pub struct RemoteTarget {
    factory: Box<ProducerFactory>,
    producers: Mutex<BTreeMap<(String, u32), Producer>>,
    probe: RemoteSource,
}

impl RemoteTarget {
    pub fn new(factory: Box<ProducerFactory>, probe: RemoteSource) -> Self {
        Self {
            factory,
            producers: Mutex::new(BTreeMap::new()),
            probe,
        }
    }
}

impl MirrorTarget for RemoteTarget {
    fn produce_copy(&self, stream: &str, partition: u32, mut batch: Batch) -> Result<u64> {
        let offset = batch.base_offset;
        // Already held? A checkpoint can trail the target by the copies that
        // were acknowledged and not yet saved. Producing again would give the
        // record a second offset, and the mirror would then refuse the
        // acknowledgement at the wrong offset after the duplicate was already
        // written. So ask first, and accept a held copy only if it is the
        // same records: an offset held by something else is the other writer
        // ADR 0119 forbids, and is refused by name.
        if self.probe.committed_through(stream, partition)? > offset {
            let held = self.probe.fetch_committed(stream, partition, offset, 1)?;
            let first = held.first().ok_or_else(|| {
                Error::io(format!(
                    "{stream}:{partition}: the target is committed past offset {offset} and \
                     served nothing for it"
                ))
            })?;
            let DecodeOutcome::Complete(existing) = Batch::decode(&first.bytes)? else {
                return Err(Error::io(format!(
                    "{stream}:{partition}: the target's batch at {offset} is torn"
                )));
            };
            if existing.records != batch.records {
                return Err(Error::invalid(format!(
                    "{stream}:{partition}: the target holds different records at offset \
                     {offset} than the source committed there; only the mirror may write a \
                     region-qualified stream, so stop and find the other writer"
                )));
            }
            return Ok(offset);
        }
        // Stamps belong to the accepting broker; see `MirrorTarget for Broker`.
        batch.partition = None;
        batch.origin = None;
        let mut producers = self
            .producers
            .lock()
            .map_err(|_| Error::io("the target producers' lock was poisoned"))?;
        let key = (stream.to_string(), partition);
        if !producers.contains_key(&key) {
            let mut producer = (self.factory)(stream, partition)?;
            producer.init()?;
            producers.insert(key.clone(), producer);
        }
        let producer = producers
            .get_mut(&key)
            .ok_or_else(|| Error::io("a producer inserted a moment ago is missing"))?;
        Ok(producer.send(batch)?.base_offset())
    }
}

/// A checkpoint file, one line per lane: `stream partition next_offset hash`,
/// with `-` for no hash. Written to a sibling temporary file and renamed, so
/// a crash mid-write leaves the previous checkpoint rather than half of one.
#[derive(Clone, Debug)]
pub struct FileCheckpoints {
    path: PathBuf,
}

impl FileCheckpoints {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Every stored lane. A missing file is a first run and answers empty; a
    /// file that exists and does not parse is refused, because starting
    /// again from zero over a corrupt checkpoint would re-copy a whole
    /// region's history on the strength of a guess.
    pub fn load(&self) -> Result<Vec<(String, u32, MirrorCheckpoint)>> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(Error::io(format!(
                    "cannot read the mirror checkpoint {}: {error}",
                    self.path.display()
                )));
            }
        };
        let mut rows = Vec::new();
        for (number, line) in text.lines().enumerate() {
            let bad = || {
                Error::invalid(format!(
                    "{}:{}: a checkpoint line is `stream partition next_offset hash|-`, got \
                     `{line}`; fix or remove the file rather than restart from zero",
                    self.path.display(),
                    number + 1
                ))
            };
            let mut fields = line.split(' ');
            let (Some(stream), Some(partition), Some(next), Some(hash), None) = (
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
            ) else {
                return Err(bad());
            };
            rows.push((
                stream.to_string(),
                partition.parse().map_err(|_| bad())?,
                MirrorCheckpoint {
                    next_offset: next.parse().map_err(|_| bad())?,
                    last_batch_hash: (hash != "-").then(|| hash.to_string()),
                },
            ));
        }
        Ok(rows)
    }

    pub fn save(&self, rows: &[(String, u32, MirrorCheckpoint)]) -> Result<()> {
        let mut text = String::new();
        for (stream, partition, checkpoint) in rows {
            if stream.contains(char::is_whitespace) {
                return Err(Error::invalid(format!(
                    "stream `{stream}` has whitespace and cannot be written to the checkpoint"
                )));
            }
            text.push_str(&format!(
                "{stream} {partition} {} {}\n",
                checkpoint.next_offset,
                checkpoint.last_batch_hash.as_deref().unwrap_or("-")
            ));
        }
        let temporary = self.path.with_extension("tmp");
        let io = |what: &str, error: std::io::Error| {
            Error::io(format!(
                "cannot {what} the mirror checkpoint {}: {error}",
                self.path.display()
            ))
        };
        std::fs::write(&temporary, text).map_err(|e| io("write", e))?;
        std::fs::rename(&temporary, &self.path).map_err(|e| io("replace", e))
    }
}

/// A mirror, its two links and its checkpoint file.
pub struct MirrorWorker {
    mirror: Mirror,
    source: Box<dyn MirrorSource + Send>,
    target: Box<dyn MirrorTarget + Send>,
    store: FileCheckpoints,
    telemetry: FabricdTelemetry,
}

impl MirrorWorker {
    /// Resume from whatever the store holds.
    pub fn new(
        mut mirror: Mirror,
        source: Box<dyn MirrorSource + Send>,
        target: Box<dyn MirrorTarget + Send>,
        store: FileCheckpoints,
        telemetry: FabricdTelemetry,
    ) -> Result<Self> {
        for (stream, partition, checkpoint) in store.load()? {
            mirror.restore(&stream, partition, checkpoint)?;
        }
        Ok(Self {
            mirror,
            source,
            target,
            store,
            telemetry,
        })
    }

    pub fn mirror(&self) -> &Mirror {
        &self.mirror
    }

    /// One pass, then the checkpoint. The checkpoint is saved after the
    /// target acknowledged, never before: a position saved first would skip
    /// a record the target never took.
    pub fn step(&mut self) -> Result<MirrorReport> {
        let report = self
            .mirror
            .pass(self.source.as_ref(), self.target.as_ref(), &self.telemetry);
        self.store.save(&self.mirror.checkpoints())?;
        Ok(report)
    }

    /// Step every `interval` until `stop` is set. A pass's lane failures are
    /// in its report and do not end the loop; a checkpoint that cannot be
    /// written does, because continuing would copy on a position that
    /// would not survive a restart.
    pub fn run(&mut self, stop: &AtomicBool, interval: StdDuration) -> Result<()> {
        while !stop.load(Ordering::SeqCst) {
            self.step()?;
            std::thread::sleep(interval);
        }
        Ok(())
    }
}

impl std::fmt::Debug for RemoteSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteSource")
            .field("timeouts", &self.timeouts)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for RemoteTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteTarget").finish_non_exhaustive()
    }
}

impl std::fmt::Debug for MirrorWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MirrorWorker")
            .field("store", &self.store)
            .finish_non_exhaustive()
    }
}
