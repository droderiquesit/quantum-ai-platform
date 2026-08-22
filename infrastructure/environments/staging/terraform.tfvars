# Staging.
#
# Paper trading against production-shaped data. Deliberately not live: a
# staging environment that can trade is a production environment nobody
# reviews.
environment      = "staging"
autonomy_ceiling = "paper_trading"

node_count   = 2
machine_type = "n2-standard-4"

authorised_networks = []
