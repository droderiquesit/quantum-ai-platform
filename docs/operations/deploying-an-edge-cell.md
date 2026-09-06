# Deploying a new edge cell

A cell is one entry in `execution_nodes` in the environment's
`infrastructure/environments/<env>/terraform.tfvars`
(`infrastructure/terraform/variables.tf:227-275`), provisioned by
`infrastructure/terraform/modules/execution-node` as one Compute Engine
machine running `qip-edge-node` bare under systemd, in shadow mode by a
literal in `infrastructure/terraform/main.tf:492` that no tfvars value can
turn off ([ADR 0024](../adr/0024-the-blueprint-runtime-is-provisioned-in-code-and-the-gitops-runtime-is-retired.md)).
The node id and the cell id are the same string
(`modules/execution-node/variables.tf:9-25`).

**Before you start:** a cell trades on its own authority, inside a capital
envelope granted in advance. Bringing one up wrong does not fail loudly — it
produces a cell that either cannot reach its venue, or can reach more than its
venue. Both are quiet.

Nothing in this runbook has been executed against a real project. It is written
from the configuration in `infrastructure/`, which has never been applied, and
the first person to follow it should expect to correct it. Two facts bound what
it can promise:

* `execution_nodes = {}` in every environment
  (`environments/{dev,test,stage,prod}/terraform.tfvars`). No node exists
  anywhere, because a node needs at least one venue and no venue's published
  ranges are recorded in this repository.
* Nothing in this repository builds the boot image
  (`modules/execution-node/README.md`, "No image bake exists"). `deploy.yml`
  builds and attests a container image of `qip-edge-node`; turning that into a
  Compute Engine image with the kernel command line the module verifies is a
  build step nobody has written. Until it exists, step 4 names an image
  somebody built by hand.
* A node this module boots **runs passes**. The startup script writes
  `QIP_VENUE_FEED=simulated` (`startup.sh.tftpl:174`), the one value
  `qip-edge-node` accepts — any other stops the process naming ADR 0003
  (`backend/crates/apps/qip-edge-node/src/feed.rs:118-131`) — and the loop
  runs `Cell::work` over an in-process simulated venue's own depth, never
  over the venues in step 5 (`6340610`). Proven in
  `backend/crates/apps/qip-edge-node/tests/pass.rs`; run by no deployed
  node, because of the first bullet.

## Do this

1. **Pick the node id and the region.** The id goes in three places and must
   be the same string in all of them: the `execution_nodes` key, which the
   startup script writes as `QIP_CELL_ID`
   (`modules/execution-node/templates/startup.sh.tftpl:148`), and the `cell`
   field of every `CapitalEnvelope` the central plane grants there. A cell
   refuses an envelope addressed elsewhere, so a mismatch is a cell that starts
   and then rejects every grant it is sent. The region is chosen for distance to
   the venue (`variables.tf:27-30`); the zone must be in it, and the module
   refuses one that is not (`variables.tf:45-48`). The group is zonal on
   purpose — a compact placement policy is a claim about one zone
   (`variables.tf:32-43`).

2. **Allocate its subnet.** One range, in the node's region
   (`modules/execution-node/main.tf:109-130`). It must overlap neither another
   node's range nor any trust zone's: overlapping ranges route to whichever
   subnet was created first, silently (`variables.tf:243-245`), and the root
   refuses two nodes sharing a range (`variables.tf:271-274`). There is no pod
   or service range any more; the node is one machine with one address on one
   subnet and no external address (`main.tf:333-345` — no `access_config`).

3. **Choose the machine.** One of `c3-highcpu-8`, `c3-highcpu-22`,
   `c3d-highcpu-8`, `c3d-highcpu-16`; the module refuses anything else
   (`modules/execution-node/variables.tf:90-98`). The isolated core range is
   derived from the shape (`main.tf:80-87`): on an 8-vCPU machine it is `2-7`
   and the blueprint's §41.3 thread assignment does not fit, which
   `terraform output execution_nodes` reports as `isolated_cpus` so a
   deployment can see which of the two it got.

4. **Name the boot image by self-link.** Never a family — the module refuses
   `/family/` (`variables.tf:120-123`). The image is the other half of the
   node: `isolcpus`, huge pages, no swap, no container runtime,
   `/usr/local/bin/envoy`, `/usr/local/bin/qip-fetch-secret` and the Ops Agent
   all belong to it, and the startup script **verifies** each at boot and
   refuses to start the units when one is missing
   (`modules/execution-node/README.md`, "What this module cannot enforce").
   There is no admission controller on a bare machine, so this is what stands
   between a hand-built image and a trading node.

5. **Write the venues map from the venue's own connectivity documentation.**
   It may not be empty — `qip-edge-node` refuses an empty `QIP_VENUES` and the
   template's precondition refuses the plan first
   (`main.tf:308-311`) — and it may not be `0.0.0.0/0`
   (`variables.tf:189-194`). Do not guess a range: a wrong one produces a cell
   that cannot trade, and a wide one produces a cell that can reach the
   internet. In shadow mode the node is *configured* for these venues and can
   *reach* none of them: no venue firewall rule is created at all while
   `shadow_mode` is true (`main.tf:551-552`). The passes the node runs in the meantime are priced
   off the in-process simulator, not off any of these venues.

6. **Set `region_allocation`.** The ceiling on the capital this node may hold
   in reservation across all of its strategies, as a positive decimal string.
   Since ADR 0039's second slice the node opens *unfunded* under this ceiling
   and funds its region table only from the share the centre ships it in the
   signed `capital_grants` slot; the ceiling bounds that share, it does not
   fund anything by itself. It is required
   per entry with no default anywhere, and the module refuses an empty or zero
   value at plan time (`modules/execution-node/variables.tf`,
   `region_allocation`). The startup script writes it as
   `QIP_REGION_ALLOCATION` and `qip-edge-node` refuses to start without it:
   under ADR 0008 a cell that has lost the centre must still refuse its own
   second proposal, and it can only do that against an allocation it was
   given at assembly — so a node without one is a cell in which two
   strategies can each spend the whole envelope. Zero is refused because
   "send nothing" is what the halt flag is for; a node that can reserve
   nothing should be halted, not started. Choose it below the sum of the
   envelopes the centre will grant the cell — the allocation binds first.

7. **Set `create_egress_nat`.** True only when the node's region has no NAT of
   its own; a second NAT over the same subnetworks in one region is an apply
   error, not a redundancy (`variables.tf:427-446`). The primary region already
   has one from `modules/network`.

8. **Add the entry.** The shape is in
   `modules/execution-node/README.md`, "The tfvars entry". Leave everything
   about shadow mode alone: it is not a field.

9. **Plan, and read the plan.** `infra.yml`'s `plan` action is a manual
   dispatch and refuses `prod` (`.github/workflows/infra.yml:43,83-85`);
   `up` prints the plan before applying. What a correct plan for a new node
   contains, all from `modules/execution-node/main.tf`: the subnet; one
   service account with no key (`:174-195`); `secretmanager.secretAccessor` on
   the capital-envelope key (`:200-205`); `monitoring.metricWriter` and
   `logging.logWriter` (`:223-233`); `storage.objectCreator` on the evidence
   bucket (`:238-244`); a placement policy, a health check, an instance
   template and a managed instance group of one (`:260-465`); and firewall
   rules named `deny-egress`, `google-apis`, `central-plane`, `health-checks`
   and `deny-ingress` (`:472-621`).

   What it must **not** contain: any rule named `…-venue-…`, and any
   `google_secret_manager_secret_iam_member` on `qip-venue-credential`. Either
   means `shadow_mode` at `main.tf:492` has been edited or the ceiling refusal
   in `variables.tf:105-116` has been removed. Stop and ask.

10. **Apply — a person's decision.** ADR 0024 authorises the code and not an
   apply; the first plan against a project that ran the old runtime will
   propose destroying it, and whoever reads that plan decides.

11. **Confirm the tag is carried.** Every rule constraining the node targets
    the tag in `terraform output execution_nodes` (`node_tag`), and a rule
    targeting a tag nothing carries does nothing, silently. Check with
    `gcloud compute instances list --filter="tags.items=<tag>"`; an empty
    result means the deny-all egress rule constrains nothing.

12. **Write the verification key.** The `qip-capital-envelope-key` secret is
    created empty by Terraform (`infrastructure/terraform/main.tf:201-204`),
    because a value in Terraform is a value in the state file. Write the
    version out of band. The startup script fetches it into the unit's run
    directory and `qip-edge-node` refuses to start without it
    (`modules/execution-node/main.tf:197-199`). Its confidentiality matters
    less than its integrity: whoever can replace it can mint envelopes.

13. **Confirm the node can reach nothing yet.** The group judges the instance
    by `GET /health` on port 8080 (`main.tf:282-285`) after a five-minute
    initial delay (`:456`), and the startup script's last line reads
    `started in SHADOW mode` (`startup.sh.tftpl:292`) — read it over an
    OS Login session, which is the only shell the node permits (`main.tf:390-392`;
    the serial console is off). The state to confirm: the instance is healthy,
    `terraform output execution_nodes` shows `shadow_mode = true` and
    `venue_credential_bound = false`, and
    `gcloud compute firewall-rules list --filter="name~^qip-<env>-exec-<id>-venue-"`
    is empty. Running, connected to nothing, holding no envelope. That is the
    state to be in before granting any capital.

14. **Attach the journal snapshot schedule.** Terraform creates the schedule
    and cannot attach it, because the disk is created by the group after any
    apply. Run `terraform -chdir=infrastructure/terraform output -raw journal_snapshot_attachment_command`
    now and again after every replacement; until then the journal is covered
    by nothing (`modules/backup/NOT-COVERED.md`). See
    [disaster recovery](disaster-recovery.md).

15. **Know what is still not connected.** The centre-to-node path is unwired
    on this runtime: `QIP_MESH_PEER` is not set on the node and
    `QIP_MESH_CELLS` is not set on the API (ADR 0024, "What it costs";
    `infrastructure/terraform/catalogue.tf:21-27`), so the node starts
    detached and no envelope, policy or kill-switch scope reaches it. Granting
    capital through the central plane's approval path is the last step of this
    runbook in principle, and today there is no path to deliver the grant.
    The node's Ops Agent receiver scrapes its health port
    (`startup.sh.tftpl:178-196`), but `workload_metrics_exist` stays false in
    every environment until a scrape has been observed, so no alert exists for
    it yet.

## Taking a node out of shadow mode

Not a tfvars value. `shadow_mode = true` is a literal at
`infrastructure/terraform/main.tf:492`, and letting a node out is an edit
there that a reviewer sees. Turning it off is what creates the one-rule-per-
venue egress (`modules/execution-node/main.tf:541-572`). The venue credential
binding additionally requires an autonomy ceiling that permits live trading
(`main.tf:100-104`), which `infrastructure/terraform/variables.tf:105-116`
refuses at plan time for every environment — so a node out of shadow mode can
reach a venue's address and still cannot authenticate to it. ADR 0020 step 3
is the ordering: observed before it takes anything.

## To take a cell out

Remove its entry from `execution_nodes` and apply. The subnet, the service
account, the group and the firewall rules go with it, which is the reason a
node's identity lives in its own module rather than in a shared map: an
account left behind by a removed cell is a credential nobody owns.
`infra.yml`'s `down` action destroys `module.execution_node` alone and refuses
an untargeted destroy (`.github/workflows/infra.yml:19,223`).

The journal disk is `auto_delete = true` (`main.tf:319-331`): it goes with the
instance. Its snapshots do not — the schedule keeps them on disk deletion
(`modules/backup/main.tf:89`) — but only if step 14 was run for that disk.

Let the envelopes expire rather than revoking them, if you can. Every envelope
expires by construction, and a cell that stops when its grant runs out stops
cleanly.

## The seven locations

ADR 0008 calls for at least seven cells; nine names are reserved. Adding one is
one entry in the map above — no new module, no new directory.

| cell id | location | region | note |
| --- | --- | --- | --- |
| `dallas-1` | Dallas | `us-south1` | in the metro |
| `chicago-1` | Chicago | `us-central1` | **not in the metro** — Council Bluffs, Iowa |
| `newyork-1` | NY/NJ | `us-east4` | **not in the metro** — Ashburn, Virginia |
| `london-1` | London | `europe-west2` | in the metro |
| `frankfurt-1` | Frankfurt | `europe-west3` | in the metro |
| `singapore-1` | Singapore | `asia-southeast1` | in the metro |
| `tokyo-1` | Tokyo | `asia-northeast1` | in the metro |
| `saopaulo-1` | São Paulo | `southamerica-east1` | in the metro — B3 |
| `dubai-1` | Dubai | `me-central1` | **not in the metro** — Doha, Qatar |

Three of the nine are a problem worth reading before building on this table.

Google Cloud has no region in Chicago, none in the New York/New Jersey
metropolitan area, and none in Dubai. The nearest regions are roughly 400, 300
and 380 kilometres away respectively, which is several milliseconds of round
trip that a cell whose whole argument is source-adjacency cannot spend. A cell
in `us-central1` is not next to CME any more than the central plane is.

That is an architectural gap rather than a configuration one, and it has three
honest answers: colocation with a partner interconnect back to the VPC
(`modules/connectivity`, off everywhere, and a circuit nobody has ordered),
running those cells somewhere other than Google Cloud, or accepting that the
two American equity and futures cells are not latency-competitive and saying
so. ADR 0008 already names the condition under which the whole cell
architecture should be collapsed back into the central plane, and this is
evidence for that question rather than against it.

## The cell's journal lives on the boot disk

The startup script sets `QIP_STORAGE_TARGET=engine` and
`QIP_STORAGE_ROOT=/var/lib/qip/journal` (`startup.sh.tftpl:152-153`) and
creates that directory owned by the node's user (`:215`). The journal is on
the instance's 100 GB boot disk, labelled `qip_journal=true` so the snapshot
schedule can find it (`main.tf:326-330`), and that disk is deleted with the
instance. A replacement under the group's policy is a new machine with a new
disk — so a cell's hash-chained decision record outlives its machine only as
snapshots, and only from the moment the schedule was attached. That is the
intended friction, and it is weaker than the namespace-scoped backup the old
runtime had; `modules/backup/NOT-COVERED.md` says so in as many words.

`QIP_MIRROR_PATH` used to select the journal's destination on its own. It is no
longer read, and the node **refuses to start** if it is set rather than
ignoring it (`backend/crates/apps/qip-edge-node/src/main.rs:134-146`) — a cell
deployed with the old variable would otherwise write its journal nowhere while
its configuration still claimed a path. The refusal names the two replacements.

Book and feature state is in memory, deliberately. A cell rebuilds its books
from the feed on start, and state that survived a restart would be state
nobody reconciled against the venue — the same reasoning that makes a stale
book serve no price.

## What this runbook does not cover

* **The boot image.** Nothing builds it; see above.
* **The venue decision.** The ranges in step 5 come from the venue, and no
  venue has been chosen for any environment.
* **A venue feed.** `QIP_VENUE_FEED` has one value, `simulated`. A feed from
  a market is an architecture decision (ADR 0003), not a configuration line,
  and this runbook does not describe one.
* **The centre-to-node path.** Step 15. The blueprint's control fabric is
  Pub/Sub, and building it is work ADR 0024 names and does not do.
* **A collector or an alert for the node.** The receiver is declared; nothing
  has been scraped. See `.claude/rules/domains/observability.md`.

See also [external dependencies](external-dependencies.md) — the standing
list; note that its edge-cell section still describes the retired runtime's
`modules/edge-cell` and is not corrected here.
