# The execution node

The Algorik blueprint's §41.2 and §41.4 describe one workload that is not a
container: a single Rust binary on a dedicated C3 or C3D, under systemd, with no
container runtime, no external address, and the cores above 1 isolated. ADR 0022
made the blueprint the architecture of record, ADR 0020 sequences the node's
arrival, and ADR 0024 is the owner's instruction to provision the runtime in
code. This module is the Terraform for that machine.

**Wired, and provisioning nothing.** `infrastructure/terraform/main.tf`
instantiates this module once per entry in `execution_nodes`, and every
environment's tfvars leaves that map empty. A node must be configured for at
least one venue — `qip-edge-node` refuses an empty `QIP_VENUES` and the
template's precondition refuses the plan first — and no venue's published
address ranges have been recorded anywhere in this repository. The first entry
is therefore a venue decision, not a Terraform one, and the plan that carries it
is the evidence ADR 0020's step 3 asks for.

## What it provisions

| | |
|---|---|
| Subnet | Its own range in the node's region, private Google access on, flow logs sampled |
| Identity | One service account, federated through the metadata server. **No key, and none may ever be created** |
| Grants | Secret Manager accessor on the capital envelope key; the venue credential only under the two conditions below; `monitoring.metricWriter` and `logging.logWriter`; object *creation* on the evidence bucket when one is named. No Artifact Registry role — there is no runtime to pull an image |
| Machine | Instance template: C3/C3D high-CPU, gVNIC, `TIER_1` egress, compact placement, shielded (secure boot, vTPM, integrity monitoring), **no `access_config`, so no external address**. The disk is labelled `qip_journal=true` for `modules/backup` |
| Supervision | A startup script that verifies the image and installs two systemd units with `Restart=always`: the egress proxy, and the node that requires it |
| Egress proxy | `qip-egress.service`: a static Envoy on loopback running the one committed bootstrap (`infrastructure/egress/envoy.yaml`), the same file every Cloud Run sidecar mounts. The binary's adapters are configured with `http://127.0.0.1:910x` |
| Telemetry | The Ops Agent's Prometheus receiver scrapes the health port on loopback every 30 s, carrying every `qip_edge_*` series to Cloud Monitoring, which is what the edge alert policies in `modules/observability` query |
| Replacement | A zonal managed instance group whose update policy surges one, waits for the health check, then retires the old machine |
| Network | Deny-all egress at priority 65000, with named allows for Google APIs, the central plane, and — only outside shadow mode — one rule per venue. No rule for a proxy: there is no address to permit |

## Shadow mode is structural, and it is the default

`shadow_mode = true` is not a flag the binary reads and could ignore. With it on
this module creates **no venue egress rule and no venue-credential binding**, so
the node cannot open a venue session at all. Turning it off is what creates the
route, which makes taking venue sessions a reviewed diff rather than a value
somebody edits in a config file.

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

### Kernel settings and the two extra binaries belong to the image

Terraform cannot set a kernel parameter. `isolcpus`, the preallocated huge
pages, the absence of swap and the absence of a container runtime are properties
of the image named in `boot_image`. What the startup script does is **verify**
them at boot and refuse to start the units when they are missing — so this
configuration does not make a kernel isolate anything; it makes a machine that
did not isolate anything decline to trade.

| §41.4 setting | Who enforces it | What happens if it is absent |
|---|---|---|
| `isolcpus` for cores 2 upwards | The image's kernel command line | Startup script refuses; no unit starts |
| Huge pages preallocated | The image's kernel command line | Startup script refuses below `required_hugepages_gb` |
| No swap | The image | Startup script refuses |
| No container runtime | The image | Startup script refuses if `docker`/`containerd`/`podman`/`crio`/`runc` is present |
| `/usr/local/bin/envoy` and `/usr/local/bin/qip-fetch-secret` | The image | Startup script refuses; a node with no proxy has no outbound HTTPS |
| `google-cloud-ops-agent.service` | The image | Startup script refuses; a node nothing scrapes is a node nobody can operate |
| `mlockall` | The binary, with `CAP_IPC_LOCK` and `LimitMEMLOCK=infinity` from the unit | The call fails and the binary decides |
| Governor `performance`, C-states disabled | **Nobody here.** On Compute Engine these are host and image properties and the guest usually has no authoritative `cpufreq` at all | The script logs what it found and continues. This module does not enforce it and nobody should say it does |
| systemd watchdog | Off by default | See below |

### The watchdog is off by default, and that is honest rather than cautious

§41.4 asks for a watchdog. A watchdog is a contract: systemd expects
`sd_notify(WATCHDOG=1)` within the interval and kills the process when it does
not arrive. `qip-edge-node` sends no such notification, so setting
`watchdog_seconds` against today's binary produces a kill and a restart every
interval. Set it when the image ships a binary that pings. Until then the module
provides `Restart=always`, which is the half of that line the current binary can
honour.

### Binary Authorization has no analogue on a bare VM

The repository requires Binary Authorization on every deployed image, and every
Cloud Run service in the catalogue evaluates the project policy on every
revision. A bare Compute Engine instance has no admission controller, because
§41.4's whole point is that nothing sits between the binary and the kernel, and
admission control is something that sits in between.

What survives the topology change is the *signing* half: the attestation chain in
`.github/workflows/deploy.yml` is registry-side and topology-agnostic, and the
`qip-edge-node` image it builds, scans, signs and attests is the artefact the
boot image is contracted to be built from. What has no VM analogue is the
*admission* half.

So, plainly:

- **Enforced here**: the image is pinned to one self-link, never a family (a
  `validation` refuses `/family/`); secure boot, vTPM and integrity monitoring
  are on.
- **Not enforced here, and contracted on the image build**: that the image was
  built from the signed, attested `qip-edge-node` artefact, and that it contains
  `/usr/local/bin/qip-edge-node`, `/usr/local/bin/qip-fetch-secret`,
  `/usr/local/bin/envoy` (the digest in `infrastructure/egress/vendored-images.txt`,
  extracted from the mirrored image) and nothing that could pull a second one.

Nobody should read this module as satisfying the Binary Authorization rule. It
narrows the gap and names the rest.

### No image bake exists

Nothing in this repository builds the boot image. `deploy.yml` builds and
attests a container image of `qip-edge-node`; turning that, Envoy and the Ops
Agent into a Compute Engine image with the kernel command line above is a build
step that has not been written. Until it is, `boot_image` in a tfvars entry
names an image somebody built by hand, and the startup script's checks are the
only thing standing between a hand-built image and a trading node. ADR 0024
records this as remaining work.

## The tfvars entry

```hcl
execution_nodes = {
  "london-1" = {
    region       = "europe-west2"
    zone         = "europe-west2-a"
    subnet_cidr  = "10.68.0.0/20"
    machine_type = "c3-highcpu-8"
    boot_image   = "projects/<project>/global/images/qip-execution-node-<build>"
    venues = {
      "sim-1" = { cidr = "203.0.113.0/24", port = 443 }
    }
    create_egress_nat = true   # its region has no NAT of its own
  }
}
```

The root passes `shadow_mode = true` unconditionally: the first node is
observed before it takes anything, per ADR 0020 step 3, and letting a node out
of shadow mode is an edit to `main.tf` that a reviewer sees, not a tfvars value.
The venue-credential predicate the root passes is the membership test over the
three live rungs — `contains([...live rungs...], var.autonomy_ceiling)` — and
not `!= "paper_trading"`, which is true for exactly the two rungs *below* paper
trading and once granted the credential by lowering the ceiling. Do not
simplify it back.
