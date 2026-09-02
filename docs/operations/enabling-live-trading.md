# Enabling live trading

**This platform is paper-trading only, and this page authorises nothing.** It
describes what would have to change, in the order the controls sit, so that a
reader can see that each is a separate control and that the first one refuses.
[ADR 0023](../adr/0023-real-trading-is-the-destination-and-the-opening-is-gated.md)
gates the opening; nothing on this page is a step towards it without that
record being revisited.

Three independent layers hold the boundary, and none may be weakened:

1. **Terraform.** `infrastructure/terraform/variables.tf:105-116` refuses
   `supervised_live`, `limited_autonomous_live` and `autonomous_live` at plan
   time, so a live ceiling never reaches a workload.
2. **The composition roots.** `AutonomyLevel::deployable` refuses the same
   three at start-up in `qip-api` (`src/main.rs:60`), `qip-fastbrain`
   (`src/main.rs:115`) and `qip-deepbrain` (`src/main.rs:138`). A live value
   stops the process; it is never silently lowered to paper.
3. **The type system.** `qip_edge::Cell::new` takes no ceiling and there is no
   constructor that takes another
   (`backend/crates/edge/qip-edge/src/cell.rs:249-254`). A cell cannot raise
   its own ceiling.

Terraform catches the reviewed, committed mistake. The composition roots catch
the unreviewed edit to a running service. Neither is redundant.

## 1. Raise the deployment ceiling — refused

In `infrastructure/environments/<env>/terraform.tfvars`:

```hcl
autonomy_ceiling = "supervised_live"
```

The plan stops here, naming the reason: "This platform is paper-trading only,
and the autonomy ceiling names a level at which orders reach a real venue."
That is layer 1, and it is the earliest of the three.

What the configuration *would* do if that validation did not exist, so a
reader can see what it guards (`infrastructure/terraform/main.tf`):

* Create the IAM binding that lets the fast brain read the venue credential
  (`main.tf:228-260`; the predicate is `ceiling_reaches_a_venue` at `:99`, a
  membership test over the three live rungs and never `!= "paper_trading"`).
  Until then the binding does not exist, so the credential is unreadable
  regardless of what the application does.
* Label every resource `live_capable = "true"` (`main.tf:103-117`) and change
  the `live_capable` output (`outputs.tf:45-57`), so a query can answer "which
  of our environments can trade" from the infrastructure.
* Still bind nothing to an execution node. The node's own binding additionally
  requires `shadow_mode = false`, a literal at `main.tf:492`
  (`modules/execution-node/main.tf:100-104`).

## 2. The ceiling every workload reads

There is no separate configuration object to update. Every Cloud Run service
in the catalogue takes `QIP_AUTONOMY_CEILING = var.autonomy_ceiling`
(`infrastructure/terraform/catalogue.tf:65,131,177`) — from the one root
variable, never from a literal, so the ceiling appears in exactly one diff.
The execution node sets no ceiling at all
(`modules/execution-node/templates/startup.sh.tftpl:202-207`), because a
cell's ceiling is structural: layer 3.

A service updated by hand — `gcloud run services update --set-env-vars` —
bypasses layer 1 and meets layer 2: the process refuses to start with a live
value.

## 3. Write the venue credential

Terraform creates the secret `qip-venue-credential` empty
(`infrastructure/terraform/main.tf:205-226`); it never creates a version,
because a version has a value and a value in state is a leaked credential.
Write it out of band:

```sh
gcloud secrets versions add qip-venue-credential --data-file=-
```

Supplying it changes nothing: no identity can read it at any ceiling a plan
can carry. Live trading is not a credential problem.

## 4. Two operators raise the level

Were the platform ever *capable* of live trading, it would still be paper
trading. Raising the level requires:

* An authenticated operator identity.
* A second approver, who must be a different person — `with_second_approver`
  ignores an approver matching the subject.
* A stated reason of at least ten characters.
* A credential authenticated within the last fifteen minutes.

There is deliberately no API endpoint and no CLI command for this. A bearer
token cannot establish two people, and a command line cannot either.

## Confirming

```sh
curl -H "Authorization: Bearer $QIP_TOKEN_VIEWER" .../api/v1/autonomy
```

The response carries the level, the ceiling, and the full history of changes
with the operator, the second approver and the reason for each.

## Going back

Reducing needs none of the above:

```rust
controller.reduce_to(AutonomyLevel::PaperTrading, "reason", now);
```

No operator, no second approver, no freshness requirement. Requiring authority
to stop would be a way for the platform to keep trading when it should not.
