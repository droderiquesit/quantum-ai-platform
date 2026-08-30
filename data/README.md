# data/

The data domain's home at the repository's top level (ADR 0016). It holds
what is *data about the world*, as distinct from the code that processes it:

| Path | What belongs there |
|---|---|
| `data/local/` | State written by local development runs — journals, event logs, dev identity stores. Git-ignored; safe to delete between runs. |
| `data/datasets/` | Committed reference datasets, when one earns a place in the repository. None exist yet: every market fact the platform uses today arrives through a connector at run time, and inventing a committed dataset would be a second source of truth for facts the event log already holds. |
| `data/catalogs/` | Licensed-source catalogues exported from `qip-data-finder`, when the desk needs one reviewable outside the process. |

Two things deliberately do **not** live here:

- **Connector fixtures and manifests** (for example
  `backend/crates/services/qip-market-ingestion/src/connectors/fixtures/`).
  Those are inputs to a specific crate's tests, versioned and reviewed with
  the code that reads them. Moving a fixture away from its test turns every
  fixture edit into a cross-domain change and helps nobody.
- **Production data.** The production record is the hash-chained event log on
  its provisioned disk, described in `docs/operations/disaster-recovery.md`.
  Nothing under `data/` is ever a production store.
