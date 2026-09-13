# data/

The data domain's home at the repository's top level (ADR 0016). It holds
what is *data about the world*, as distinct from the code that processes it:

| Path | What belongs there |
|---|---|
| `data/local/` | State written by local development runs — journals, event logs, dev identity stores. Git-ignored; safe to delete between runs. |
| `data/datasets/` | Committed reference datasets. One exists: `universe.json`, the instrument catalogue every central composition root reads from `QIP_UNIVERSE_PATH` and refuses to start without. It names instruments — object id, asset class, venue, sector, country, currency, price, licensing posture — and never a data source: every market fact still arrives through a connector at run time. Its instruments are synthetic and mirror the synthetic exchange's, so a deployment on the synthetic feed sizes into real exposure buckets; its SHA-256 is journaled at assembly so a run says which catalogue it ran against. |
| `data/catalogs/` | Licensed-source catalogues exported from `qip-data-finder`, when the desk needs one reviewable outside the process. |
| `data/risk-limits/` | Signed limit sets (ADR 0061): the artefact two operators' signatures on a recalibration proposal emit — the running `LimitSet` with exactly one bound replaced — committed here, named by `risk_limits_file`, and mounted on all three central roots as `QIP_RISK_LIMITS_PATH`. Every root validates one at boot against the shipped set: a file may move a bound and never remove a control, and a file that does not validate stops the process. None exists yet, because no recalibration has been signed; see `docs/operations/recalibrating-a-limit.md`. |

Two things deliberately do **not** live here:

- **Connector fixtures and manifests** (for example
  `backend/crates/services/qip-market-ingestion/src/connectors/fixtures/`).
  Those are inputs to a specific crate's tests, versioned and reviewed with
  the code that reads them. Moving a fixture away from its test turns every
  fixture edit into a cross-domain change and helps nobody.
- **Production data.** The production record is the hash-chained event log on
  its provisioned disk, described in `docs/operations/disaster-recovery.md`.
  Nothing under `data/` is ever a production store.
