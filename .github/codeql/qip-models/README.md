# qip/rust-models — the repository's CodeQL model pack

This pack tells CodeQL things about this code that it cannot work out from
names. It is loaded by `.github/workflows/codeql-analysis.yml` for the Rust
analysis only, through `CODEQL_ACTION_EXTRA_OPTIONS`, which hands
`--additional-packs=<checkout>/.github/codeql/qip-models --model-packs=qip/rust-models`
to `codeql database run-queries`. It is protected like `scripts/sast-baseline.json`
(`.github/**` is in `scripts/agent-policy.json`): an agent-authored change to a
row needs a person's approving review, because a wrong row here is a finding
nobody sees.

## Why a model pack and not a code change

On 2026-10-18 the `sast / Analyze (rust)` job refused 33 `rust/cleartext-logging`
findings beyond the baseline. Reproduced locally with the same bundle
(CodeQL 2.27.2, `codeql/rust-queries` 0.1.44), every one of the 33 traced back
to five source expressions, and every one printed something harmless: order
ids, refusal strings, counts, a banner. The analyser's reasoning was:

1. A call is a sensitive source if its **name** matches a regex —
   `acc(ou)?nt`, `secret`, `cert`, `api.?(key|tok)` — whatever its type
   (rust-all `SensitiveData.qll`, `HeuristicNames`).
2. `f()?` carries the whole Result's taint to the enclosing function's return
   value, so one `from_secret(..)?` inside `CentralPlane::assemble` taints every
   `Platform` ever built.
3. A tainted struct taints every field read and method result
   (`TaintTrackingImpl.qll`, "read steps give rise to taint steps").
4. Two generated sink rows are wrong on their face: pushing into a `Vec` or a
   `VecDeque` is modelled as logging (`<alloc::vec::Vec>::insert` and
   `<alloc::collections::vec_deque::VecDeque>::push_front`, `Argument[self]`).

No shape of code escapes 1–3 without removing the call or renaming it, and a
rename to dodge a scanner is refused in this repository. Raising the baseline
is refused too (ADR 0117, Amendment 1). What is honest is to state, where a
reviewer can check it, that a particular value is not a secret in any printable
form, and to let the analyser continue from there. That is what a
`barrierModel` row does: flow stops at that call's return value.

## The rule for a row

A row may sit only on a value that carries **no secret bytes in any form a
format string can print**. "Redacted `Debug`" counts only when `Display` is
redacted or absent too and there is no public accessor that hands the bytes
back. A row must never sit on the function that *returns* the secret
(`VenueCredential::secret`, `qip_core::secret::resolve_from`), on a struct that
holds one as a plain `String`, or on anything that would hide a real
`println!("{secret}")`. `neutralModel` is not used: it would switch off the two
wrong generated sinks, but a sink row is about where text goes, and this
repository would rather keep every log sink and reason about sources.

## The rows, and the evidence for each

| Row | What the value is | Check |
|---|---|---|
| `<qip_edge_node::gateway::SimulatedGateway>::venue_account` | A `VenueAccount`: venue id, the quantity still open per order id, a count of resting quotes, a timestamp. "Account" in the sense of a statement, not an identifier. | `backend/crates/edge/qip-edge/src/resume.rs`, `pub struct VenueAccount` |
| `<qip_edge::cell::Cell>::observe_venue_account` | Returns `Result<()>`. | `backend/crates/edge/qip-edge/src/cell.rs`, `pub fn observe_venue_account` |
| `<qip_compliance::signing::SigningKey>::from_secret` | A `SigningKey` handle. The secret is private, there is no accessor, and the hand-written `Debug` prints `secret: "<redacted>"`; there is no `Display`. The key the kernel builds with it is derived from the run seed (`central_signing_secret` in `platform.rs`, "not a production key"). | `backend/crates/libs/qip-compliance/src/signing.rs`, the struct doc and `impl std::fmt::Debug for SigningKey` |
| `<qip_brokers::adapter::VenueAdapter>::authenticate` and the `RestOrderEntryAdapter` impl | Returns `Result<()>`. The session secret is read inside through `Secret::expose_str` straight into the request header and is never returned. `Secret` redacts both `Debug` and `Display` and does not implement `Serialize`. The trait row is needed because `bring_up`, the trait's default method, calls `self.authenticate(..)`, which resolves to the declaration. | `backend/crates/services/qip-brokers/src/adapter.rs` (`fn authenticate`, `fn bring_up`), `rest.rs` (`fn authenticate`), `credential.rs` (`impl fmt::Debug for Secret`, `impl fmt::Display for Secret`) |
| `qip_storage::gcp::workload_certificate::workload_certificate` | A `CasIdentity`: an `Arc<WorkloadIdentity>`, a `RenewalStatus`, a stop flag and a thread handle. `WorkloadIdentity`'s hand-written `Debug` prints validity, the root count and `key: "<redacted>"`. The downstream prints are the shipper's destination, producer, stream and partition. | `backend/crates/libs/qip-transport/src/cas.rs` (`pub struct CasIdentity`), `backend/crates/libs/qip-tls/src/lib.rs` (`impl std::fmt::Debug for WorkloadIdentity`) |
| `qip_fastbrain::config::live_feed` | `Option<LiveFeedSettings>`, whose `api_key` is a `qip_brokers::Secret` since 2026-10-18. Before that it was a plain `String` inside two `Debug` structs, and this row would have been a lie; the test `the_resolved_credential_is_redacted_from_the_config_s_debug_rendering` in `config.rs` holds the claim. | `backend/crates/apps/qip-fastbrain/src/config.rs` (`pub api_key: Secret`), `feed.rs` (the one `expose_str()`) |
| `<qip_edge_node::scheduler::DecisionLoop>::with_certificate` | The `DecisionLoop` itself, returned by value with an `Arc<RenewalStatus>` stored. The name matches `cert`, so the whole loop is a source and every field read on it is tainted: the `eprintln!`s in `scheduler.rs` that print the journal's `retained()` count, the peer-offer counter and the rollback's `from`/`to` sequences, and the `held_control.push_back` a generated sink row calls logging, were charted as a certificate logged in clear (main's ci run 1052 blocked on `scheduler.rs:630`), and so were four in `event_fabric/inputs.rs`. Every field type of the loop was walked, because it derives `Debug`: `Cell` and its config hold no key, token or credential; `NodeGateway`'s `SimulatedGateway` holds an exchange, a venue, reports and orders, and its `RestGateway` holds a `RestOrderEntryAdapter` whose hand-written `Debug` prints `credential` as `<redacted>` over a `Secret` that redacts `Debug` and `Display`; `SimulatedFeed`'s `FeedKey` is a venue and a channel name; `MeshWorker`, `ReflexPort`, `JournalRelay`, `DrainThread` and the `Outbox` hold channels, atomics, counters, thresholds and thread handles; `RenewalStatus` holds a `WorkloadIdentity` whose `Debug` prints `key: "<redacted>"`, two counters and the last renewal error the health body already publishes. The one plain-text identifier reachable is the REST adapter's venue account *name*, which is not a credential, and nothing in the node formats a `DecisionLoop`. Reproduced with CodeQL 2.27.2 and `codeql/rust-queries` 0.1.44 on 1fa6b4f8: 154 `rust/cleartext-logging` results without the row, 146 with it, the eight removed being exactly the four `scheduler.rs` and four `event_fabric/inputs.rs` sinks, all with `main.rs`'s `decisions.with_certificate(status)` as their source. | `backend/crates/apps/qip-edge-node/src/scheduler.rs` (`pub struct DecisionLoop`, `pub fn with_certificate`), `main.rs` (`decisions.with_certificate(status)`), `gateway.rs` (`NodeGateway`), `backend/crates/services/qip-brokers/src/rest.rs` (`impl std::fmt::Debug for RestOrderEntryAdapter`), `credential.rs` (`impl fmt::Debug for Secret`, `impl fmt::Display for Secret`), `backend/crates/libs/qip-transport/src/cas.rs` (`pub struct RenewalStatus`), `backend/crates/libs/qip-tls/src/lib.rs` (`impl std::fmt::Debug for WorkloadIdentity`) |

## Verifying a change to this pack

With the bundle the workflow uses (`codeql-bundle-v2.27.2`) and a database of
the tree (`codeql database create <db> --language=rust --source-root=<checkout>`):

```
codeql database analyze <db> codeql/rust-queries:codeql-suites/rust-security-and-quality.qls \
  --additional-packs=<checkout>/.github/codeql/qip-models --model-packs=qip/rust-models \
  --format=sarif-latest --output=out/results.sarif --threads=4 --ram=11000
node scripts/sast-gate.mjs --policy scripts/sast-policy.json out
```

The last line of the gate is the evidence. Canonical paths for a new row come
from the database, not from reading the code:

```
codeql query run --database=<db> --additional-packs=<bundle>/qlpacks -- <a .ql selecting Function.getCanonicalPath()>
```

A default heap (about 2 GB) made the Rust analysis page for over half an hour
on this tree; `--ram=11000` finished in fifteen minutes.

## What would make this wrong

A row whose claim stops being true: a `Display` added to `SigningKey`, an
accessor on `Secret` that is not conspicuously named, `api_key` going back to a
`String`. Each row names the file that holds its claim, so a reviewer of a
change to that file can find the row that depends on it with
`grep -rn <type> .github/codeql/qip-models/`.
