# ADR 0156: Platform plumbing is restated to the accepted records: schema descriptors, archive format, storage classes and the telemetry drain have one home each

- **Status**: **Accepted**, 2026-10-19, under the owner's explicit approval ("I allow
  ai agent to decide correctly also i provide my full approval", 2026-10-19). That
  authorises the engineering choices below. It does not authorise a billed resource,
  an edit to a protected path, a risk value or anything near the paper boundary; the
  "Not decided here" section lists each and no row closes on them. **Nothing is built
  by this record.**
- **Date**: 2026-10-19
- **Extends**: [ADR 0100](0100-the-event-fabrics-first-build-is-an-in-tree-single-node-broker-and-a-separate-single-writer-ledger-on-std-tcp-with-producer-retained-durability.md)
  (the fabric, the segment archive, `qip-transport` as the one socket-owning lib),
  [ADR 0112](0112-mtls-through-rustls-where-the-platform-owns-the-socket-iam-invocation-on-cloud-run-and-quic-only-where-measured.md)
  (mTLS, QUIC only where measured),
  [ADR 0113](0113-the-ledger-of-record-moves-to-spanner-the-tick-lake-codec-waits-on-measurement-and-the-organisation-splits-into-projects-at-the-second-region.md)
  (the tick-lake codec, the project split at the second region).
- **Keeps**: [ADR 0109](0109-third-party-crates-are-admitted-from-a-committed-audited-allow-list-and-the-decision-core-stays-at-two.md)
  (no crate without an allow-list line),
  [ADR 0111](0111-event-fabric-replication-is-delegated-to-a-managed-consensus-and-not-written-in-tree.md)
  and its Amendment 1 (three brokers per region),
  [ADR 0118](0118-the-gcp-tenant-is-a-platform-folder-holding-one-labelled-budgeted-project-per-environment-and-a-seed-project-applied-once-by-the-owner.md)
  (one project per environment now),
  [ADR 0119](0119-the-cross-region-fabric-mirror-is-an-in-tree-asynchronous-replicating-consumer-and-no-write-waits-on-another-region.md)
  (the mirror), [ADR 0135](0135-fabric-clients-bootstrap-from-the-srv-record-through-an-in-tree-dns-query-keep-a-bounded-last-good-answer-and-fall-back-to-configured-addresses.md)
  (SRV discovery). ADR 0003 and the three paper layers are untouched.

## Context

Nineteen register rows (R12) sit PARTIAL for one of two reasons. Either the
requirement text still describes an architecture an accepted record has since
replaced (QUIC, five brokers, "the fabric does not mirror", "mTLS blocked"), or the
code has a seam with two possible homes and nobody has said which. The first kind is
closed by restating the requirement, and a restatement is a decision, not a
demotion: it says what the register must now ask for, and the test that pins the
real claim replaces the test that pinned a fixture. The second kind is closed by
naming one home.

Reading the eight records these rows lean on, they disagree in six places. They are
reconciled first because every decision below depends on which side wins.

### Reconciliation of ADRs 0100, 0109, 0111, 0112, 0113, 0118, 0119, 0135

| Contradiction | Ruling |
|---|---|
| ADR 0100 §7: plaintext TCP, bearer tokens, "mTLS BLOCKED(C2)". ADR 0112: mTLS on every Compute Engine hop, no plaintext fallback. | **0112 wins**, as its own header says (it amends 0100 §7). The code agrees: `qip-fabricd` binds a TLS listener with a required client certificate and admitted peer names, and `qip-transport`'s server hands the verified name to handlers. Three artefacts still describe 0100's world: the operator CLI (loopback plus bearer), `fabric_009` and the register text "mTLS blocked". Decision 4 fixes them. |
| ADR 0100 §1 and register: "the fabric does not mirror", `fabric-mirror` BLOCKED(C8). ADR 0119: mirror is an in-tree asynchronous consumer, built now, deployed at the second region. | **0119 wins on direction; 0100 describes the first build.** `StreamPolicy` still refuses every mirroring policy but `None` until the mirror is built (`fabric_083` pins that), so "refused today" is true and "cannot exist" is false. Decision 4(b) rewrites the wording. |
| ADR 0100 §3: RF1, replication waits for C2. ADR 0111: Spanner is the consensus for P0/P1. | **0111 wins for P0 and P1; 0100 §3 stands for P2-P4 and for the segment archive.** The archive decisions below are about the P2 segment archive and the sealed-segment objects, which 0111 did not touch. |
| ADR 0111 body: "two or three" brokers, module cap of three as the lease ceiling. Amendment 1: three per production region, "five only on a committed throughput measurement". The module's `broker_internal_addresses` validation refuses a fourth. | **Both are one rule.** Three is the ceiling today; five is reachable only by a later record that carries the measurement *and* resizes the lease. "Five" in a requirement is therefore a measured future, not a present gap. Decision 6(a). |
| ADR 0109's table: `zstd` "deferred to measurement", persisted codecs need their own ADR. ADR 0113 §3 and Amendment 1: `zstd` is the lake default, scoped to `qip-storage`. The allow-list now carries `zstd` (the lines name ADR 0113). | **The admission is scoped, not general.** A crate on the allow-list is admitted for the crate and the caller its line names. `zstd` is admitted for tick-lake segments in `qip-storage` and for nothing else. Decision 4(e). |
| ADR 0113 §4: NCC joins the per-zone VPCs at the second region. ADR 0118: one project per environment now. ADR 0119: mirror deployed at the second region. ADR 0035: one execution node in one region. | **One trigger, three consequences.** Nothing network-shaped, mirror-shaped or project-shaped is deployed before a record supersedes ADR 0035's "one". Decision 6(c) states what a hub may carry when that record arrives. |

## Decision

### 1. Schema descriptors: the committed lock is the one source, and the broker admits by (topic, version)

1. **The per-topic descriptors live in `qip-events/schemas.lock.json`**, the file the
   `event_fabric_schema_lock` suite already holds equal to the shapes computed from
   the Rust types. `qip-streaming` does not grow a second descriptor source and the
   service's `SchemaRegistry` is not populated by hand-written samples.
2. **`qip-events` gains a pure loader** (I/O-free, over a string it embeds with
   `include_str!`) returning `SchemaDescriptor` values from the lock rows.
   `Service::apply_catalogue` receives them and registers them in the service's
   registry. The composition root passes the embedded lock; nothing reads a file at
   run time, so a deployed broker cannot be pointed at a different lock than the one
   its image was built with.
3. **Admission is by `(topic, schema_version)`, not by runtime shape.** Every record
   already carries its envelope's `schema_version`. `Broker::require_admitted_topics`
   compares that version with the set the lock holds for the topic and refuses with
   `SchemaRefused` before append. Admission by *shape* at produce time is refused: the
   lock's own module documents that a shape read off a runtime value hides retyped
   fields behind `null` and empty arrays. A produce-path check that can pass a
   retyped body is a control that reads as protection. Shape is held where it can be
   held honestly, at build time, by the lock suite and the compatibility gate.
4. **A catalogue topic with no lock row is refused**, and a stream that names no
   topics is refused, instead of being "unrestricted, as before". A stream that
   admits everything is the failure the topic gate was written to remove; keeping it
   as a default keeps the hole. The catalogue file that is committed must be
   checked against this before the change lands.
5. **`the_schema_registry_is_a_production_caller_in_qip_fabricd` is rewritten, not
   weakened.** It asserted `len() == 0`; it will assert the registry holds exactly
   the lock's rows. The premise (an empty registry is not a production caller) is
   what made the old assertion true, and the new one is strictly stronger.
6. **Consumer inventory (CONTRACT-9121).** A committed
   `qip-events/consumers.lock.json` lists, per `(topic, version)`, the consumers that
   decode it: `{name, kind: crate|binary, type}`. It is a reviewed file, not a
   computation: which crates decode a type is not recorded in Cargo metadata, and a
   heuristic that guesses wrong in the quiet direction is the failure mode here.
   `qip event-fabric schema-gate` reads the base and head lock plus the head
   inventory and admits a change only if (i) a version that lost a field was bumped,
   as today, **and** (ii) the superseded version stays in the lock while any consumer
   row for it is not `migrated`, and leaves only when
   `ConsumerInventory::retire`'s own rule holds. A version bump alone is no longer
   "the producer's deliberate break, nobody asked".
7. **The inventory is kept honest by a conservative acceptance check**: every
   inventoried consumer's source names the body type, and every workspace crate whose
   source names a bound body type is inventoried. It proves a lower bound, and the
   test says so; it does not claim to compute consumers.
8. **No dual-read/write tooling is built.** Dual-read already exists: the envelope
   refuses a *newer* version than the type supports and decodes older ones. Dual-write
   is the lock carrying both versions during the window, which (ii) enforces. A tool
   that rewrites records between versions would be a second writer of history.

### 2. The archive: segment objects are immutable and never merged; a per-batch digest is a reserved format, built on a measurement

1. **No compaction merges archive objects.** `Compactor` concatenates every object
   under a prefix into a content-addressed blob and returns the originals as
   deletable. On the archive that is wrong both ways: keep the originals and storage
   grows by a merged copy per pass (the compactor re-reads its own output); delete
   them and `ArchiveReader::hydrate` fails for every archived segment, because each
   is fetched by the exact key its manifest names and verified against the
   manifest's content hash. Rewriting the manifest key "atomically with the merge" is
   not available on object storage, which has no multi-object transaction, and it
   would also break the seal chain: `chain_in` and `chain_out` are hashes of the
   *segments*, so a merged object has no place in the chain `verify_seal_chain`
   walks. The archive's objects are the unit of evidence; evidence is not
   re-encoded for tidiness.
2. **Fewer, larger archive objects come from the seal, not from a later pass.**
   `roll_after_bytes` is the knob; `archive_names`' size band already refuses
   objects outside it. A flood of small objects is fixed where the segment is cut.
3. **`Compactor` is for the lake's small objects only, and it refuses the archive.**
   It refuses the segment-object prefix and the manifest-index prefix by name,
   honours `min_object_size` (which it stores and never reads today, so its
   documented behaviour and its actual one differ), and has no production caller
   until the lake needs one. A dead compactor with a refusal is cheaper than a live
   one with a footgun.
4. **Per-batch digest (TICK-2058): the format is decided, the build waits.** The
   manifest's `replay_index` holds `(offset, byte_start)` and nothing a fetched byte
   range could be checked against; `read_indexed` therefore re-proves the *whole
   segment* hash before it serves a batch, and that guarantee, "never returns
   unverified bytes", is the reason a ranged read cannot simply be added. The format
   that would allow it: a manifest wire version, and replay-index entries gaining the
   SHA-256 of the batch's bytes as they sit in the segment. A manifest without the
   field (every one written so far; the wire struct has no version today, so the new
   field is optional on read) is served by the whole-segment path and is never
   refused. A ranged read is a **new, separately named method** whose result says
   what it proved (the batch equals the digest this manifest recorded), not a faster
   `read_indexed`. It proves less: the digests are trusted exactly as far as the
   manifest is, whereas the whole-segment path trusts the manifest only for the hash.
   That weakening is real and is stated here out loud (principle 4 of the judging
   list), which is why it is built only if a committed measurement of hydrate bytes
   and latency per `read_indexed` on the broker's replay path shows the whole-segment
   cost is the problem. Segment size is bounded by `roll_after_bytes`, and the last
   hydrated segment is cached, so no such measurement exists today.

### 3. Storage classes: a committed list in `qip-storage`, checked where a store is made

1. **The declared list lives in `qip-storage`** (a lib, so it can be read by every
   app): each entry is a *class* with its namespace or archive role, its write
   discipline (`immutable` for the fabric archive, `overwrite` for the rest) and its
   owner. The classes found in the tree: the fabric archive, the tick lake, model
   artifacts, knowledge packs, reflex packages, the tick desk's output, and the
   preflight namespace. Apps' own `*_NAMESPACE` constants are re-pointed at the list;
   an app-owned constant that the lib cannot see is what made a fixed prefix list
   look impossible.
2. **The guard is on construction, not on `validate_key`.** The store is reached two
   ways with different key shapes (fabricd's archive under a configurable prefix
   writing `segments/sha256/..`, `manifests/..` and a startup probe; the provider
   making the namespace the prefix and writing arbitrary keys under it).
   `StorageProvider::blobs(namespace)` refuses a namespace the list does not
   declare, and fabricd's archiver construction refuses unless its role is the
   declared archive class. The prefix remains configurable; the *class* is not.
   Within the archive class the archive types validate their own key shapes.
3. **An empty or absent list refuses everything.** Fail-closed means a build that
   forgot the list cannot write, not that it can write anywhere.
4. **No raw-corpus class is declared.** A raw vendor corpus is a licensing question
   that `qip-data-finder` answers before a source is used, and a class declared
   ahead of that answer is permission granted in advance. Adding a class is a
   reviewed diff to the list with a test.
5. **Buckets are not enabled by this record.** `enable_cloud_storage` is the owner's
   switch in a protected tfvars file and the buckets bill. The acceptance test
   cross-checks that every bucket the data module declares has at least one
   declared class and every class names a bucket, so a bucket cannot appear
   unclassified and a class cannot point at nothing.

### 4. Requirement texts are restated to the accepted records

(a) **Transport and QUIC (FABRIC-10195, FABRIC-10076).** The requirement becomes:
"fabric sessions use mutual TLS 1.3 over HTTP/1.1 through `rustls`, both certificates
verified, the peer identity taken from the certificate; QUIC is admitted for a hop
only on a committed TCP+TLS against QUIC measurement (ADR 0112 §4 and Amendment 1)".
`fabric_009` is rewritten to drive a real mutual-TLS listener and client from
`qip-transport` and assert a client without a certificate never reaches a handler;
its QUIC and "session keys derived from the event log's trust anchor" assertions go,
because the second names no mechanism in the tree (the trust anchor is the CA pool),
and the no-QUIC fact stays pinned by `fabric_036`'s gap-pin. `fabric_007` is rewritten
to produce a control frame to a P0 stream keyed by region and by cell, fetch it back,
and assert a cell's partition holds its region's frame and the global one. Neither is
closed by weakening: each asserts more than the fixture did.

(b) **Mirroring and the three views (FABRIC-7042).** "The fabric does not mirror"
becomes "the mirror is not built; ADR 0119 specifies it". The *mirror-lag view* is
the mirror's checkpoint read as `lag` already reads a group's, and is built with the
mirror. The other two are decided now. **Health** is the metadata route extended to
report isolation (the broker already exposes `Broker::isolation`; the route does not
carry it), with a `qip event-fabric health` that prints leader epoch, high watermark,
`archived_through` and isolation as the broker computed them, refusing rather than
printing a field the response lacks. **Replay** is `qip event-fabric replay`: fetch a
partition from an offset through the SDK, verify the batch chain, print the verdict
in `qip replay`'s AGREES/DIFFERS discipline. It reads the fabric; it does not
re-drive anything.

(c) **A second consumer (FABRIC-10078).** The plural is restated, not served. The
fabric has one production consumer service, `qip-ledgerd`, by design: ADR 0100 §2
keeps financial state out of `qip-api`, so a read model there is the placement that
decision rejected. The first honest second consumer is ADR 0119's mirror, a
replicating consumer in its own group. Nothing is built to satisfy a word. Any
consumer added later names its group, its stream and its catalogue grant in its own
record.

(d) **Admin-plane authentication (FABRIC-8081).** The operator's identity is the
verified client-certificate name, read from the server-written peer-identity header,
and the broker authorises it against the catalogue's admin grants; `--operator`
becomes a check that the name equals the certificate's, no longer a free-text
attribution. Today the service never reads that header, so over mTLS the identity is
still the bearer token's, and the CLI still refuses any non-loopback peer. The bearer
path survives only for a `local` plaintext broker, as ADR 0112 §2 already says of
the listener. Remaining verbs (quota or retention change, partition move, drain) are
**not** added by this decision; each is its own slice with its own refusal test. An
operator certificate is issued from the same CA pool under a template that names the
operator's identity; creating that template is infrastructure and is listed below.

(e) **Compression (MESH-9181).** No compression codec is admitted on the peer mesh or
any transport. The row asks whether to admit a dependency; the code shows one is
already admitted, scoped to lake segments. Mesh frames are bounded (a 64 KiB frame,
a fan-out limit, a centre answer cap) and carry the compact touch encoding; a codec
on that hop adds an inflate-amplification surface at the receiver, on a channel
adjacent to the decision loop, to save bytes the bounds already cap. An acceptance
check pins that only `qip-storage` depends on a compression crate. Extending `zstd`
elsewhere needs a measurement and an allow-list line, which is a protected edit.

(f) **Game-day fixture (RES-10154).** The ledgerd-loss scenario may include a
signed-policy publisher fixture, on these terms: the fixture is an operator-tool
subcommand that refuses unless the deployment is `local`; it generates an ECDSA P-256
key in memory with the existing `LocalSigner::generate`, writes only the *public*
trust bundle into the scenario's temporary directory before the node starts, and
signs the frames it publishes. The node verifies against that bundle through its
ordinary `QIP_TRUST_BUNDLE_PATH` root. No private key is written, committed or logged;
no edge-node test mode exists; the digest check is untouched. A static scan refuses
key material in the scenario files. The verdict flips from inconclusive only if the
outcomes stream is non-empty.

### 5. Telemetry: one drain, in a services crate; the cross-process graph is joined at ingestion

1. **The OTLP drain moves to one new services crate, `qip-telemetry-drain`.** It needs
   `qip-transport`'s client and `qip-observability`'s `Snapshot::to_otlp_metrics`,
   both libs; a lib cannot host it because it spawns a thread and performs network
   I/O, and an app cannot because no app may depend on an app. A services crate may
   depend on both libs and every app may depend on it. It adds no third-party crate.
2. **Configuration is a value, not the environment.** The crate takes a lookup
   closure (the existing `read` form); `from_env` stays in each app, because
   `std::env` is prohibited outside `apps/**`.
3. **The three ~780-line copies are deleted in the same change** that adds the crate
   and the three apps adopt it. The byte-identical pin in the deepbrain tests is
   replaced, not dropped: an acceptance check asserts the exporter's entry point is
   defined in exactly one file in the workspace. Structure now holds what a test of
   identical copies held.
4. **Nothing is pointed at a collector by this record.** `QIP_OPENOBSERVE_URL` stays
   unset: no collector is vendored or attested, the egress bootstrap has no listener
   for one and `manifest_wiring` argues the entries unset. Ingestion stays unproven
   and the platform is not described as observable.
5. **`qip-fabricd` spans (OBS-8082).** Spans are recorded at the service's request
   handler, not inside the `Broker` library, with attributes `stream`, `class` and
   `outcome`, never a key, producer or order identifier, so cardinality is bounded by
   the catalogue. fabricd exports them through the shared drain only when the drain
   is configured; unset, they are recorded and unexported, as the brains' are today.
   `qip-ledgerd` and `qip-edge-node` adopt the drain the same way once the home
   exists; the edge node's scrape endpoint is unchanged.
6. **The platform graph (OBS-7115) is joined where signals are ingested, not by
   processes calling one another.** No process pushes its entity graph to another, and
   none scrapes a peer across a VPC boundary: the reflex, fabric and service networks
   are separate and unjoined, so a pull from the deep brain to an edge node has no
   path today, and opening one to make a graph is a topology decision. Each process
   keeps its in-process graph, scoped to itself, and the deployment entity key moves
   from a literal to `service`, `region` and `cell` values so that a later join is by
   key. The row stays PARTIAL until ingestion is shown; that is deployment work.

### 6. Infrastructure rules

(a) **Brokers (FABRIC-8096).** The requirement is restated to ADR 0111 Amendment 1:
"three brokers per production region, one per zone, C4 or C4D; five only after a
committed throughput measurement and a record that resizes the lease". The module's
existing refusals (one per zone, in-region, shape, at most three) stay. Whether `prod`
sets `event_fabric_broker` is the owner's switch in a protected tfvars file and the
instances bill.

(b) **NCC export (FABRIC-8099).** No hub, spoke or export filter is declared before
the second region. The row presumed a mirror to export for; ADR 0119 makes the mirror
real, but its deployment waits for the same trigger, so there is still nothing to
export. Reserved internal addresses stay as built.

(c) **Topology at the trigger (GCP-8053).** When the record superseding ADR 0035's
"one" arrives, the hub joins VPC *spokes* (Shared VPC stays refused), and route
export is **deny by default, per pair**: the only prefix a spoke may export is its
control-fabric subnet, and only to the other regions' control-fabric spokes, because
the mirror's mTLS pull is the one cross-region flow ADR 0119 authorises. The reflex
and service VPCs are never exported to the hub. An acceptance scan pins today's half
now: no hub, spoke or VPC peering is declared. The hub, its attachments and its data
transfer are billed and are the owner's.

(d) **Cold-start tolerance (GCP-10032).** Every catalogue workload with
`min_instances = 0` carries a non-empty `cold_start_tolerance` stating what a cold
start costs its caller and why that is acceptable, mirroring the
`always_on_justification` the module already demands of a floor above zero; an
acceptance test refuses an empty one and keeps the brains' floor of one. **No latency
number is invented**: a seconds-per-workload SLO is an owner input and the rule does
not need one. The field is read by that test and by a reviewer reading the diff,
which is where the catalogue's other written justifications are read; the row's
objection that it is "consumed only by the test" is accepted and answered by the
`always_on_justification` precedent, whose consumers are the same.

## Consequences

- A record on a fabric stream is admitted by topic *and* version from one committed
  file, and a topic with no lock row cannot be streamed.
- A schema bump is no longer a silent break: the old version stays in the lock until
  its inventoried consumers have moved.
- The archive's objects stay the evidence they are; the storage the compactor would
  have tidied is bounded at the seal.
- No store is made for an undeclared class, and the declared list is the answer to
  "where may this platform write".
- Three drains become one, and the exporter's behaviour is fixed in a place a test
  can hold.
- Nine rows close by restating the requirement; the register stops asking for QUIC,
  five brokers and a mirror that cannot exist.

## What it costs

- A new services crate, a rewritten registry test, a rewritten two acceptance
  fixtures, a catalogue field and a committed consumer inventory someone must keep
  current; a stale inventory is the quiet failure, which is why a check walks the
  workspace for crates that name a bound type.
- A catalogue stream that lists no topics will now fail to start. That is the point,
  and the change must read the committed catalogue first.
- Admission by version rather than shape means a producer that retypes a field
  *without* bumping the version is caught at build time by the lock suite, not at
  produce time. If the lock suite is skipped, the broker will not notice.
- A ranged archive read is deferred, so hydrate cost stays whole-segment until a
  measurement says otherwise.
- The operator CLI needs a workload certificate source it does not have, so admin
  from a laptop is not possible until the operator template exists; incident access
  stays "on the broker's own host" meanwhile.

## Alternatives rejected

- **Admit by runtime shape in `Broker::produce_to`.** Unsound for the reason in 1.3.
- **Compute the consumer inventory from Cargo metadata.** A heuristic with a quiet
  failure direction; the reviewed file is the form `retirement.rs` already assumes.
- **Merge-compact the archive, rewriting manifests.** See 2.1.
- **Add the per-batch digest and ranged reads now.** A weaker proof shipped without
  a measured need.
- **A fixed prefix list inside `validate_key`.** It would refuse fabricd's prefixed
  archive and every provider namespace outside the four it named.
- **Make the drain a lib or a fourth copy in `qip-fabricd`.** The first performs I/O in
  a lib; the second repeats the ~780-line liability twice more.
- **Push each process's graph to the deep brain.** It makes the deep brain a
  collector with an inbound listener and an auth story, and crosses unjoined VPCs.
- **A test-only edge-node mode that deploys a plan without a payload** (RES-10154).
  It weakens the digest check and is refused.

## Dissent

**The strongest case against 2.1 and 2.4 is cost.** An archive that only ever grows
by many small immutable objects pays a request per object forever, replay that must
hydrate a whole segment to read one batch pays for the bytes it discards, and
object storage has lifecycle rules built for exactly this tidying. A principal
engineer could fairly say: build the digest, build the merge, accept the weaker
proof for the cheaper store. It lost because the archive's whole value is the
chain: a merged object has no seat in it, and a ranged read that trusts the
manifest where the whole-segment path trusts only the hash is a weaker guarantee
bought for a cost nobody has measured. It is revived by a committed measurement
showing archive request cost or hydrate cost exceeding what a larger
`roll_after_bytes` can remove; the reserved format in 2.4 is what makes that cheap
to act on.

**On 5.6** a reasonable view is that the graph should be built now by a scrape from
the deep brain, since the signals exist. It lost because the network does not
connect the processes, and because a graph assembled before ingestion is shown would
chart a platform nothing has proved is being observed. It is revived by NCC at the
second region together with an attested collector.

## What would make this wrong

- A produce accepted for a topic or version the lock does not hold, or a deployed
  broker whose embedded lock differs from the one its image's suite verified.
- A merge of archive objects, or a manifest rewritten after its segment was archived.
- A ranged archive read that returns bytes without a stated, tested proof.
- A store made for a namespace not on the declared list, or a bucket with no class.
- A second definition of the OTLP exporter anywhere in the workspace, or any
  `QIP_OPENOBSERVE_URL` set before a collector is attested and listened for.
- An operator action recorded under a name that is not the verified certificate name.
- A compression crate depended on outside `qip-storage`.
- A hub or peering resource declared before the record that supersedes ADR 0035.
- Admin or mirror traffic accepted over plaintext outside a `local` deployment.

## Code it constrains

Cited by symbol; locate each with the command beside it.

- `SchemaDescriptor`, `SchemaRegistry` (`qip-events/src/registry.rs`) and the lock
  file `qip-events/schemas.lock.json`:
  `grep -rn 'schemas.lock.json' backend/crates --include=*.rs`.
- `Service::apply_catalogue` (`qip-streaming/src/event_fabric/service.rs`),
  `Broker::register_body_topics`, `Broker::require_admitted_topics`
  (`.../broker.rs`): `grep -n 'fn apply_catalogue\|fn register_body_topics\|fn require_admitted_topics' backend/crates/services/qip-streaming/src/event_fabric/*.rs`.
- `the_schema_registry_is_a_production_caller_in_qip_fabricd`:
  `grep -rn 'fn the_schema_registry_is_a_production_caller_in_qip_fabricd' backend/crates`.
- `schema_gate::run` (`qip-cli/src/event_fabric/schema_gate.rs`),
  `ConsumerInventory` (`qip-events/src/retirement.rs`):
  `grep -rn 'struct ConsumerInventory\|fn run' backend/crates/apps/qip-cli/src/event_fabric/schema_gate.rs backend/crates/libs/qip-events/src/retirement.rs`.
- `Compactor::compact`, `CompactionConfig::min_object_size`
  (`qip-storage/src/compaction.rs`); `Archiver`, `ArchiveReader::read_indexed`,
  `Manifest::replay_index` (`qip-storage/src/segment/archive.rs`):
  `grep -rn 'min_object_size\|fn read_indexed\|replay_index' backend/crates/libs/qip-storage/src`.
- `StorageProvider::blobs` (`qip-storage/src/provider.rs`), the `*_NAMESPACE` constants:
  `grep -rn 'NAMESPACE: &str' backend/crates --include=*.rs`.
- `fabric_007_..`, `fabric_009_..`
  (`qip-acceptance/tests/event_fabric_infrastructure_basics.rs`):
  `grep -n 'fn fabric_007\|fn fabric_009' backend/crates/tests/qip-acceptance/tests/event_fabric_infrastructure_basics.rs`.
- `Server::bind_tls`, `PEER_IDENTITY_HEADER` (`qip-transport/src/server.rs`), the CLI's
  loopback refusal in `qip-cli/src/event_fabric/admin.rs`:
  `grep -rn 'PEER_IDENTITY_HEADER\|is_loopback' backend/crates/libs/qip-transport/src backend/crates/apps/qip-cli/src`.
- The three drains: `grep -rn 'pub fn export_once' backend/crates/apps`.
- `broker_internal_addresses` validation:
  `grep -n 'broker_internal_addresses' infrastructure/terraform/modules/event-fabric-broker/variables.tf`.
- `cloud_run_catalogue`: `grep -n 'min_instances\|always_on_justification' infrastructure/terraform/catalogue.tf`.
- `LocalSigner::generate`, `choose_root`: `grep -rn 'fn generate\|fn choose_root' backend/crates/libs/qip-transport/src/trust.rs backend/crates/apps/qip-edge-node/src/trust.rs`.

## Rows unblocked

- **R12-FABRIC-8090**: descriptors come from the committed lock; admission by (topic, version); empty-topic streams refused.
- **R12-FABRIC-10078**: plural restated; ledgerd is the sole consumer service until the mirror; nothing built.
- **R12-CONTRACT-9121**: committed consumer inventory, gate rule, no migration tool.
- **R12-FABRIC-8081**: certificate-name identity plus catalogue admin grants; more verbs are separate slices; operator template listed below.
- **R12-FABRIC-7042**: mirroring wording restated to ADR 0119; health and replay views decided; mirror view built with the mirror.
- **R12-FABRIC-10195**: requirement is mTLS over HTTP/1.1; `fabric_009` and `fabric_007` rewritten to drive real code.
- **R12-FABRIC-10076**: QUIC clause restated to ADR 0112's measured-only rule; closes with no code.
- **R12-FABRIC-8096**: restated to three per region; five needs measurement and a lease record; prod switch is the owner's.
- **R12-FABRIC-8099**: no export before the second region; nothing to build now.
- **R12-DATA-9187**: archive objects are never merged; compactor refuses the archive and honours its minimum.
- **R12-TICK-2058**: per-batch digest format reserved, built only on a hydrate measurement.
- **R12-DATA-9055**: declared class list in `qip-storage`, checked at construction; buckets are the owner's.
- **R12-MESH-9181**: no compression codec on the mesh; `zstd` stays scoped to the lake.
- **R12-RES-10154**: ephemeral-key, local-only fixture publisher in the scenario; no key material committed.
- **R12-OBS-7115**: graph joined at ingestion by shared entity keys; no cross-process push or scrape.
- **R12-OBS-8082**: handler-level spans with bounded attributes; export waits on the drain and a collector.
- **R12-OBS-9140**: `qip-telemetry-drain` services crate, three copies deleted; collector deploy is the owner's.
- **R12-GCP-10032**: `cold_start_tolerance` required for a zero floor; no SLO number invented.
- **R12-GCP-8053**: topology rule decided (deny-by-default, fabric-subnet-only); no hub before the second region; hub billed.

## Not decided here

- **BILLED**: the NCC hub, spokes and their data transfer (R12-GCP-8053, R12-FABRIC-8099); `prod` broker instances, and any count above three per region (R12-FABRIC-8096); the buckets behind `enable_cloud_storage` (R12-DATA-9055); a collector for OTLP, vendored or hosted, and its ingestion (R12-OBS-9140, R12-OBS-8082, R12-OBS-7115); the CA Service template and per-certificate issuance for operator certificates (R12-FABRIC-8081); a second consumer region's mirror host (R12-FABRIC-7042).
- **PROTECTED**: setting `event_fabric_broker` for `prod`, `enable_cloud_storage`, or any `QIP_OPENOBSERVE_*` value in `infrastructure/environments/*/terraform.tfvars`; any further allow-list line should `zstd` or another codec be wanted outside `qip-storage` (`scripts/dependency-allowlist.txt`). The need is described; the edits are not decided.
- **PAPER-BOUNDARY**: none touched. The admin plane, the mirror, the drain and the fixture publisher open no order path; the three layers and the ledger's fourth fence stand.
- **RISK**: none. No limit value, threshold or reserve appears in this record.
