# The execution node's boot image

`modules/execution-node` names an image in `boot_image`, refuses anything that
is not a full self-link, and **verifies** at every boot that the image is the
one §41.4 describes. It cannot create any of it: Terraform cannot set a kernel
parameter, and a startup script on a machine with no container runtime and no
route to the internet cannot fetch a binary.

Until this directory existed, nothing in this repository produced such an
image. `modules/execution-node/README.md` said so under "No image bake exists",
`environments/dev/terraform.tfvars` carried
`boot_image = "projects/algorik-dev/global/images/<the baked image>"` in a
comment because no real value could be written, and ADR 0035 — which decides
one node, `newyork-1`, `us-east4`, shadow mode, dev — records the missing image
as one of the two things blocking it.

This is the other half of that module. Three files, and one workflow:

| | |
|---|---|
| `.github/workflows/image.yml` | The bake. Manual dispatch, prod refused. Produces one image self-link and prints it. |
| `provision.sh` | Runs on the throwaway builder machine. Creates what `startup.sh.tftpl` verifies, then runs the same predicates itself. |
| `qip-fetch-secret` | The helper `startup.sh.tftpl` refuses to start any unit without. Reads one Secret Manager payload and writes it to a file. |
| `modules/image-bake` | The staging bucket, the builder's identity and the builder's subnet. Creates nothing unless an environment names a range. |

## What the image contains, and who says so

Derived from `modules/execution-node/templates/startup.sh.tftpl`, which is the
file that refuses a boot when one of these is missing. Not from this table:
`the_boot_image_bake_installs_exactly_what_the_node_refuses_to_start_without`
in the infrastructure acceptance suite reads both files and fails when they
disagree, so an entry added to the startup script and not to `provision.sh` is
a failing test rather than a node that will not come up.

| Path | Where it comes from | Chain of custody |
|---|---|---|
| `/usr/local/bin/qip-edge-node` | `crane export` of `qip-edge-node@<digest>` in the environment's registry | Built, scanned, pushed, signed and attested by `deploy.yml`. The bake **refuses** a digest the environment's attestor has not signed. |
| `/usr/local/bin/envoy` | `crane export` of `vendor/envoy@<digest>`, the digest read out of `infrastructure/egress/vendored-images.txt` | Mirrored and attested by `vendor.yml`. Same refusal. Same file `modules/egress-proxy` reads, so the proxy on the node and the proxy in every Cloud Run sidecar cannot fork. |
| `/usr/local/bin/qip-fetch-secret` | This directory | Reviewed in the diff that changed it, staged with a sha256 in the payload manifest. |
| `google-cloud-ops-agent.service` | A `.deb` pinned by version and sha256 in `image.yml` | Fetched by the runner, verified there, staged in the payload, verified again on the builder. |

Plus, from `provision.sh` and not from a package:

- a kernel command line carrying `isolcpus`, `nohz_full` and `rcu_nocbs` over
  the isolated range, and `default_hugepagesz=1G hugepagesz=1G hugepages=N`;
- no swap: stripped from `/etc/fstab`, `swapoff -a`, `swap.target` masked;
- no container runtime — **refused rather than removed**, because a base image
  carrying one is a base somebody chose wrongly and purging it silently would
  hide the choice;
- `/etc/qip/boot-image-manifest`, which records the commit, the run, the base
  image and every input digest, so an operator on a node can answer "which
  artefacts is this machine running" without the run that built it.

## Built from the signed artefact, never from source

There is no compiler in the bake and there must never be one. `image.yml`
resolves the container image `deploy.yml` already signed, refuses to continue
unless the attestor signed those exact bytes, and extracts the binary out of
it. A binary recompiled during the bake is a binary nothing signed, however
identical the source, and the attestation chain would end at the container
registry rather than at the machine.

Say the limit of that plainly. **There is no admission control on a bare VM.**
Every Cloud Run service in the catalogue evaluates the project's Binary
Authorization policy on every revision; a Compute Engine instance evaluates
nothing, because §41.4's whole point is that nothing sits between the binary
and the kernel and an admission controller is something that sits in between.
What this arrangement gives is a *signing* chain a person can check — the
inputs were attested, the bake refused to run without the attestations, and the
image records the digests. It is not admission control and nobody should
describe it as such. `modules/execution-node/README.md` says the same thing
from the other side.

## One image per machine shape

`modules/execution-node` derives the isolated range from the machine type —
§41.3 gives cores 0 and 1 to the OS, the telemetry drainer and the
control-plane client and isolates the rest, so a `c3-highcpu-8` is
`isolcpus=2-7` and a `c3d-highcpu-16` is `isolcpus=2-15`. That range is in the
image's kernel command line, and the node's startup script greps `/proc/cmdline`
for its own shape's range and refuses when it is absent.

So an image is valid for exactly one shape, its name says which, and a tfvars
entry whose `machine_type` disagrees with the image produces a machine that
boots and declines to trade. `image.yml` reads the permitted shapes out of
`modules/execution-node/variables.tf` rather than listing them again, and
derives the range with the module's own arithmetic.

## What the base image has to be

Supplied to the bake as a full self-link and validated by the same rule
`var.boot_image` applies — no family, no bare name, no partial URL. A family is
a moving pointer and there is no admission controller here to catch what it
moved to. Find one with:

```
gcloud compute images list --project debian-cloud \
  --filter='family=debian-12' --format='value(selfLink)'
```

`provision.sh` refuses a base that does not carry `python3` (the whole runtime
of `qip-fetch-secret`), `dpkg`, `update-grub`, `/etc/default/grub.d`, `curl`,
`sha256sum` or `tar` — or that carries a container runtime. Each is checked
before anything is installed, so a base chosen wrongly is named there rather
than discovered by a node that will not start.

The created image declares `UEFI_COMPATIBLE`, `GVNIC` and
`VIRTIO_SCSI_MULTIQUEUE` as guest OS features. This is not decoration:
`modules/execution-node` sets `nic_type = "GVNIC"` and a full shielded config,
and Compute Engine refuses an instance whose image declares neither. An image
baked without them plans cleanly and fails at apply, after the subnet, the
identity and four IAM bindings already exist — which is the failure the whole
arrangement is meant to move earlier.

## The builder reaches one object and nothing else

`modules/image-bake`'s subnet denies all egress except TCP 443 to the
restricted VIP, and the builder machine has no external address. Its identity
holds exactly one grant: `roles/storage.objectViewer` on the staging bucket.
So a provisioning script that grew a `curl https://…/install.sh` would fail to
connect rather than quietly pulling an unreviewed third party into the trusted
computing base of a trading machine. That is why the Ops Agent arrives as a
staged, pinned `.deb` rather than through Google's repository-adding script.

The payload object is named by its own sha256, and that is load-bearing rather
than tidy: `image.yml` authenticates as `qip-infra-<env>`, whose custom storage
role in `modules/cicd` carries `storage.objects.create` and deliberately not
`storage.objects.delete`, so nothing it runs can delete from the evidence
bucket. Overwriting a Cloud Storage object requires the delete permission, so a
bake that wrote a fixed object name would fail on its second run with a 403
naming a permission this account is never going to hold. The bucket's lifecycle
rule expires payloads, because nothing else can.

## What a bake still does not unblock

**Nothing here deploys a node, and `execution_nodes` stays `{}`.** ADR 0035
authorises one node and two things blocked it. This is one of them. The other
is `region_allocation` — the ceiling on the capital the cell may hold in
reservation. It has no default anywhere on purpose: a default is a number
nobody chose, and it is the one number in a cell's envelope a reviewer has to
have read. Nothing in this directory or that workflow may pick it.

The order, then: a person sets `image_bake_subnet_cidr` in the environment's
tfvars and dispatches `infra.yml up`; dispatches `image.yml` and reads the
self-link out of the run; writes the `execution_nodes` entry with that
self-link and an allocation they chose; dispatches `infra.yml plan` and reads
the plan. That plan is the evidence ADR 0020 step 3 asks for.

## What has and has not been exercised

Stated because "the bake exists" and "the bake has produced an image" are
different facts and the second one is not true yet.

- **Run, and checked**: the crane extraction against the real vendored Envoy
  image — `usr/local/bin/envoy`, 102,034,136 bytes, mode 0755, dynamically
  linked against glibc alone (`libm`, `librt`, `libdl`, `libpthread`, `libc`,
  `ld-linux-x86-64`), all of which a Debian 12 base provides. The Ops Agent
  pin: the `.deb` at the path in `image.yml` downloads and hashes to the
  sha256 in `image.yml`, and ships `/lib/systemd/system/google-cloud-ops-agent.service`
  — the exact unit `startup.sh.tftpl` looks for — and depends only on `libc6`,
  `libgcc-s1`, `libssl3`, `libstdc++6` and `libsystemd0`, so `dpkg -i` needs no
  network. `qip-fetch-secret` end to end against a stub metadata server and a
  stub Secret Manager: it sends `Metadata-Flavor: Google`, sends the bearer
  token, base64-decodes the payload, writes the file at mode 0600 and leaves no
  partial. Its refusals fire with their own messages. Every text-parsing
  command in `image.yml` against the real files it reads.
- **Not run**: the bake itself. No image has been created, because that needs
  a project, a credential and an applied `module.image_bake`, and this
  repository's rule is that an agent shows the plan and a person applies. The
  first dispatch is where the cloud-side behaviour — the serial-console
  handshake, `update-grub` on the chosen base, the image's guest OS features —
  is proven, and it should be read as a first run rather than a regression
  test.
