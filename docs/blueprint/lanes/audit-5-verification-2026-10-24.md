# AUDIT-5 verification, 2026-10-24: the 12 proposed promotions

AUDIT-5 re-graded 34 contested rows against `lanes/audit-5-bar.md` and proposed
12 promotions in `audit-5-proposals.json`. It did not apply them. Each proposal
went through two independent checks, run by sessions that did not write it.
Only a proposal that passed both was promoted.

1. **Mechanical re-run.** The parent session ran this check in its own checkout.
   For each proposal it applied the exact `old` to `new` replacement and
   asserted the `old` string occurred once. It ran the named test filter with
   `--exact` on clean code, then on mutated code, then restored the file with
   `git checkout` and confirmed `git diff --quiet`. It read every mutated
   failure to tell a panic from a compile error:
   - Eleven failures are panics at the named assertion.
   - EVENT-5087 and EVENT-5088 both panic at `access.rs:62`, the `refusal()`
     helper's `expect_err`. That is the refusal the mutation removed, so it
     counts as a panic at the named assertion.
   - REFLEX-5029 ended in SIGABRT from `decision_loop.rs`'s `Watchdog`. That is
     a bounded failure.

   **All 12 reproduced.**
2. **Two read-only skeptics per proposal**, in a 24-agent workflow (`wf_c8a909a1-ff4`).
   Both read only from `git show 9599237c:` and were told to refute the
   proposal, defaulting to refuted when unsure:
   - One lens checked the default-path and configuration claims (bar parts 2
     and 3).
   - The other read the requirement text clause by clause (bar parts 1 and 4).

   A promotion lands only if neither lens refutes it.

## Result: 4 promoted, 8 refuted

| Row | Mechanical | default-path lens | requirement-text lens | Outcome |
|---|---|---|---|---|
| R12-AGENCY-6089 | reproduces | holds | holds | **COMPLETE**; verification now names `qip-agency --test causal` |
| R12-AMBIENT-1026 | reproduces | holds | holds | **COMPLETE** |
| R12-LEDGER-5016 | reproduces | holds | holds | **COMPLETE**; qip-ledgerd's own apply path |
| R12-REASON-3060 | reproduces | holds | holds | **COMPLETE**; verification now names `qip-world-model --test inference` |
| R12-AMBIENT-1144 | reproduces | holds | refuted | PARTIAL: no test asserts that the scheduled, unprompted loop journals a named wake |
| R12-CICD-10203 | reproduces | refuted | refuted | PARTIAL: `infrastructure/fleet/main.tf`'s DOCKER repository has mutable tags |
| R12-CICD-8031 | reproduces | refuted | refuted | PARTIAL: the tree scan's private-key pattern never runs, and `*/tests/*` is excluded |
| R12-CONTRACT-7083 | reproduces | holds | refuted | PARTIAL: the requirement's round-trip half has no test |
| R12-COVERAGE-5107 | reproduces | refuted | refuted | PARTIAL: `SimulatedExchange::list_synthetic` lists without a `CreationAdmission`, and the edge node calls it |
| R12-EVENT-5087 | reproduces | refuted | refuted | PARTIAL: the requirement's `kind` is capability, so bar part 3 does not apply, and the path is gated off |
| R12-EVENT-5088 | reproduces | refuted | refuted | PARTIAL: the market-factory seeding path never checks `policy.kind` |
| R12-REFLEX-5029 | reproduces | refuted | refuted | PARTIAL: `QIP_MESH_PEER` is unset everywhere, and the requirement's sameness proof does not hold on the shipped path |

Each refuted row's `gap` now carries the skeptic's finding and what closes it.

The mechanical check alone would have promoted all 12. A test that fails under a
mutation proves the line is tested. It does not prove the requirement is met.

## Three findings worth an owner's attention

The skeptics found them, and the parent session confirmed each in the files.

1. **The secret scan's tree mode cannot match a private key.**
   - `scripts/check-secrets.sh` passes its patterns to `git grep -n -I -E "$pattern"` without `-e`.
   - `'-----BEGIN [A-Z ]*PRIVATE KEY-----'` therefore parses as an option: `git grep` prints "unknown option" and exits 129.
   - `2>/dev/null` and the surrounding `if` turn that into "no match".
   - With `-e`, the same query finds `infrastructure/gitops/bootstrap/README.md:58`, a placeholder.
   - The `--history` mode uses `grep -E` and does catch a key, and ci runs both modes. So a committed key outside `tests/` is caught in ci, but `make check`'s tree scan is blind to one.
   - The fix is `docs/blueprint/lanes/secrets-scan-protected.patch`, and it needs a person's review because `scripts/check-*` is protected.
   - The README placeholder no longer carries a key header. It is replaced in the same commit as this report, and the path is not protected, so the fixed scan stays clean. The README change has to land first: the README hunk itself contains the header, so it could not go in the patch without tripping the history scan.
   - Separately, `EXCLUDE_PATHS` skips `*/tests/*` in both modes. Narrowing that is an owner decision.
2. **A second container registry has mutable tags.** `infrastructure/fleet/main.tf`'s `google_artifact_registry_repository.fleet` (format DOCKER, ADR 0102's worker image) declares no `docker_config { immutable_tags = true }`.
3. **A market can be listed without the creation guard.** `qip_brokers::exchange::SimulatedExchange::list` and `list_synthetic` are public and take no `CreationAdmission`. `qip-edge-node/src/gateway.rs` calls `list_synthetic`. This is paper-only: the exchange is the simulator. Even so, the "created only where venue, product and legal governance permit" control is not structural.

## The rate, reconciled

AUDIT-5 downgraded none of the 34. Four of its 12 promotions survived. Every
promoted row is in AUDIT-4's sample, so main's sampled overclaims fall from 42
to **38 of 180: 21.1%, 95% Wilson interval 15.8% to 27.6%**. Applied to the 331
COMPLETE rows on main that no audit sampled, that predicts about 70 more
overclaims (52 to 92).

The register's COMPLETE count is therefore an upper bound. Its expected
verified value is that count minus about 70.
