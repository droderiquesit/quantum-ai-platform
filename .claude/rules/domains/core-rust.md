# Domain: core Rust

**Scope** — `crates/libs/**`, `crates/services/**`, `crates/runtime/**`,
`crates/edge/**`, `crates/agents/**`, `crates/quant/**`

## Approved

- Rust 2024. Blocking I/O with an explicit timeout on every call that leaves
  the process.
- `Result` everywhere fallible, with `qip_core::error::Error` and a constructor
  naming the class (`invalid`, `denied`, `io`, `numeric`).
- Errors are refusals whose message names what to do instead.
- `Decimal` for money — never `f64`. Statistics may be `f64`, and the crossing
  point between the two is stated in a comment where it happens.
- `BTreeMap`/`BTreeSet` wherever iteration order reaches output. A replay that
  reorders is not a replay.

## Prohibited

- `unsafe` (forbidden at the workspace root).
- `todo!()`, `unimplemented!()`, and `panic!()` in a `Result`-returning function.
- `unwrap()`/`expect()` outside `#[cfg(test)]` and `tests/`.
- `std::env` outside `crates/apps/**` — a service that reads the environment
  cannot be tested and cannot be deployed twice with different settings.
- Any new dependency without an ADR.
- Clamping an invalid input instead of refusing it.

## Required evidence

`cargo fmt --all --check`; `cargo clippy --workspace --all-targets` at zero
warnings; `cargo test --workspace --no-fail-fast`; and the mutation report for
every new test.
