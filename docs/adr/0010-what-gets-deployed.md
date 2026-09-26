# 0010 — Four of the eight application crates are deployed, and the other four are not

**Status:** accepted

## Decision

`crates/apps` holds eight crates. Four of them are deployed as Kubernetes
workloads, and the other four are deliberately not. This record is the list,
so that "why is that one missing" has an answer somewhere other than a comment
in a build matrix.

| crate | binary | deployed | why |
| --- | --- | --- | --- |
| `qip-api` | `qip-api` | yes | the API and the operator interface |
| `qip-fastbrain` | `qip-fastbrain` | yes | market data, microstructure, real-time risk, execution |
| `qip-deepbrain` | `qip-deepbrain` | yes | world model, discovery, reasoning, simulation, learning |
| `qip-edge-node` | `qip-edge-node` | yes | one cell's hot path, applied per cell by a runbook rather than by the pipeline |
| `qip-cli` | `qip` | **no** | an operator's tool, run by a person |
| `qip-web` | *none* | **no** | not a binary — a library `qip-api` links and renders from |
| `qip-fabricd` | `qip-fabricd` | **no** | event fabric broker (ADR 0100); composition root unfinished, and GCP placement is undecided (ADR 0099 C8) |
| `qip-ledgerd` | `qip-ledgerd` | **no** | ledger writer (ADR 0100); composition root unfinished, and GCP placement is undecided (ADR 0099 C8) |

Deployed means all three of: an entry in the image matrix in
`.github/workflows/deploy.yml`, a manifest in
`infrastructure/kubernetes/base/`, and a service account Terraform creates. A
crate has all three or none; two out of three is the failure mode this record
and the tests behind it exist to prevent.

### `qip-cli` is not deployed

It builds a binary called `qip`, and `qip` is a tool a person runs against a
cluster — `qip status`, `qip governance`, `qip limits`. There is no loop for it
to run and no request for it to answer, so a Deployment would be a pod that
starts, does one thing and exits, restarting for ever.

The image is not built either. An image nobody deploys is a build that costs
money and ships nothing, and it reads to a reviewer as a component that is
deployed and constrained when it is neither.

An operator who wants the CLI against a cluster runs it locally against the
API, which is what its `--endpoint` handling is for. The day that stops being
enough — a cluster whose control plane an operator's workstation cannot
reach — the answer is a `Job` a person creates, not a `Deployment` that stands
there.

### `qip-web` is not a binary at all

This is the one worth stating clearly, because "six crates, four images" looks
like an oversight and is not.

`crates/apps/qip-web` declares no `[[bin]]` and has no `src/main.rs`. It is a
library: server-rendered HTML with no JavaScript, linked by `qip-api`, which
serves its pages on `qip-api`'s port alongside the JSON API. There is no
process to schedule, no port to bind and nothing to probe.

The consequences follow from that rather than from a preference:

* **No image.** The Dockerfile builds `cargo build --release --bin "$BINARY"`.
  A `qip-web` entry in the matrix would fail the build, because there is no
  binary target of that name.
* **No manifest.** A Deployment pulling an image that cannot be built is a
  rollout waiting for a tag that will never exist.
* **No service account.** `infrastructure/terraform/main.tf` says the same
  thing at its `service_accounts` map: an identity with nothing attached is
  permission nobody is watching.
* **One content-security policy, not two.** The header the pages are served
  under is `qip-api`'s — `default-src 'none'; style-src 'self'` and no script
  source at all — set in `qip-transport/src/server.rs`. A second deployment
  would be a second place for that policy to be set, and a second place for it
  to be set differently.

### `qip-fabricd` and `qip-ledgerd` are not deployed yet

ADR 0100 gives the event fabric broker and the ledger writer each their own
binary, argued under ADR 0091's own test: each is "a writer of the log or a
clock of its own" — `qip-fabricd` is the sole writer of every partition's
batch chain and runs its own segment-roll clock, and `qip-ledgerd` is the sole
writer of the ledger's double-entry chain. Both pass that test. Neither is
imaged or scheduled today, for a reason distinct from `qip-cli`'s and
`qip-web`'s: this is not a decision that either binary should never deploy,
only that it cannot yet.

Each binary's `main.rs` refuses to start and exits non-zero, naming ADR 0100,
because its composition-root modules — `qip-fabricd`'s `config`, `health` and
`archiver`; `qip-ledgerd`'s `config`, `consumer`, `read_api` and `store` — are
still doc-only stubs, each waiting on the packet named in its own module
comment. A binary that cannot read configuration, prove a store writable or
serve a request has no business in an image matrix or a catalogue entry: an
image built from it would ship a process that exits the instant Cloud Run
started it, and a catalogue entry would be a revision that never serves.
Beyond that, *where* either binary runs is undecided — tracked as ADR 0099's
conflict C8 — and `qip-ledgerd`'s own custody-separation argument in ADR 0100
§2 is part of what that decision has to weigh.

So both are on the exclusion lists `NOT_A_WORKLOAD` and
`NOT_IN_THE_IMAGE_MATRIX` in `infrastructure.rs`, with this section as the
reason. Excluding is the restrictive, reversible choice: the moment each
composition root lands and C8 resolves placement, the binary needs all three
of an image matrix entry, a catalogue or execution-node entry, and a service
account — the same bar `qip-fastbrain` and `qip-deepbrain` already clear — and
this record's own tests fail the day one of those three arrives without the
other two.

## Why

The failure this prevents is not a missing deployment. It is a **half** one: a
manifest for an image nobody pushes, an image nobody runs, an identity nothing
uses. Each of those is invisible in the same way — everything present reads
correctly, and the thing that is absent is absent from the review too. This
repository has already had one: an `allow-deepbrain-egress` NetworkPolicy
governing a pod no Deployment created, which read to anyone opening
`namespace.yaml` as a deep brain that was deployed and constrained.

So the correspondence is checked in every direction that can be wrong, in
`crates/tests/qip-acceptance/tests/infrastructure.rs`:

* every binary the workspace builds is in the image matrix, or on a named
  exclusion list carrying its reason;
* every image the matrix builds has a manifest that runs it;
* every manifest runs an image the matrix builds;
* every Deployment the pipeline applies is waited on by the rollout check, and
  nothing else is;
* every exclusion above is named in this record, and stops being an exclusion
  the moment its reason expires — `qip-web` gaining a `main.rs` fails the
  suite, rather than quietly shipping a library nobody serves.

A list with reasons is deliberately not a predicate. "Which crates are
excluded" is answerable by a filter; "why is this one" is not, and it is the
question the next person will actually have.

## What it costs

**Two things that would be convenient are unavailable.** The CLI cannot be
`kubectl exec`'d into — there is nothing to exec into in any case, since the
images are `FROM scratch` — and the web interface cannot be scaled, rolled or
rate-limited separately from the API. If the operator console ever becomes the
expensive half of `qip-api`'s load, the two are coupled and separating them is
a code change rather than a replica count.

**A sixth crate that looks like an application is not one.** `crates/apps` now
means "the top of the dependency graph" rather than "the things that deploy",
and a reader has to open a `Cargo.toml` to tell which. The table above is the
compensation for that, and the tests are what keep the table true.

**The exclusion list is a place to hide.** Adding an entry is easier than
adding a deployment, and a reason written to get a test to pass is still a
reason. Nothing prevents that except review — which is why the entries are
here, in a record someone reads on purpose, rather than only in a `const`.

## What would make this wrong

* **`qip-web` gains a serving loop of its own** — a reason to render pages in a
  process that is not the API, such as a console that must stay up while the
  API is being rolled. Then it needs all three: image, manifest, identity. The
  test suite fails the moment it declares a binary, which is the intended
  order.
* **The console's load stops being negligible next to the API's.** Sharing one
  Deployment shares one set of resource limits and one rollout. A console
  serving many operators, or rendering something expensive, is a reason to
  split.
* **An operator needs the CLI inside the cluster** because the control plane is
  unreachable from anywhere else. That argues for a `Job` and an image, and it
  would make `qip-cli`'s absence from the matrix wrong — but not its absence
  from the manifests, which is a separate decision.
* **`crates/apps` acquires a crate that is neither.** A third category —
  neither a deployed workload nor a library the API links — means this table
  has stopped describing the directory, and the directory should be split
  before the table grows a column.
* **Either `qip-fabricd`'s or `qip-ledgerd`'s composition root lands and ADR
  0099's C8 resolves placement.** Then that binary needs all three of an
  image matrix entry, a catalogue or execution-node entry, and a service
  account, and leaving it on `NOT_A_WORKLOAD` / `NOT_IN_THE_IMAGE_MATRIX` past
  that point is the exclusion list becoming a place to hide, which this
  record's own `## What it costs` already names as the risk to watch.
