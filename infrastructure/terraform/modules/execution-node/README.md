# The execution node

The Algorik blueprint's §41.2 and §41.4 describe one workload that is not a
container: a single Rust binary on a dedicated C3 or C3D, under systemd, with no
container runtime, no external address, and the cores above 1 isolated. ADR 0022
made the blueprint the architecture of record and ADR 0020 sequences the
migration. This module is the Terraform for that machine.

**Nothing calls it.** `infrastructure/terraform/main.tf` has no `module
"execution_node"` block and must not acquire one here. ADR 0020 step 3 —
standing a node up in shadow mode — requires recorded human approval naming that
step, and evidence is what earns the conversation, never the authorisation. A
module that exists is not a node that exists, and wiring it is its own change
with its own plan.

## What it provisions

| | |
|---|---|
| Subnet | Its own range in the node's region, private Google access on, flow logs sampled |
| Identity | One service account, federated through the metadata server. **No key, and none may ever be created** |
| Grants | Secret Manager accessor on the capital envelope key; the venue credential only under the two conditions below; `monitoring.metricWriter` and `logging.logWriter`; object *creation* on the evidence bucket when one is named. No Artifact Registry role — there is no runtime to pull an image |
| Machine | Instance template: C3/C3D high-CPU, gVNIC, `TIER_1` egress, compact placement, shielded (secure boot, vTPM, integrity monitoring), **no `access_config`, so no external address** |
| Supervision | A startup script that verifies the image and installs one systemd unit with `Restart=always` |
| Replacement | A zonal managed instance group whose update policy surges one, waits for the health check, then retires the old machine |
| Network | Deny-all egress at priority 65000, with named allows for Google APIs, the egress proxy, the central plane, and — only outside shadow mode — one rule per venue |

## Shadow mode is structural, and it is the default

`shadow_mode = true` is not a flag the binary reads and could ignore. With it on
this module creates **no venue egress rule and no venue-credential binding**, so
the node cannot open a venue session at all. Turning it off is what creates the
route, which makes taking venue sessions a reviewed diff rather than a value
somebody edits in a config map.

The venue credential needs three things to be readable: an environment whose
autonomy ceiling permits live trading (the root computes one predicate and hands
the same value to this module and to `modules/secrets`), `shadow_mode = false`,
and a secret actually named. The first of those is unsatisfiable today, because
the plan-time refusal in `infrastructure/terraform/variables.tf` rejects every
ceiling that permits live trading — so no environment that can be applied binds
this credential at all. That is the intended state, not a gap. The module
**does not accept an autonomy ceiling**. The ceiling is decided in three
places already and a fourth would weaken all three.

## What this module cannot enforce

This is the section to read before believing anything above.

### Kernel settings belong to the image

Terraform cannot set a kernel parameter. `isolcpus`, the preallocated huge
pages, the absence of swap and the absence of a container runtime are properties
of the image named in `boot_image`. What the startup script does is **verify**
them at boot and refuse to start the unit when they are missing — so this
configuration does not make a kernel isolate anything; it makes a machine that
did not isolate anything decline to trade.

| §41.4 setting | Who enforces it | What happens if it is absent |
|---|---|---|
| `isolcpus` for cores 2 upwards | The image's kernel command line | Startup script refuses; unit never starts |
| Huge pages preallocated | The image's kernel command line | Startup script refuses below `required_hugepages_gb` |
| No swap | The image | Startup script refuses |
| No container runtime | The image | Startup script refuses if `docker`/`containerd`/`podman`/`crio`/`runc` is present |
| `mlockall` | The binary, with `CAP_IPC_LOCK` and `LimitMEMLOCK=infinity` from the unit | The call fails and the binary decides |
| Governor `performance`, C-states disabled | **Nobody here.** On Compute Engine these are host and image properties and the guest usually has no authoritative `cpufreq` at all | The script logs what it found and continues. This module does not enforce it and nobody should say it does |
| systemd watchdog | Off by default | See below |

### The watchdog is off by default, and that is honest rather than cautious

§41.4 asks for a watchdog. A watchdog is a contract: systemd expects
`sd_notify(WATCHDOG=1)` within the interval and kills the process when it does
not arrive. `qip-edge-node` sends no such notification, so setting
`watchdog_seconds` against today's binary produces a kill and a restart every
interval — a supervision setting that turns a healthy process into a crash loop
while the console shows a unit systemd is diligently restarting. Set it when the
image ships a binary that pings. Until then the module provides `Restart=always`,
which is the half of that line the current binary can honour.

### Binary Authorization has no analogue on a bare VM

The repository requires Binary Authorization on every deployed image. That
enforcement is `evaluation_mode = "PROJECT_SINGLETON_POLICY_ENFORCE"` in
`modules/cluster/main.tf` — a **GKE admission controller**. Cloud Run has an
equivalent. A bare Compute Engine instance has none, because §41.4's whole point
is that nothing sits between the binary and the kernel, and admission control is
something that sits in between.

What survives the topology change is the *signing* half: the attestation chain in
`.github/workflows/deploy.yml` is registry-side and topology-agnostic. What has
no VM analogue is the *admission* half.

So, plainly:

- **Enforced here**: the image is pinned to one self-link, never a family (a
  `validation` refuses `/family/`); secure boot, vTPM and integrity monitoring
  are on.
- **Not enforced here, and contracted on the image build**: that the image was
  built from a signed, attested artefact, and that it contains
  `/usr/local/bin/qip-edge-node` and `/usr/local/bin/qip-fetch-secret` and
  nothing that could pull a second one.

Nobody should read this module as satisfying the Binary Authorization rule. It
narrows the gap and names the rest.

### The unbuilt prerequisite: there is no non-Kubernetes egress proxy

**This is a blocker for the module to function at all, not a caveat.**

The dependency policy permits `serde` and `serde_json`, so
`qip_transport::http` has no TLS stack and refuses the `https` scheme by name
rather than downgrading it (`backend/crates/libs/qip-transport/src/http.rs`:
"Refuses `https` by name"). Every outbound adapter in the workspace therefore
needs an `http://host:port` that terminates TLS on its behalf, and it must be a
**reverse** proxy: `HttpRequest::encode` emits an origin-form request line and
never `CONNECT`, so the destination is chosen by which listener the client
connects to and cannot be named in the request.

The only implementation in this repository is
`infrastructure/kubernetes/base/egress.yaml`, an Envoy Deployment, asserted by
`qip-acceptance/tests/egress.rs`. **Nothing here provides a non-Kubernetes
equivalent**, so a node on bare GCE has no outbound HTTPS until one is built.

This module does not solve that — adding TLS would mean a crypto dependency and
an ADR, and neither belongs in an infrastructure module. What it does instead:
`egress_proxy` is **required with no default**, so a deployment must say where
the proxy is or the plan refuses. A default pointing at the in-cluster service
would resolve to nothing on a machine with no cluster DNS, and the node would
fail at its first vendor call instead of at plan time.

Recorded here because a decommission sequence that retires the Helm chart
(ADR 0020 step 5) while the egress proxy is only a Kubernetes object retires the
outbound path of every service that migrated.

## Wiring it, when that is approved

The wiring pass is a separate change with its own plan. It adds to
`infrastructure/terraform/main.tf`, after `module.secrets`, `module.network` and
`module.evidence`:

```hcl
module "execution_node" {
  source   = "./modules/execution-node"
  for_each = var.execution_nodes

  depends_on = [module.services]

  project_id  = var.project_id
  environment = var.environment
  labels      = local.labels

  node_id = each.key
  region  = each.value.region
  zone    = each.value.zone

  network_id  = module.network.network_id
  subnet_cidr = each.value.subnet_cidr

  machine_type = each.value.machine_type
  boot_image   = each.value.boot_image

  # Observed before it takes anything. ADR 0020 step 3.
  shadow_mode = true

  venues               = each.value.venues
  central_plane_ranges = local.central_plane_ranges
  egress_proxy         = each.value.egress_proxy

  capital_envelope_secret_id = module.secrets.secret_ids["qip-capital-envelope-key"]
  venue_credential_secret_id = module.secrets.secret_ids["qip-venue-credential"]

  # The root's own predicate, passed through unchanged. This module then
  # requires shadow mode to be off as well.
  venue_credential_readable = contains(["supervised_live", "limited_autonomous_live", "autonomous_live"], var.autonomy_ceiling)

  evidence_bucket = module.evidence.bucket_name
}
```

Copy that predicate as it stands. It is a membership test over the three live
rungs rather than the shorter-looking `!= "paper_trading"` because the ceiling
has six values, not two: `variables.tf` refuses the three live ones at plan
time, so the reachable set is `{observation, advisory, paper_trading}` and the
negation is true for exactly the two rungs *below* paper trading — lowering the
ceiling would have granted the venue credential instead of withholding it. That
inversion shipped once, in this very block. The membership test names the
property the security rules state — the credential is readable only where the
ceiling could use it — instead of a complement that happens to agree with it in
some configurations, and it is false in every configuration a plan can carry.
Do not simplify it back.

It also needs a root `execution_nodes` variable — an empty map by default, so
that the wiring alone provisions nothing — and a per-environment entry in
`infrastructure/environments/<env>/terraform.tfvars` before any machine exists.
Each of those is a plan somebody reads.
