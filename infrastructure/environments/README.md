# Environments

Four directories, one per environment, named after the value of
`var.environment` inside them. Normally you do not apply these by hand —
`scripts/bootstrap-deploy.sh <env>` does the whole first deployment, reads its
project and repository out of the file, and never auto-approves. By hand it is:

```
terraform -chdir=infrastructure/terraform apply \
  -var-file=../environments/<env>/terraform.tfvars
```

The names are short because they are interpolated into Google resource ids with
hard length limits: `qip-edge-frankfurt-1-production` is 31 characters against a
service account's limit of 30, so `production` could not deploy a cell at all.
Directory name and variable value are the same string on purpose — the thing you
type is the thing that lands in a resource name.

`project_id` **is** in these files, and each environment names a project of its
own — or says it has none. `dev` is `algorik-dev` and is the only one
provisioned; `test`, `stage` and `prod` carry the literal marker
`unprovisioned`, which `terraform` refuses at plan time and `deploy.yml` and
`vendor.yml` refuse before they authenticate, each naming what is missing.

These three used to carry a real id — all four named one project, on the
reasoning that the `environment` prefix kept resource names apart. That
premise expired twice without the files noticing: `dev` moved to its own
project, and the project the other three named was deleted, so their recorded
id pointed at nothing while reading as entirely plausible in review. A dead id
that looks real is worse than an obvious hole, which is what the marker is.

Provisioning one of them means a project of its own — never a project another
environment already uses, because two environments in one project share one
IAM boundary, one KMS key ring and one Binary Authorization attestor whatever
their name prefixes say — plus its own state bucket, and the id and number
recorded here. `every_environment_names_a_project_of_its_own` in the
acceptance suite holds both halves of that.

The id itself is an identifier and not a secret: it appears in every resource
name and in the pipeline's own configuration, so keeping it out of version
control would buy nothing and cost reproducibility.

`project_number` is not here — it is a `terraform output`, not an input.

## How many cells each environment runs

| env | cells | why |
| --- | --- | --- |
| `dev` | 0 | A cell exists to be next to a venue and there is no venue here. |
| `test` | 1 | One cell exercises every two-process failure worth catching. |
| `stage` | 3 | Three spans the failures that only appear under combined load. |
| `prod` | 1 (`london-1`) | Cells come up one at a time; the other eight are commented in place. |

The eight commented cells in `prod/terraform.tfvars` are kept in the file rather
than deleted so their CIDR blocks are not reused. Uncommenting one, then filling
in its `venues`, is how a cell is added — see
`docs/operations/deploying-an-edge-cell.md`.

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
