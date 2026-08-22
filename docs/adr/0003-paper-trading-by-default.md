# 0003 — Paper trading by default, with a deployment ceiling

**Status:** accepted

## Decision

Three independent controls stand between the platform and a real venue:

1. `AutonomyLevel::DEFAULT` is `PaperTrading`.
2. The deployment's *ceiling* also starts at paper trading, and is set at
   assembly from `PlatformConfig`. Nothing at runtime can raise it.
3. Reaching any live level requires an authenticated operator with a second
   approver, a stated reason, and a credential authenticated within the last
   fifteen minutes.

The infrastructure adds a fourth: the venue credential's IAM binding does not
exist in an environment whose ceiling is paper trading.

## Why

The failure mode this guards against is not an operator deliberately enabling
live trading. It is a platform that ends up trading live because a
configuration value was missing, an environment variable was misread, or a
default was chosen for convenience.

Every control here is designed so that *forgetting* something leaves the
platform safer, not less safe. An unset environment variable means paper
trading. A missing credential means the API does not start. An IAM binding that
was never created means the venue credential cannot be read.

## What it costs

Enabling live trading is deliberately laborious: a change to a `.tfvars` file,
a Terraform apply, a config map update, and two operators authenticating within
fifteen minutes of each other. That is four steps where a simpler design would
have one.

It also means a live-capable deployment is visibly different in the
infrastructure — a label, an output, an IAM binding — which is the point.

## What would make this wrong

Nothing about the platform's maturity. If the models become excellent, the
right response is to raise the ceiling deliberately, not to remove the ceiling.

The one genuine argument against is operational: if an incident requires
enabling live trading urgently to unwind a position, four steps and two people
is slow. The mitigation is that *reducing* autonomy needs no authority at all,
and the kill switch trips without one — so the urgent direction is always the
fast one.
