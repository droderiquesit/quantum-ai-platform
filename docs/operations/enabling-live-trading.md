# Enabling live trading

Four steps, in order. Each is a separate control and none can be skipped.

## 1. Raise the deployment ceiling

In `infrastructure/environments/<env>/terraform.tfvars`:

```hcl
autonomy_ceiling = "supervised_live"
```

Then apply. This does three things beyond changing a variable:

* Creates the IAM binding that lets the fast brain read the venue credential.
  Until now that binding did not exist, so the credential was unreadable
  regardless of what the application did.
* Labels the environment `live_capable = true`, so a query can answer "which of
  our clusters can trade" from the infrastructure.
* Changes the `live_capable` output.

## 2. Update the config map

```yaml
# infrastructure/kubernetes/base/config.yaml
data:
  autonomy_ceiling: "supervised_live"
```

The ceiling lives in a named resource rather than a command line so that
changing it appears in a diff and in the cluster's audit log.

## 3. Write the venue credential

Terraform creates the secret; it never creates a version, because a version has
a value and a value in state is a leaked credential. Write it out of band:

```sh
gcloud secrets versions add qip-venue-credential-<env> --data-file=-
```

## 4. Two operators raise the level

The platform is now *capable* of live trading and is still paper trading.
Raising the level requires:

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
