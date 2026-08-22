# Development.
#
# Paper trading, small, and reachable only from the office network.
environment      = "development"
autonomy_ceiling = "paper_trading"

node_count   = 1
machine_type = "n2-standard-2"

# Replace with the ranges that actually need control-plane access. The
# validation rule refuses 0.0.0.0/0, so leaving this empty is safer than
# guessing.
authorised_networks = []
