# Production.
#
# The ceiling here is still paper trading. Raising it is a separate, reviewed
# change to this file, and even then it only permits two authenticated
# operators to enable live trading — it does not enable it.
#
# This is the one line in the repository that decides whether the platform can
# ever reach a real venue, which is why it is a line rather than an inference
# from the environment name.
environment      = "production"
autonomy_ceiling = "paper_trading"

node_count   = 3
machine_type = "n2-standard-8"

authorised_networks = []
