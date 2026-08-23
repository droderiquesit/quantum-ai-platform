# Deploying a new edge cell

**Before you start:** a cell trades on its own authority, inside a capital
envelope granted in advance. Bringing one up wrong does not fail loudly — it
produces a cell that either cannot reach its venue, or can reach more than its
venue. Both are quiet.

Nothing in this runbook has been executed against a real project. It is written
from the configuration in `infrastructure/`, and the first person to follow it
should expect to correct it.

## Do this

1. **Pick the cell id and the region.** The cell id goes in three places and
   must be the same string in all of them: the Terraform `edge_cells` key, the
   `QIP_CELL_ID` environment variable, and the `cell` field of every
   `CapitalEnvelope` the central plane grants there. A cell refuses an envelope
   addressed elsewhere, so a mismatch is a cell that starts and then rejects
   every grant it is sent.

2. **Allocate its addresses.** Cell *n*, counting from one, takes:

   | range | value |
   | --- | --- |
   | subnet | `10.(16n).0.0/20` |
   | pods | `10.(16n+4).0.0/14` |
   | services | `10.(16n+8).0.0/20` |

   So the first cell is `10.16.0.0/20`, `10.20.0.0/14`, `10.24.0.0/20`; the
   second is `10.32.0.0/20`, `10.36.0.0/14`, `10.40.0.0/20`. The primary subnet
   (`10.0.0.0/20`, `10.4.0.0/14`, `10.8.0.0/20`) is cell zero in this scheme and
   is where the central plane lives. Overlapping ranges do not error: they route
   to whichever subnet was created first.

3. **Add the entry** to `edge_cells` in the environment's
   `infrastructure/environments/<environment>/terraform.tfvars`. Leave `venues`
   empty for now.

4. **Apply the Terraform.** This creates the subnet, the service account, the
   workload identity binding, the egress firewall rules and the IAM the cell
   needs — object creation on the evidence bucket, read on the registry, read on
   the capital-envelope verification key, and telemetry.

5. **Tag the cell's nodes.** Every firewall rule constraining the cell targets
   the tag in `terraform output edge_cells`. A rule targeting a tag nothing
   carries does nothing, and does it silently. Check with
   `gcloud compute instances list --filter="tags.items=<tag>"` before going
   further; an empty result means the cell has unconstrained egress.

6. **Write the verification key.** The `qip-capital-envelope-key` secret is
   created empty by Terraform, because a value in Terraform is a value in the
   state file. Write the version out of band. It is the key envelope signatures
   are checked against — its confidentiality matters less than its integrity,
   because whoever can replace it can mint envelopes.

7. **Render and apply the manifests.** `edge-cell.yaml` is not applied by the
   deploy pipeline; it is applied here, deliberately, because a workload that
   trades should not appear unattended.

   ```sh
   sed -e "s#CELL_ID#london-1#g" \
       -e "s#CELL_REGION#europe-west2#g" \
       -e "s#CELL_VENUES#<venue ids, comma separated>#g" \
       -e "s#IMAGE_PREFIX#$(terraform output -raw image_prefix)#g" \
       -e "s#IMAGE_TAG#<commit sha>#g" \
       -e "s#ENVIRONMENT#<environment>#g" \
       -e "s#PROJECT#<project>#g" \
       infrastructure/kubernetes/base/edge-cell.yaml | kubectl apply -f -
   ```

   `CELL_VENUES` becomes `QIP_VENUES`, and the node refuses to start without
   it: a cell that does not know which venues it is for cannot check an
   envelope's venue scope, and one that started anyway would report itself
   healthy while being unable to trade. Use the same venue ids as the
   `venues` map in step 3, even while that map is empty — see the next step
   for why those two are not the same thing.

8. **Confirm the cell can reach nothing yet.** Two different things are being
   checked here, and conflating them is how a cell ends up trading before
   anyone meant it to.

   `QIP_VENUES` is what the cell is *configured* for. The Terraform `venues`
   map — still empty at this point — is what it may *reach*: no entry means no
   firewall rule and no `allow-edge-egress` rule, so every venue connection
   fails at the network. The narrower of the two always wins.

   So the state to confirm is: the node starts, logs its cell id, prints the
   venue endpoints and credentials it is still awaiting, serves `/health`, and
   opens no venue connection. Running, connected to nothing, holding no
   envelope. That is the state to be in before granting any capital.

9. **Add the venues, last.** Put the ranges the venue publishes into the
   `venues` map, apply, and substitute `VENUE_CIDR` and `VENUE_PORT` in
   `allow-edge-egress`. Terraform validation refuses `0.0.0.0/0`. Do not guess
   a range: a wrong one produces a cell that cannot trade, and a wide one
   produces a cell that can reach the internet.

10. **Grant capital last of all**, through the central plane's approval path.
    Until an envelope exists the cell can commit nothing, which is the correct
    state for a cell nobody has watched yet.

## To take a cell out

Remove its entry from `edge_cells` and apply. The service account, the subnet
and the firewall rules go with it, which is the reason a cell's identity lives
in its own module rather than in the shared account map: an account left behind
by a removed cell is a credential nobody owns.

Let the envelopes expire rather than revoking them, if you can. Every envelope
expires by construction, and a cell that stops when its grant runs out stops
cleanly.

## The seven locations

ADR 0008 calls for seven cells. Adding the other six is six entries in the map
above — no new module, no new directory.

| cell id | location | region | note |
| --- | --- | --- | --- |
| `dallas-1` | Dallas | `us-south1` | in the metro |
| `chicago-1` | Chicago | `us-central1` | **not in the metro** — Council Bluffs, Iowa |
| `newyork-1` | NY/NJ | `us-east4` | **not in the metro** — Ashburn, Virginia |
| `london-1` | London | `europe-west2` | in the metro |
| `frankfurt-1` | Frankfurt | `europe-west3` | in the metro |
| `singapore-1` | Singapore | `asia-southeast1` | in the metro |
| `tokyo-1` | Tokyo | `asia-northeast1` | in the metro |

Two of the seven are a problem worth reading before building on this table.

Google Cloud has no region in Chicago and none in the New York/New Jersey
metropolitan area. The nearest regions are roughly 400 and 300 kilometres away
respectively, which is several milliseconds of round trip that a cell whose
whole argument is source-adjacency cannot spend. A cell in `us-central1` is not
next to CME any more than the central plane is.

That is an architectural gap rather than a configuration one, and it has three
honest answers: colocation with a partner interconnect back to the VPC, running
those two cells somewhere other than Google Cloud, or accepting that the two
American equity and futures cells are not latency-competitive and saying so.
ADR 0008 already names the condition under which the whole cell architecture
should be collapsed back into the central plane, and this is evidence for that
question rather than against it.

## What this runbook does not cover

The cell's node pool. This module creates the cell's subnet and identity, not
the nodes that run it, and until a node pool exists carrying the cell's tag the
cell has nowhere to be scheduled. See
[external dependencies](external-dependencies.md).
