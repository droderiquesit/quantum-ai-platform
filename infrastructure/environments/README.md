# Environments

Four directories, one per environment, named after the value of
`var.environment` inside them. Normally you do not apply these by hand —
`scripts/bootstrap-deploy.sh <env>` does the whole first deployment, reads its
project and repository out of the file, and never auto-approves. By hand it is:

```
terraform -chdir=infrastructure/terraform apply \
  -var-file=../environments/<env>/terraform.tfvars \
  -var-file=../environments/<env>/images.tfvars
```

Two files per environment, and the split is who writes them. `terraform.tfvars`
is reviewed configuration a person edits. `images.tfvars` is written by
`.github/workflows/deploy.yml` after it has built, scanned, signed and attested
an image and moved the Cloud Run service to it; it records the digest each
service was last deployed at, so Terraform creates a service at bytes a
numbered run attested and never at a digest somebody typed. `dev` has one;
the three unprovisioned environments do not, because nothing has ever been
built for them, and `catalogue.tf` refuses to plan a service with no digest.

The names are short because they are interpolated into Google resource ids with
hard length limits: a service account is 30 characters, and `production` was
one over for a node. Directory name and variable value are the same string on
purpose — the thing you type is the thing that lands in a resource name.

`project_id` **is** in these files, and each environment names a project of its
own — or says it has none. `dev` is `algorik-dev` and is the only one
provisioned; `test`, `stage` and `prod` carry the literal marker
`unprovisioned`, which `terraform` refuses at plan time and `deploy.yml` and
`vendor.yml` refuse before they authenticate, each naming what is missing.

Provisioning one of them means a project of its own — never a project another
environment already uses, because two environments in one project share one
IAM boundary, one KMS key ring and one Binary Authorization attestor whatever
their name prefixes say — plus its own state bucket, and the id and number
recorded here. `every_environment_names_a_project_of_its_own` in the
acceptance suite holds both halves of that.

## What each environment runs

| env | Cloud Run services | execution nodes | why |
| --- | --- | --- | --- |
| `dev` | api, fastbrain, deepbrain | 0, and 1 authorised | ADR 0035 authorises `newyork-1` in `us-east4`, shadow mode. No boot image exists yet and no allocation has been chosen, so the entry sits in the tfvars as a comment. |
| `test` | the same three | 0 | A test node that could reach a real venue could send a real order. |
| `stage` | the same three | 0 | ADR 0035 authorises `dev` and nothing else, until the first node has run and taught something. |
| `prod` | the same three | 0 | The same, and prod additionally requires a human dispatch and its own approval. |

Every environment leaves `execution_nodes = {}`, and only `dev` has an
authorisation to stop doing so. The reason is no longer "no venue's ranges are
recorded" — the node ADR 0035 authorises prices from the in-process simulated
feed and needs none. It is that nothing in this repository bakes a Compute
Engine boot image, and that `region_allocation` is a number a person chooses.
`modules/execution-node/README.md` has the entry a node needs and
`environments/dev/terraform.tfvars` has the one that is decided.

Adding an entry to `test`, `stage` or `prod` is outside ADR 0035 and
`adr_0035_authorises_one_shadow_node_in_dev_and_none_anywhere_else` in the
`infrastructure` acceptance suite refuses it.

## The CIDR plan

Trust zones take `10.0.32.0/24` upward in every environment — each environment
is its own project and its own VPC, so the plan does not collide across them —
and the console's subnet is `10.0.16.0/26` where it exists. Execution nodes are
allocated from `10.<64 + n>.0.0/16`, one block per node, `n` fixed per node id
and never reused:

| n | node | subnet |
| --- | --- | --- |
| 1 | `dallas-1` | `10.65.0.0/20` |
| 2 | `chicago-1` | `10.66.0.0/20` |
| 3 | `newyork-1` | `10.67.0.0/20` |
| 4 | `london-1` | `10.68.0.0/20` |
| 5 | `frankfurt-1` | `10.69.0.0/20` |
| 6 | `singapore-1` | `10.70.0.0/20` |
| 7 | `tokyo-1` | `10.71.0.0/20` |
| 8 | `saopaulo-1` | `10.72.0.0/20` |
| 9 | `dubai-1` | `10.73.0.0/20` |

Two of the nine have no Google Cloud region in the right metropolitan area —
`chicago-1` and `dubai-1` — and `docs/operations/deploying-an-edge-cell.md`
says what that costs.
