# Stage: production's shape at a third of its size.
#
# Three cells rather than one or nine. One cell cannot show a failure that only
# appears when several cells publish to the centre at once — a queue that
# refuses under combined load, a spool that fills, a circuit breaker that opens
# for one peer while the others keep working. Nine would cost production money
# to learn nothing more.
#
# The three are chosen to span the differences that matter rather than to be
# nearby: three continents, three round-trip times to the centre, and one of
# them (`newyork-1`) is a cell the documentation is explicit is *not* in its
# metropolitan area. Staging is where that cost should be measured, not
# production.

environment      = "stage"
region           = "europe-west2"
autonomy_ceiling = "paper_trading"

node_count   = 2
machine_type = "e2-standard-8"

authorised_networks = []

edge_cells = {
  "london-1" = {
    region       = "europe-west2"
    subnet_cidr  = "10.68.0.0/20"
    pod_cidr     = "10.68.16.0/20"
    service_cidr = "10.68.32.0/20"
    venues       = {}
  }
  "newyork-1" = {
    region       = "us-east4"
    subnet_cidr  = "10.67.0.0/20"
    pod_cidr     = "10.67.16.0/20"
    service_cidr = "10.67.32.0/20"
    venues       = {}
  }
  "tokyo-1" = {
    region       = "asia-northeast1"
    subnet_cidr  = "10.71.0.0/20"
    pod_cidr     = "10.71.16.0/20"
    service_cidr = "10.71.32.0/20"
    venues       = {}
  }
}

# Still off. Staging proves the deployment, and turning on a store no adapter
# can open would prove nothing except that Terraform can create it.
enable_bigquery      = false
enable_cloud_storage = false
enable_alloydb       = false
enable_bigtable      = false
enable_memorystore   = false
enable_spanner       = false
enable_vertex_ai     = false

# The only repository whose pipeline may deploy into this project. No default
# exists for this on purpose: a default would name a repository somebody else
# could be running, and the consequence of getting it wrong is that their
# pipeline pushes images and applies manifests here.
github_repository = "droderiquesit/quantum-ai-platform"
