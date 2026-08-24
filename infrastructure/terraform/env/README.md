# Environments

Four files, one per environment, applied with:

```
terraform -chdir=infrastructure/terraform apply -var-file=env/<name>.tfvars
```

`project_id` and `project_number` are deliberately **not** in these files.
Every environment should be a separate Google project — a blast radius that
stops at a project boundary is the only one that reliably stops — so the pair
is passed on the command line or in a `*.auto.tfvars` that is not committed.

## The CIDR plan

The central plane keeps the module defaults in every environment. Cells are
allocated from `10.<64 + n>.0.0/16`, one block per cell, `n` fixed per cell id
and never reused:

| n | cell | subnet | pods | services |
| --- | --- | --- | --- | --- |
| 1 | `dallas-1` | `10.65.0.0/20` | `10.65.16.0/20` | `10.65.32.0/20` |
| 2 | `chicago-1` | `10.66.0.0/20` | `10.66.16.0/20` | `10.66.32.0/20` |
| 3 | `newyork-1` | `10.67.0.0/20` | `10.67.16.0/20` | `10.67.32.0/20` |
| 4 | `london-1` | `10.68.0.0/20` | `10.68.16.0/20` | `10.68.32.0/20` |
| 5 | `frankfurt-1` | `10.69.0.0/20` | `10.69.16.0/20` | `10.69.32.0/20` |
| 6 | `singapore-1` | `10.70.0.0/20` | `10.70.16.0/20` | `10.70.32.0/20` |
| 7 | `tokyo-1` | `10.71.0.0/20` | `10.71.16.0/20` | `10.71.32.0/20` |
| 8 | `saopaulo-1` | `10.72.0.0/20` | `10.72.16.0/20` | `10.72.32.0/20` |
| 9 | `dubai-1` | `10.73.0.0/20` | `10.73.16.0/20` | `10.73.32.0/20` |

The number belongs to the cell id rather than to the environment, so the same
cell has the same range everywhere and a range read from a packet capture
identifies a cell without asking which environment it came from. `edge_cells`
has a validation that refuses two cells sharing a subnet, which catches a
copy-paste that reuses a block — but only within one apply, so the table above
is what keeps them apart across environments.

## The node counts are per zone

`node_count`, `min_node_count` and `max_node_count` are all **per zone**, and every
environment here is a regional cluster across three. The real numbers are three
times what these files say: production's `min_node_count = 3` is nine nodes.

Reading them as regional totals sizes a pool at a third of what was meant, which
is why all three are stated explicitly in every file rather than left to the
module defaults.

`node_count` is now the size **at creation only**. After the pool exists the
autoscaler owns its size, and the node pool ignores later changes to that
variable on purpose: `initial_node_count` forces replacement, so editing this
line would otherwise destroy the pool and recreate it — draining every pod in
the cluster at once, in a plan whose summary reads "1 to add, 1 to destroy".
Change the bounds instead.

The floor is deliberately not lower than the committed size outside development
and test. Scaling down means draining, and a quiet period on this platform is a
market that has closed followed by one that opens.

## Why `venues` is empty everywhere

Every cell here has `venues = {}`, which means it can reach no venue. That is
the correct committed state: a venue's address ranges come from the venue's own
published documentation, and a range guessed from a DNS lookup is a firewall
rule that works until the venue moves a host.

`deploying-an-edge-cell.md` step 5 is where they get filled in, per venue, by
someone reading the venue's documentation.

## What none of these files can do

None enables live trading. `autonomy_ceiling` is `paper_trading` in all four,
including production, because live trading is enabled by two authenticated
approvals at run time and not by a deployment. A tfvars file that could turn it
on would make an infrastructure change into a trading decision.
