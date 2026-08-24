# Test: the central plane and one cell.
#
# One cell rather than none, because the failures worth catching before
# production are the ones that need two processes and a network between them —
# an envelope that does not verify, a delta that arrives twice, a peer that
# stops answering. One cell exercises every one of those. Nine would exercise
# the same code nine times.
#
# `london-1` because it is one of the six that is genuinely in the right
# metropolitan area, so a latency number measured here means something.

environment      = "test"
region           = "europe-west2"
autonomy_ceiling = "paper_trading"

node_count   = 1
machine_type = "e2-standard-4"

authorised_networks = []

edge_cells = {
  "london-1" = {
    region       = "europe-west2"
    subnet_cidr  = "10.68.0.0/20"
    pod_cidr     = "10.68.16.0/20"
    service_cidr = "10.68.32.0/20"
    # No venues. A test cell that could reach a real venue is a test cell that
    # can send a real order.
    venues = {}
  }
}

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
