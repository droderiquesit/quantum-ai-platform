# Operations

Runbooks for the things that actually happen. Each one starts with what to do
and explains afterwards, because the person reading it at three in the morning
needs the first line more than the reasoning.

* [The deployment path](deployment-path.md) — the one way a change reaches a
  running, observable state, what proves each stage, how a rollback is done,
  and how a serving revision traces back to a commit
* [The kill switch tripped](kill-switch.md)
* [A risk limit is breached](limit-breach.md)
* [Enabling live trading](enabling-live-trading.md)
* [An agent attempted an ungranted capability](permission-violation.md)
* [The book and the venue disagree](reconciliation-break.md)
* [Deploying a new edge cell](deploying-an-edge-cell.md)
* [Registering a venue](registering-a-venue.md) — what a person does before a source that needs an account is admitted, and why the platform offers no anonymous or automated signup
* [The arbitrage desk's policy](arbitrage-policy.md)
* [Provisioning a managed Google service](provisioning-managed-services.md)
* [Disaster recovery](disaster-recovery.md)
* [Scaling and availability](scaling-and-availability.md)
* [Multi-region, and why it is not a manifest change](multi-region.md)

And the standing list of what this build cannot reach:

* [External dependencies](external-dependencies.md) — every managed service the
  platform has a port for and no credentials to use, what each would need, and
  the gaps in the infrastructure itself. Read it before deploying anything: it
  names the things that would stop a deployment rather than degrade one.

## The one thing to know

**Reducing autonomy needs no authority.** `AutonomyController::reduce_to` takes
no operator identity, and tripping the kill switch takes none either. If you
are unsure whether to stop the platform, stop it — a false stop costs far less
than a missed one, and clearing it is the step that requires deliberation.

## Reading the state

```sh
qip status        # autonomy, ceiling, kill switch, cycle count, log chain
qip governance    # the agent roster's governance review
qip limits        # the risk limits and why each exists
```

Or over HTTP:

```sh
curl -H "Authorization: Bearer $QIP_TOKEN_MONITOR" \
  https://.../api/v1/health
curl -H "Authorization: Bearer $QIP_TOKEN_VIEWER" \
  https://.../api/v1/system/status
```

`health` answers the two questions a monitor needs: is it halted, and could it
trade live.
