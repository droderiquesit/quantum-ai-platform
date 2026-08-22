# Workspace-level tests

Tests that span more than one crate, or that check something outside the Rust
code entirely.

* `infrastructure.rs` — structural checks on the Terraform and Kubernetes
  configuration. `terraform validate` catches a malformed configuration; these
  catch a well-formed one that would deploy something unsafe. Both run in CI.
* `documentation.rs` — checks that what the documentation claims matches what
  the code does. Documentation that has drifted from the code is worse than
  none, because someone will believe it.
