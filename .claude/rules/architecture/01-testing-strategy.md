# Architecture: testing strategy

## Where a test belongs

| Kind | Location |
|---|---|
| Unit — one type's own invariants | `#[cfg(test)] mod tests` beside the code |
| Crate contract | `crates/<group>/<crate>/tests/` |
| Cross-cutting behaviour | `crates/tests/qip-acceptance/tests/` |

The acceptance suites are `acceptance`, `architecture`, `chaos`,
`compliance_proof`, `documentation`, `e2e`, `e2e_live`, `infrastructure`,
`performance`, `resilience`, `security`, `stress`, `truth_loop`. Adding a
fourteenth is fine; putting a cross-cutting assertion in a crate's own tests
is not, because nothing there can see the other side of the seam.

## How a test is written

- **Named as a full sentence** describing the property, not the function:
  `an_opportunity_worth_less_than_the_panel_does_not_convene_one`, not
  `test_routing`.
- **Asserts its own premise first.** A test that filters a list and asserts the
  result is empty passes when the list was empty. Assert the list was
  non-empty, then assert the filter.
- **Substring matching is a trap.** `contains("autonomous_live")` is true of
  `"limited_autonomous_live"`. Match the delimited token.
- **Comments name the failure the test prevents**, and where it has already
  happened once, say so — the reader needs to know it is not hypothetical.

## Mutation verification is mandatory

For every new test: break the implementation, run the test, confirm it **fails
for the right reason**, restore byte-for-byte, confirm it passes. Report the
mutation and that it fired.

This is not ceremony. A test in this repository has already passed a mutation
that deleted the exact value it was written to protect — because the value was
a substring of its neighbour. Only mutation testing catches that class.

## Running

Always `cargo test --workspace --no-fail-fast`. Without the flag, cargo stops
at the first failing binary and the totals silently describe a fraction of the
suite.
