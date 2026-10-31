# ADR 0240: The warehouse is fed by batch load from Cloud Storage, and no shipped path streams rows

- **Status**: **Accepted**, 2026-10-31, decided under the owner's 2026-10-31
  delegation to the parent, recorded in
  [`docs/blueprint/owner-decisions-2026-10-31.md`](../blueprint/owner-decisions-2026-10-31.md),
  and ruled for this row in that day's decision 10. Conservative and
  paper-only: it changes how derived research rows reach a table, switches
  nothing on, and adds no risk limit, threshold or capital budget.
- **Date**: 2026-10-31
- **Amends**: [ADR 0182](0182-a-simulation-lesson-is-a-promoted-claim-an-embedding-is-a-fifth-model-form-and-event-arrival-and-the-warehouse-write-path-stay-as-decided.md)
  decision 4 ("the warehouse write path stays `insertAll`").
- **Rows**: R12-DATA-8127. R12-DATA-9054 and R12-DATA-10036 build on it.

## Context

R12-DATA-8127 asks for the research warehouse to be fed from the platform's
own data without a streaming path in front of the producers. ADR 0182
decision 4 kept BigQuery's `insertAll` streaming API as the write path,
because the Storage Write API the requirement names needs a gRPC stack no ADR
admits. That left the one shipped writer, the deep brain's `OutcomeSink`,
calling `insertAll` from its cycle.

The ruling of 2026-10-31 weighed two other ways to feed the warehouse. A
fabric sink in `qip-fabricd` would read every stream, but `qip-fabricd` has
no boot image yet, so a sink there reaches no deployed process. A batch load
from Cloud Storage needs only the two storage switches already declared
(`enable_cloud_storage`, `enable_bigquery`), the Cloud Storage adapter the
platform already has, and one more BigQuery REST call. Streaming inserts are
also the BigQuery write path with the weakest failure story: a request that
answers 200 can still have rejected rows, and rows streamed are billed and
held in a streaming buffer that a load does not have.

## Decision

1. **The warehouse is fed by batch load.** A writer stages its rows as one
   newline-delimited JSON object in Cloud Storage and asks BigQuery to load
   that object into the table (`jobs.insert` with a `load` configuration:
   `NEWLINE_DELIMITED_JSON`, `WRITE_APPEND`, `CREATE_NEVER`, no bad records
   tolerated and no unknown columns ignored, so a schema mistake fails the
   job rather than dropping data).
2. **The job id is derived from the batch, so a retry is the same job.** A
   writer that lost the response to a load asks again with the same id;
   BigQuery answers that the job exists, and the writer reads its state
   instead of loading the rows twice.
3. **A batch counts as written only when its job reports done without an
   error.** Until then its rows stay pending and are not offered again; a job
   that fails releases them to be staged in a new batch. A load that was
   accepted is not a load that happened.
4. **Staging is regional and expires.** The staged objects go under
   `warehouse-staging/` in the regional segment bucket, which sits in the
   dataset's region (a load reads from a co-located bucket), and a lifecycle
   rule deletes that prefix after seven days, the bucket's own unlocked
   retention floor. A staged object is a transport copy, not a record: the
   table and the event log are the records.
5. **No shipped path streams rows.** `OutcomeSink` stops calling `insertAll`.
   `BigQueryWarehouse::insert` stays in `qip-mesh` for its own tests, and
   `qip-acceptance` refuses any shipped source that builds an `InsertRow`.
6. **It stays off until both storage switches are on.** The deep brain gets
   the warehouse's project, dataset, staging bucket, the proxy endpoint and
   the metadata-server credential only when `enable_bigquery` and
   `enable_cloud_storage` are both true, which is no environment today. A
   half-configured target still stops the process at start-up.

## Consequences

- R12-DATA-8127 is graded on a staged batch load. It stays PARTIAL until the
  two storage switches are flipped (protected, billed) and a real load runs.
- ADR 0182 decision 4's open item, a fabricd consumer driving the warehouse,
  is withdrawn: the writer is the deep brain's sink, through Cloud Storage.
- Evolution-trial outcomes (R12-DATA-9054) and the backtest and tournament
  tables (R12-DATA-10036) are loaded through the same staged path.

## What it costs

- Latency: a grade reaches the table a load job after it is staged, not
  seconds after it is graded. Research queries run over history, so minutes
  are acceptable; nothing that decides reads the warehouse.
- One more object write and one more API call per batch, and a job to poll
  on the next cycle.

## What would make this wrong

- If a consumer needed rows in the table within seconds of a grade, a
  streaming path would be needed again, and that would be a new ADR, most
  likely admitting the Storage Write API's gRPC client through ADR 0109.
- If the dataset moved to a location the segment bucket does not share, the
  staging bucket would have to move with it.
