#!/bin/bash
#
# The execution node's boot image, from the inside.
#
# This runs once, as the startup script of the throwaway machine
# `.github/workflows/image.yml` creates, and the disk it leaves behind becomes
# the image a tfvars entry names in `boot_image`. It never runs on a node.
#
# The division of labour, and it is the whole design:
#
#   * `modules/execution-node/templates/startup.sh.tftpl` **verifies** the
#     image at every boot and refuses to start a unit when something is
#     missing. It cannot create any of it — Terraform cannot set a kernel
#     parameter and a startup script cannot install a runtime it has no route
#     to fetch.
#   * This script **creates** exactly what that one verifies, and then runs
#     the same predicates itself. A bake that would produce an image the node
#     refuses to boot on fails here, on a machine nobody is trading from,
#     instead of eight minutes into an apply that already created a subnet, an
#     identity and four IAM bindings.
#
# The two lists are kept in step by
# `the_boot_image_bake_installs_exactly_what_the_node_refuses_to_start_without`
# in the infrastructure acceptance suite, which reads both files and compares
# them. A path added to one and not the other fails the suite rather than the
# first boot.
#
# It fetches one object and nothing else. The builder's subnet denies all
# egress except TCP 443 to the restricted VIP (`modules/image-bake`), so a
# line that grew a `curl https://some.host/install.sh` would fail to connect
# rather than quietly pulling something nobody reviewed into the image a
# trading node boots. Everything installed here came out of the payload the
# runner staged, and everything in that payload came out of an artefact the
# pipeline attested.

set -euo pipefail

readonly METADATA=http://169.254.169.254/computeMetadata/v1/instance
readonly WORK=/var/tmp/qip-image-bake
readonly MANIFEST=/etc/qip/boot-image-manifest

# Everything goes to the serial console as well as to stderr. The workflow has
# no network route to this machine — it has no external address and answers no
# port — so `gcloud compute instances get-serial-port-output` is the only way
# a run can tell a bake that worked from one that did not.
say() { echo "qip-image-bake: $*" | tee /dev/console >&2 || echo "qip-image-bake: $*" >&2; }

# The two sentinels the workflow greps for. Exactly one is ever printed.
result_ok() { echo "QIP-IMAGE-BAKE-RESULT: OK" | tee /dev/console >&2 || true; }
result_failed() { echo "QIP-IMAGE-BAKE-RESULT: FAILED $*" | tee /dev/console >&2 || true; }

# A failure prints the FAILED sentinel and shuts down anyway. Leaving the
# machine running would bill for a build nobody is going to use, and the
# workflow refuses to create an image without the OK sentinel, so a stopped
# machine with no OK is unambiguous.
on_failure() {
  local line=$1
  result_failed "at line ${line}"
  say "REFUSING TO BAKE. The disk is left as it is; no image is created from it."
  # Scheduled a minute out rather than immediate, so the workflow's serial
  # read still finds the sentinel above. `shutdown` returns as soon as it has
  # scheduled, which is why the explicit exit below is here: an ERR trap that
  # falls through would let the script carry on past the step that failed.
  shutdown -h +1 || poweroff -f || true
  exit 1
}
trap 'on_failure $LINENO' ERR

fail() {
  say "REFUSING TO BAKE: $*"
  return 1
}

metadata() {
  curl -fsS --max-time 20 -H "Metadata-Flavor: Google" "${METADATA}/attributes/$1"
}

# --- 1. what the base image has to already be -------------------------------
#
# Checked before anything is installed, so a base image chosen wrongly is
# named here rather than discovered by a node that will not start.

say "checking the base image"

# `python3` is the whole runtime of `qip-fetch-secret`, which the node's
# startup script refuses to start without. Google's Debian images carry it;
# a minimal base somebody swapped in might not, and the failure would then be
# a node that boots, finds the helper, runs it, and gets "No such file or
# directory" for the interpreter — an error naming neither the helper nor the
# base image.
command -v python3 >/dev/null 2>&1 || fail "the base image has no python3. /usr/local/bin/qip-fetch-secret is a python3 program and the node refuses to start without a working one. Choose a Debian base image that carries python3."
command -v dpkg >/dev/null 2>&1 || fail "the base image has no dpkg; the Ops Agent is installed from a .deb this bake staged."
# Checked before they are used, not while they are: `curl` reads the instance
# metadata below, `sha256sum` is what proves the payload is the reviewed
# bytes, and `tar` unpacks it. A base image missing any of the three would
# otherwise fail at a line that reads as a network problem.
for tool in curl sha256sum tar; do
  command -v "$tool" >/dev/null 2>&1 || fail "the base image has no ${tool}; the payload cannot be fetched, verified or unpacked without it."
done
command -v update-grub >/dev/null 2>&1 || fail "the base image has no update-grub, so the kernel command line §41.4 requires cannot be written. This bake targets a Debian image using GRUB."
[ -d /etc/default/grub.d ] || fail "the base image has no /etc/default/grub.d, so there is nowhere to drop the kernel command line without editing a file the base's own updates own."

# §41.4: nothing between the binary and the kernel. Refused rather than
# purged: a base image carrying a container runtime is a base somebody chose
# wrongly, and quietly removing it would hide that choice from the next bake.
for runtime in docker containerd podman crio runc; do
  if command -v "$runtime" >/dev/null 2>&1; then
    fail "the base image has ${runtime} installed. §41.4 requires an image with no container runtime, and the node's startup script refuses to start any unit on one. Choose a base without it."
  fi
done

# --- 2. the payload ---------------------------------------------------------
#
# One object, content-addressed, staged by the runner out of artefacts the
# pipeline attested. Read with the instance's own token from the metadata
# server; the builder identity's only grant is `storage.objectViewer` on the
# one bucket.

payload_url="$(metadata qip-payload-url)"
payload_sha256="$(metadata qip-payload-sha256)"
isolcpus="$(metadata qip-isolcpus)"
hugepages_gb="$(metadata qip-hugepages-gb)"
provenance="$(metadata qip-provenance)"

[ -n "$payload_url" ] || fail "no qip-payload-url in the instance metadata"
[ -n "$payload_sha256" ] || fail "no qip-payload-sha256 in the instance metadata"
[ -n "$isolcpus" ] || fail "no qip-isolcpus in the instance metadata"
[ -n "$hugepages_gb" ] || fail "no qip-hugepages-gb in the instance metadata"

say "fetching the payload"
mkdir -p "$WORK"
cd "$WORK"

# The token is obtained, used and discarded inside one process — never an
# argument, so never in `ps`; never an environment value, so never in
# /proc/<pid>/environ or a crash dump. The same rule `qip-fetch-secret`
# follows, for the same reason, in the same language.
python3 - "$payload_url" "${WORK}/payload.tar.gz" <<'PY'
import json, sys, urllib.request

url, destination = sys.argv[1], sys.argv[2]
token_url = ("http://169.254.169.254/computeMetadata/v1/instance/"
             "service-accounts/default/token")
with urllib.request.urlopen(
    urllib.request.Request(token_url, headers={"Metadata-Flavor": "Google"}),
    timeout=20,
) as response:
    token = json.loads(response.read().decode("utf-8"))["access_token"]
if not token:
    sys.exit("the metadata server returned no access_token; the builder has no identity")
request = urllib.request.Request(url, headers={"Authorization": "Bearer %s" % token})
with urllib.request.urlopen(request, timeout=600) as response, open(destination, "wb") as out:
    while True:
        chunk = response.read(1 << 20)
        if not chunk:
            break
        out.write(chunk)
PY

echo "${payload_sha256}  ${WORK}/payload.tar.gz" | sha256sum -c - \
  || fail "the payload does not hash to what the runner staged. Either the object changed under the bake or the download truncated; nothing is installed from bytes that are not the reviewed ones."

mkdir -p "${WORK}/payload"
tar -xzf "${WORK}/payload.tar.gz" -C "${WORK}/payload"
cd "${WORK}/payload"

# The payload carries its own manifest, and every member is checked against
# it. The outer hash proves the tarball is the one the runner uploaded; this
# proves each file inside it is the one the runner put there, which is what
# lets the image's manifest name a digest per file rather than one per
# archive.
[ -f MANIFEST.sha256 ] || fail "the payload carries no MANIFEST.sha256"
sha256sum -c MANIFEST.sha256 \
  || fail "a file in the payload does not match the manifest the runner wrote"

# --- 3. the binaries the node refuses to start without -----------------------
#
# Every one of these is checked by
# modules/execution-node/templates/startup.sh.tftpl at every boot. The
# acceptance suite compares the two lists.

say "installing the binaries"
install -D -m 0755 -o root -g root qip-edge-node /usr/local/bin/qip-edge-node
install -D -m 0755 -o root -g root envoy /usr/local/bin/envoy
install -D -m 0755 -o root -g root qip-fetch-secret /usr/local/bin/qip-fetch-secret

# --- 4. the Ops Agent -------------------------------------------------------
#
# A node nothing scrapes is a node nobody can operate: the alert policies in
# modules/observability that name the edge series watch descriptors nothing
# ingests until this agent's Prometheus receiver scrapes the health port. The
# startup script writes the receiver's configuration and restarts the unit;
# this is what puts the unit there.
#
# From a .deb the runner staged and this bake verified, not from Google's
# repository-adding script. The builder has no route to the internet, and a
# `curl | bash` in an image build is an unpinned third party in the trusted
# computing base of a trading machine.
say "installing the ops agent"
dpkg -i ./google-cloud-ops-agent.deb
systemctl enable google-cloud-ops-agent.service
# Stopped before imaging: an agent left running writes state and a hostname
# into the disk the image is taken from. It is enabled, so the node starts it.
systemctl stop google-cloud-ops-agent.service || true

# --- 5. the kernel command line ---------------------------------------------
#
# The half of §41.4 that lives below everything Terraform can reach.
# `startup.sh.tftpl` greps /proc/cmdline for `isolcpus=<range>` as a whole
# word and refuses when it is absent; this is where the word comes from.
#
# `nohz_full` and `rcu_nocbs` carry the same range. isolcpus alone keeps the
# scheduler off those cores and leaves the tick and the RCU callbacks on them,
# which is a latency floor nobody measured and cannot explain afterwards. The
# node's own check does not look for these two, deliberately: they make the
# isolation worth having and their absence is not a reason to refuse a boot.
#
# 1 GiB pages, `hugepages` counted in gigabytes, because that is the unit
# `required_hugepages_gb` is in and the arithmetic in the startup script —
# Hugepagesize × HugePages_Total ÷ 1048576 — comes out in. C3 and C3D support
# 1 GiB pages; the builder's own shape is irrelevant, because nothing here
# reboots.
say "writing the kernel command line: isolcpus=${isolcpus}, ${hugepages_gb} GiB of 1G huge pages"
cat >/etc/default/grub.d/99-qip-execution-node.cfg <<EOF
# Written by infrastructure/images/execution-node/provision.sh.
#
# modules/execution-node derives the isolated range from the machine shape —
# cores 0 and 1 for the OS, the telemetry drainer and the control-plane
# client, the rest isolated (§41.3) — so this image is built for one machine
# type and the node's startup script refuses on any other.
GRUB_CMDLINE_LINUX="\$GRUB_CMDLINE_LINUX isolcpus=${isolcpus} nohz_full=${isolcpus} rcu_nocbs=${isolcpus} default_hugepagesz=1G hugepagesz=1G hugepages=${hugepages_gb}"
EOF
update-grub

# The honest post-condition. /proc/cmdline still shows the command line this
# machine booted with, and nothing reboots here, so what can be checked is
# what the next boot will use: the generated grub.cfg.
grep -q "isolcpus=${isolcpus}" /boot/grub/grub.cfg \
  || fail "update-grub produced a grub.cfg with no isolcpus=${isolcpus}; the image would boot without isolated cores and every node built from it would refuse to start."
grep -q "hugepages=${hugepages_gb}" /boot/grub/grub.cfg \
  || fail "update-grub produced a grub.cfg with no hugepages=${hugepages_gb}; the image would boot without preallocated huge pages."

# --- 6. no swap -------------------------------------------------------------
#
# A page of the order book on a disk is a latency number that cannot be
# explained afterwards, and `mlockall` cannot save a process from a machine
# that was allowed to swap in the first place. Google's Debian images ship
# none; this is what keeps that true of an image built from a base that grew
# one.
say "removing swap"
swapoff -a || true
sed -i.bak '/[[:space:]]swap[[:space:]]/d' /etc/fstab
systemctl mask swap.target
[ "$(awk 'NR>1' /proc/swaps | wc -l)" -eq 0 ] \
  || fail "swap is still active after swapoff; §41.4 requires none."

# --- 7. the same predicates the node will run -------------------------------
#
# Copied in intent from startup.sh.tftpl rather than trusted to have been
# satisfied by the steps above. A bake whose install step silently did
# nothing would otherwise produce an image that passes here and fails on the
# node, at the point where the failure costs an apply.
say "running the node's own image checks"
for runtime in docker containerd podman crio runc; do
  command -v "$runtime" >/dev/null 2>&1 && fail "${runtime} appeared during provisioning."
done
[ -x /usr/local/bin/qip-edge-node ] || fail "/usr/local/bin/qip-edge-node is not executable"
[ -x /usr/local/bin/qip-fetch-secret ] || fail "/usr/local/bin/qip-fetch-secret is not executable"
[ -x /usr/local/bin/envoy ] || fail "/usr/local/bin/envoy is not executable"
systemctl cat google-cloud-ops-agent.service >/dev/null 2>&1 \
  || fail "google-cloud-ops-agent.service is not on this image"

# The two binaries are run once, because a file being executable says nothing
# about whether its interpreter or its shared libraries are here. Envoy is
# dynamically linked against glibc; `qip-edge-node` is static musl and needs
# nothing, which is exactly the claim worth checking rather than assuming.
/usr/local/bin/envoy --version >/dev/null \
  || fail "/usr/local/bin/envoy will not run on this base image. It is dynamically linked against glibc; the base is missing something it needs."
/usr/local/bin/qip-fetch-secret >/dev/null 2>&1 && \
  fail "qip-fetch-secret exited zero with no arguments; it is supposed to refuse."

# --- 8. the image's own record ----------------------------------------------
#
# What went into this image, on the image, so an operator on a node can answer
# "which artefacts is this machine running" without the run that built it.
# The image's labels carry the same facts for someone who has only the image.
install -d -m 0755 /etc/qip
printf '%s\n' "$provenance" >"$MANIFEST"
{
  echo "isolcpus=${isolcpus}"
  echo "hugepages_gb=${hugepages_gb}"
  echo "payload_sha256=${payload_sha256}"
} >>"$MANIFEST"
chmod 0444 "$MANIFEST"
cp MANIFEST.sha256 /etc/qip/boot-image-contents.sha256
chmod 0444 /etc/qip/boot-image-contents.sha256

# --- 9. make the disk generic ------------------------------------------------
#
# An image carrying this machine's host keys would give every node built from
# it the same SSH identity, and a machine-id that never changes breaks every
# per-instance thing systemd keys on one.
say "clearing the builder's own identity from the disk"
cd /
rm -rf "$WORK"
rm -f /etc/ssh/ssh_host_*
: >/etc/machine-id
rm -f /var/lib/dbus/machine-id
journalctl --rotate >/dev/null 2>&1 || true
journalctl --vacuum-time=1s >/dev/null 2>&1 || true
rm -rf /var/lib/dhcp/* /var/tmp/* /tmp/*
apt-get clean || true
find /var/log -type f -exec truncate -s 0 {} + || true
sync

result_ok
say "done. Shutting down so the disk can be imaged."
shutdown -h +1
