#!/usr/bin/env bash
# One sharded participant replay: `run-shards.sh <shards>` starts the shards
# of data/simulation/sharded-replay/set.json as `qip simulate` processes (one
# per shard, as a batch job would) and prints each shard's output. Paper only:
# a replay is a simulated outcome over recorded history and sends nothing.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
shards="${1:?usage: run-shards.sh <shards>}"
qip="${QIP_BIN:-qip}"
for k in $(seq 1 "$shards"); do
  "$qip" simulate --set "$here/set.json" --shard "$k/$shards" --participants "$here/participants.json" &
done
wait
