#!/usr/bin/env node
/**
 * A stand-in process for the harness's own tests: serves `/health` (200)
 * and `/metrics` with `fake_records_total`, a count that rises every tick
 * and is persisted to `FAKE_STATE` every `FAKE_PERSIST_EVERY` ticks — so a
 * kill between persists loses records, the way a writer that acknowledges
 * before it syncs would. On start it resumes from the persisted count.
 * `FAKE_UNHEALTHY_FILE`, when it exists, turns `/health` into 503.
 *
 * Test fixture only. Not a model of any platform binary.
 */
import { createServer } from "node:http";
import { existsSync, readFileSync, renameSync, writeFileSync } from "node:fs";

const port = Number(process.env.FAKE_PORT);
const state = process.env.FAKE_STATE;
const every = Number(process.env.FAKE_PERSIST_EVERY ?? "1");
let records = state && existsSync(state) ? Number(readFileSync(state, "utf8")) || 0 : 0;
let ticks = 0;

setInterval(() => {
  records += 1;
  ticks += 1;
  // Write-then-rename, so a SIGKILL mid-write leaves the previous count
  // rather than a truncated file. A plain overwrite made the "persisted
  // everything" test flaky: one kill in three landed inside the write and
  // the restarted fake resumed from a torn number.
  if (state && ticks % every === 0) {
    writeFileSync(`${state}.next`, String(records));
    renameSync(`${state}.next`, state);
  }
}, 20);

createServer((request, response) => {
  if (request.url === "/metrics") {
    response.writeHead(200, { "content-type": "text/plain" });
    response.end(`# TYPE fake_records_total counter\nfake_records_total{kind="a"} ${records}\nfake_empty_total 0\n`);
    return;
  }
  const unhealthy = process.env.FAKE_UNHEALTHY_FILE && existsSync(process.env.FAKE_UNHEALTHY_FILE);
  response.writeHead(unhealthy ? 503 : 200, { "content-type": "application/json" });
  response.end(JSON.stringify({ status: unhealthy ? "not_ready" : "ready" }));
}).listen(port, "127.0.0.1");
