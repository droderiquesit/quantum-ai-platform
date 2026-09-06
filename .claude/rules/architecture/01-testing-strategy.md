# Architecture: testing strategy

## Where a test belongs

| Kind | Location |
|---|---|
| Unit — one type's own invariants | `#[cfg(test)] mod tests` beside the code |
| Crate contract | `backend/crates/<group>/<crate>/tests/` |
| Cross-cutting behaviour | `backend/crates/tests/qip-acceptance/tests/` |

There are **21** acceptance suites on 2026-09-06 —
`ls backend/crates/tests/qip-acceptance/tests/*.rs | wc -l`, and
`ls backend/crates/tests/qip-acceptance/tests/*.rs | xargs -n1 basename` for
the names. Adding a twenty-second is fine; putting a cross-cutting assertion
in a crate's own tests is not, because nothing there can see the other side of
the seam.

**This paragraph listed thirteen suites and said "adding a fourteenth is
fine", and it was wrong by eight.** The eight it never named are
`api_boundary`, `console_route`, `egress`, `gitops`, `manifest_wiring`,
`paper_boundary`, `region_share` and `terraform_contract` — which includes the
suite that holds the paper-trading boundary and the one that holds the
Terraform contract, the two an agent most needs to know exist before it
decides a cross-cutting assertion has nowhere to live. A list is the wrong
shape for this fact: it goes stale silently and every reader believes it,
whereas a command goes stale loudly. Run the command; the enumeration is gone
on purpose.

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
