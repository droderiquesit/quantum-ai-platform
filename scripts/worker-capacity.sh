#!/usr/bin/env bash
#
# What concurrency this machine can actually sustain, and which limit binds.
#
# The orchestration policy names a ladder — 8, 16, 32, 64, 100 — but a ladder
# is a target, not a capability. This script measures the rungs that are
# physically available right now and prints the binding constraint, so the
# operating point is a measurement rather than an aspiration. Run it before
# raising concurrency and after any change to the container.
#
# Exit 0 always: this reports, it does not gate. The gate is the policy.
set -uo pipefail

cpus=$(nproc 2>/dev/null || grep -c ^processor /proc/cpuinfo)
mem_avail_mb=$(awk '/MemAvailable/ {print int($2/1024)}' /proc/meminfo)
disk_avail_mb=$(df -Pm . | awk 'NR==2 {print $4}')

# The harness caps concurrent agents inside one workflow at min(16, cpus-2).
# Two cores are reserved: one for the orchestrator's own turn, one for the
# builds and test runs the workers trigger, which are what actually saturate
# a box this size.
harness_ceiling=16
reserved=2
cpu_ceiling=$(( cpus > reserved ? cpus - reserved : 1 ))
(( cpu_ceiling > harness_ceiling )) && cpu_ceiling=$harness_ceiling

# A worker that edits files needs an isolated checkout when its paths could
# collide. This repository's working tree plus a build cache is the cost.
worktree_cost_mb=${WORKTREE_COST_MB:-700}
disk_ceiling=$(( disk_avail_mb / worktree_cost_mb ))

# Each concurrent worker drives a compiler or a browser at some point.
mem_per_worker_mb=${MEM_PER_WORKER_MB:-1500}
mem_ceiling=$(( mem_avail_mb / mem_per_worker_mb ))

effective=$cpu_ceiling
binding="cpu (${cpus} cores, ${reserved} reserved)"
if (( mem_ceiling < effective )); then
  effective=$mem_ceiling
  binding="memory (${mem_avail_mb} MB available, ${mem_per_worker_mb} MB per worker)"
fi
# Disk only binds workers that need isolation; a read-only or single-file
# worker shares the tree. Reported separately for that reason.
(( effective < 1 )) && effective=1

printf 'Algorik worker capacity\n'
printf '  cpus                     %s\n' "$cpus"
printf '  memory available         %s MB\n' "$mem_avail_mb"
printf '  disk available           %s MB\n' "$disk_avail_mb"
printf '\n'
printf '  concurrent workers       %s   <- binding constraint: %s\n' "$effective" "$binding"
printf '  isolated worktrees       %s   (at %s MB each)\n' "$disk_ceiling" "$worktree_cost_mb"
printf '\n'

if (( disk_ceiling < 1 )); then
  printf '  WARNING: no room for an isolated worktree. Workers that write must\n'
  printf '           share the tree with disjoint path ownership, or wait.\n'
fi
if (( effective < 8 )); then
  printf '  NOTE: below the policy floor of 8. The ladder starts where the\n'
  printf '        machine allows, not at 8; raising it here would queue work,\n'
  printf '        not parallelise it.\n'
fi
