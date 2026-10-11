-- ADR 0111 §1: the managed log P0 and P1 are committed to before a producer
-- is answered. Read by `qip-fabricd` (`src/spanner.rs`) when
-- QIP_EVENT_FABRIC_MANAGED_LOG=spanner; nothing here is applied by any
-- workflow yet, and no environment sets that variable.
--
-- Regional instance only: a regional configuration's three read-write
-- replicas in three zones are what `replication_factor: 3` on a
-- `managed_majority` stream claims (MANAGED_REPLICATION_FACTOR). Whether the
-- tables live in a multi-region instance is ADR 0113's decision, not this
-- file's.

-- Leadership, one row per partition per region. Every produce transaction
-- reads this row and commits only if its epoch is still the holder's, so a
-- deposed leader's write fails at commit: fencing by the database.
CREATE TABLE fabric_leases (
  region       STRING(64)  NOT NULL,
  stream       STRING(255) NOT NULL,
  partition_id INT64       NOT NULL,
  holder       STRING(255) NOT NULL,
  epoch        INT64       NOT NULL,
  expires_at   TIMESTAMP   NOT NULL,
) PRIMARY KEY (region, stream, partition_id);

-- Committed batches, dense per partition. `batch` is the batch exactly as the
-- broker stamped and encoded it; `batch_hash` is its SHA-256 and
-- `previous_batch_hash` the link to the batch before it (ADR 0104), so a
-- reader re-proves both before it trusts a row.
CREATE TABLE fabric_batches (
  region              STRING(64)  NOT NULL,
  stream              STRING(255) NOT NULL,
  partition_id        INT64       NOT NULL,
  batch_offset        INT64       NOT NULL,
  lease_epoch         INT64       NOT NULL,
  batch_hash          STRING(64)  NOT NULL,
  previous_batch_hash STRING(64),
  batch               BYTES(MAX)  NOT NULL,
  committed_at        TIMESTAMP   NOT NULL OPTIONS (allow_commit_timestamp = true),
) PRIMARY KEY (region, stream, partition_id, batch_offset);

-- Consumer-group checkpoints on managed partitions, so a consumer resumes
-- from the same offset whichever instance leads after a failover (RES-7112).
CREATE TABLE fabric_checkpoints (
  region           STRING(64)  NOT NULL,
  consumer_group   STRING(255) NOT NULL,
  stream           STRING(255) NOT NULL,
  partition_id     INT64       NOT NULL,
  committed_offset INT64       NOT NULL,
) PRIMARY KEY (region, consumer_group, stream, partition_id);
