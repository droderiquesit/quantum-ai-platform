/**
 * The fleet dispatcher's tests: `node --test scripts/fleet/dispatch.test.mjs`.
 *
 * The dispatcher decides whether forty tasks start. Each test names the
 * refusal it holds, and every refusal is checked for the thing it must also
 * not have done: produced a command.
 *
 * No test here runs `gcloud` or reaches a bucket. `exec` and the store are
 * handed in, and the one test of `--execute` records what would have run.
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { MAX_TASKS, dispatch, planDispatch, settingsFrom } from "./dispatch.mjs";

const ARGV = [
  "--packets", "/packets",
  "--run", "run-1",
  "--project", "algorik-platform-dev",
  "--region", "us-east4",
  "--bucket", "algorik-platform-dev-fleet",
  "--daily-usd-ceiling", "25",
  "--slot-usd-cap", "0.5",
];
const settings = settingsFrom(ARGV);

const packet = (overrides = {}) => ({
  packet_id: "scout-0001",
  role: "scout",
  model: "google/gemini-2.5-flash-lite",
  why_this_tier: "an inventory a grep can check, so the cheapest tier",
  task: "List the functions this file declares.",
  context: "fn main() {}",
  acceptance: "one name per line",
  escalate_if: "the file is not Rust",
  paths: [],
  source_paths: ["backend/crates/libs/qip-core/src/lib.rs"],
  max_input_tokens: 2000,
  max_output_tokens: 500,
  ...overrides,
});

/** `count` valid packets with distinct ids, each with a worst case of 400 micro-dollars. */
const packets = (count, overrides = {}) =>
  Array.from({ length: count }, (_, index) => ({
    file: `/packets/${String(index).padStart(2, "0")}.json`,
    packet: packet({ packet_id: `scout-${index}`, ...overrides }),
  }));

const facts = (overrides = {}) => ({ settings, packets: packets(2), halted: false, runHasPackets: false, dayTotalMicro: 0, ...overrides });

test("a clean directory yields one upload per packet in index order and one execute naming the task count and the run, and nothing else", () => {
  assert.deepEqual(settings.problems, []);
  const plan = planDispatch(facts());
  assert.deepEqual(plan.problems, []);
  assert.equal(plan.worstMicro, 800);
  assert.deepEqual(plan.commands, [
    ["gcloud", "storage", "cp", "--no-clobber", "/packets/00.json", "gs://algorik-platform-dev-fleet/packets/run-1/0.json"],
    ["gcloud", "storage", "cp", "--no-clobber", "/packets/01.json", "gs://algorik-platform-dev-fleet/packets/run-1/1.json"],
    ["gcloud", "run", "jobs", "execute", "fleet", "--project", "algorik-platform-dev", "--region", "us-east4", "--tasks", "2", "--update-env-vars", "FLEET_RUN=run-1"],
  ]);
});

test("a halted fleet is refused at dispatch and yields no command", () => {
  // Premise: the same facts without the halt dispatch.
  assert.equal(planDispatch(facts()).commands.length, 3);
  const plan = planDispatch(facts({ halted: true }));
  assert.deepEqual(plan.commands, []);
  assert.equal(plan.problems.length, 1);
  assert.ok(plan.problems[0].startsWith("gs://algorik-platform-dev-fleet/HALT exists"), plan.problems[0]);
  assert.ok(plan.problems[0].includes("deleting the object resumes dispatch"), "the refusal does not say what to do");
});

test("the summed worst case of the whole dispatch, not each packet's own, is held to the eighty percent line", () => {
  // The line is 20.000000 and each packet's worst case is 400. With
  // 19.999200 spent, either packet alone fits (19.999600) and so do both
  // together, exactly (20.000000); one micro-dollar more and the pair does
  // not, while each alone still would. A check made packet by packet admits
  // this dispatch.
  assert.deepEqual(planDispatch(facts({ dayTotalMicro: 19_999_200 })).problems, []);
  assert.deepEqual(planDispatch(facts({ dayTotalMicro: 19_999_201, packets: packets(1) })).problems, []);
  const plan = planDispatch(facts({ dayTotalMicro: 19_999_201 }));
  assert.deepEqual(plan.commands, []);
  assert.equal(plan.problems.length, 1);
  assert.ok(plan.problems[0].startsWith("for the 2 packet(s) together, the day's ledger total of 19.999201 USD plus a worst case of 0.000800 USD"), plan.problems[0]);
});

test("forty packets are dispatched and forty-one are refused, so no execution started here runs more than forty tasks", () => {
  assert.equal(MAX_TASKS, 40);
  const atCeiling = planDispatch(facts({ packets: packets(40) }));
  assert.deepEqual(atCeiling.problems, []);
  assert.deepEqual(atCeiling.commands.at(-1).slice(-4), ["--tasks", "40", "--update-env-vars", "FLEET_RUN=run-1"]);
  const over = planDispatch(facts({ packets: packets(41) }));
  assert.deepEqual(over.commands, []);
  assert.equal(over.problems.length, 1);
  assert.ok(over.problems[0].startsWith("41 packets is more than 40"), over.problems[0]);
});

test("one packet the worker would refuse refuses the whole dispatch and names its file", () => {
  // Each of these is the worker's own refusal, reached through the same
  // function, so a packet admitted here is not refused in the cloud.
  const cases = [
    [{ role: "architect" }, "has no row in ADR 0102's roster"],
    [{ source_paths: ["backend/crates/services/qip-risk-engine/src/lib.rs"] }, "(policy §4 gate 6)"],
    [{ source_paths: undefined }, "policy §4 gate 6 cannot classify"],
    [{ model: "openai/gpt-oss-120b-maas" }, "has no row in the price table"],
    [{ max_input_tokens: 5_000_000 }, "above the slot cap"],
    [{ max_output_tokens: "500" }, "must be a positive whole number"],
  ];
  for (const [change, expected] of cases) {
    const mixed = packets(3);
    mixed[1] = { file: "/packets/01.json", packet: packet({ packet_id: "scout-1", ...change }) };
    const plan = planDispatch(facts({ packets: mixed }));
    assert.deepEqual(plan.commands, [], `a dispatch holding ${JSON.stringify(change)} produced commands`);
    assert.equal(plan.problems.length, 1, JSON.stringify(plan.problems));
    assert.ok(plan.problems[0].startsWith("/packets/01.json: ") && plan.problems[0].includes(expected), plan.problems[0]);
  }
  // A file that was not JSON at all arrives as no packet.
  const unreadable = planDispatch(facts({ packets: [{ file: "/packets/00.json", packet: undefined }] }));
  assert.deepEqual(unreadable.problems, ["/packets/00.json: the packet is not a JSON object"]);
});

test("two packets with one packet_id are refused, because the ledger could not tell them apart", () => {
  const twins = packets(2, { packet_id: "scout-same" });
  const plan = planDispatch(facts({ packets: twins }));
  assert.deepEqual(plan.commands, []);
  assert.deepEqual(plan.problems.length, 1);
  assert.ok(plan.problems[0].startsWith("/packets/01.json: packet_id 'scout-same' is also /packets/00.json's"), plan.problems[0]);
});

test("an empty directory and a run id that already holds packets are refused", () => {
  const empty = planDispatch(facts({ packets: [] }));
  assert.deepEqual(empty.commands, []);
  assert.ok(empty.problems[0].includes("nothing to dispatch"), empty.problems[0]);
  const reused = planDispatch(facts({ runHasPackets: true }));
  assert.deepEqual(reused.commands, []);
  assert.ok(reused.problems[0].startsWith("gs://algorik-platform-dev-fleet/packets/run-1/ already holds packets"), reused.problems[0]);
});

test("a value that could become a second argument or a second variable is refused before it reaches a command", () => {
  // Premise: the command line every case is derived from is accepted.
  assert.deepEqual(settings.problems, []);
  assert.deepEqual(
    { run: settings.run, project: settings.project, region: settings.region, bucket: settings.bucket, ceilingMicro: settings.ceilingMicro, slotCapMicro: settings.slotCapMicro, execute: settings.execute },
    { run: "run-1", project: "algorik-platform-dev", region: "us-east4", bucket: "algorik-platform-dev-fleet", ceilingMicro: 25_000_000, slotCapMicro: 500_000, execute: false },
  );
  const withFlag = (flag, value) => ARGV.map((argument, at) => (ARGV[at - 1] === flag ? value : argument));
  const cases = [
    // The one that matters most: gcloud splits --update-env-vars on commas.
    ["--run", "a,FLEET_DAILY_USD_CEILING=1000"],
    ["--run", "Run 1"],
    ["--run", "a/b"],
    ["--project", "algorik-platform-dev --impersonate-service-account=x"],
    ["--project", "Algorik"],
    ["--region", "us-east4;id"],
    ["--bucket", "bucket/HALT"],
    ["--daily-usd-ceiling", "25usd"],
    ["--daily-usd-ceiling", "0"],
    ["--slot-usd-cap", "1e3"],
  ];
  for (const [flag, value] of cases) {
    const { problems } = settingsFrom(withFlag(flag, value));
    assert.equal(problems.length, 1, `${flag} ${JSON.stringify(value)} gave ${JSON.stringify(problems)}`);
    assert.ok(problems[0].startsWith(`${flag} is '${value}'`), problems[0]);
  }
  // Nothing is defaulted: each flag left out is named.
  for (const flag of ["--packets", "--run", "--project", "--region", "--bucket", "--daily-usd-ceiling", "--slot-usd-cap"]) {
    const without = ARGV.filter((argument, at) => argument !== flag && ARGV[at - 1] !== flag);
    assert.deepEqual(settingsFrom(without).problems.map((p) => p.slice(0, flag.length + 11)), [`${flag} is missing`]);
  }
  assert.equal(settingsFrom([...ARGV, "--force"]).problems.length, 1, "an unknown flag was accepted");
});

// --- the command line, with gcloud and the bucket handed in -----------------

/** A directory of packet files, a recording `exec`, and a store with the given contents. */
async function commandLine(extraArguments, { objects = {}, files } = {}) {
  // A space and an apostrophe in the path, so that a printed command which
  // is not quoted for a shell is visibly a different command.
  const directory = mkdtempSync(join(tmpdir(), "fleet's dispatch-"));
  try {
    const written = files ?? { "b.json": packet({ packet_id: "scout-b" }), "a.json": packet({ packet_id: "scout-a" }), "notes.txt": "not a packet" };
    for (const [name, content] of Object.entries(written)) {
      writeFileSync(join(directory, name), typeof content === "string" ? content : JSON.stringify(content));
    }
    const ran = [];
    const said = [];
    const refused = [];
    const tokens = [];
    const code = await dispatch({
      argv: [...ARGV.map((argument, at) => (ARGV[at - 1] === "--packets" ? directory : argument)), ...extraArguments],
      exec: (command, options) => {
        ran.push(command);
        return options?.capture ? `${SESSION}\n` : "";
      },
      storeFor: (bucket, token) => {
        tokens.push([bucket, token]);
        return {
          exists: async (name) => Object.hasOwn(objects, name),
          list: async (prefix) => Object.keys(objects).filter((name) => name.startsWith(prefix)),
          read: async (name) => objects[name] ?? null,
        };
      },
      now: () => new Date("2026-10-04T12:00:00.000Z"),
      say: (line) => said.push(line),
      log: (line) => refused.push(line),
    });
    return { code, ran, said, refused, tokens, directory };
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

const TOKEN_COMMAND = ["gcloud", "auth", "print-access-token"];
/** What the recording `exec` answers the token request with. Distinctive, so its absence from the output means something. */
const SESSION = "owner-session-0000";

test("without --execute the commands are printed in a form a shell can run and the only thing run is the owner's token request", async () => {
  const { code, ran, said, refused, tokens, directory } = await commandLine([]);
  assert.equal(code, 0, refused.join("\n"));
  assert.deepEqual(ran, [TOKEN_COMMAND], "something other than the token request was run");
  assert.deepEqual(tokens, [["algorik-platform-dev-fleet", SESSION]]);
  // The summary is in dollars a person can read against the ceiling.
  assert.equal(said[0], "# run run-1: 2 packet(s), summed worst case 0.000800 USD; the 2026-10-04 ledger stands at 0.000000 USD against a 25.000000 USD ceiling.");
  // Files in name order are the task indexes; the .txt beside them is not a packet.
  const printed = said.filter((line) => !line.startsWith("#"));
  // The directory's name holds a space and an apostrophe; in single quotes
  // the apostrophe is written '\'' and the space needs nothing more.
  assert.ok(directory.includes("fleet's dispatch-"), directory);
  const shown = `'${directory.replace("'", "'\\''")}`;
  assert.deepEqual(printed, [
    `gcloud storage cp --no-clobber ${shown}/a.json' gs://algorik-platform-dev-fleet/packets/run-1/0.json`,
    `gcloud storage cp --no-clobber ${shown}/b.json' gs://algorik-platform-dev-fleet/packets/run-1/1.json`,
    "gcloud run jobs execute fleet --project algorik-platform-dev --region us-east4 --tasks 2 --update-env-vars FLEET_RUN=run-1",
  ]);
  assert.ok(said.at(-1).startsWith("# nothing was run."), said.at(-1));
  // The token is the owner's session and is never printed.
  assert.ok(![...said, ...refused].some((line) => line.includes(SESSION)), "the access token was printed");
});

test("with --execute exactly the printed commands run, in order, after the token request", async () => {
  const { code, ran, said, directory } = await commandLine(["--execute"]);
  assert.equal(code, 0);
  assert.deepEqual(ran, [
    TOKEN_COMMAND,
    ["gcloud", "storage", "cp", "--no-clobber", `${directory}/a.json`, "gs://algorik-platform-dev-fleet/packets/run-1/0.json"],
    ["gcloud", "storage", "cp", "--no-clobber", `${directory}/b.json`, "gs://algorik-platform-dev-fleet/packets/run-1/1.json"],
    ["gcloud", "run", "jobs", "execute", "fleet", "--project", "algorik-platform-dev", "--region", "us-east4", "--tasks", "2", "--update-env-vars", "FLEET_RUN=run-1"],
  ]);
  // One line was printed for each command that ran, before it ran.
  assert.equal(said.filter((line) => !line.startsWith("#")).length, ran.length - 1);
});

test("a refused dispatch runs nothing even under --execute, and says so", async () => {
  const day = "ledger/2026-10-04/";
  const cases = [
    [{ objects: { HALT: "" } }, "HALT exists"],
    [{ objects: { "packets/run-1/0.json": "{}" } }, "already holds packets"],
    [{ objects: { [`${day}x-0.json`]: JSON.stringify({ cost_micro_usd: 19_999_201 }) } }, "would pass 80%"],
    [{ files: { "a.json": "{not json" } }, "the packet is not a JSON object"],
    [{ files: { "a.json": packet({ source_paths: ["backend/crates/edge/qip-edge/src/cell.rs"] }) } }, "(policy §4 gate 6)"],
  ];
  for (const [world, expected] of cases) {
    const { code, ran, said, refused } = await commandLine(["--execute"], world);
    assert.equal(code, 1, expected);
    assert.deepEqual(ran, [TOKEN_COMMAND], `a command ran for a dispatch refused with '${expected}'`);
    assert.deepEqual(said, [], `a command was printed for a dispatch refused with '${expected}'`);
    assert.ok(refused[0].startsWith("refused: ") && refused[0].includes(expected), refused.join("\n"));
    assert.equal(refused.at(-1), "nothing was uploaded and nothing was started.");
  }
  // A command line that is itself refused does not even ask for a token.
  const bad = await dispatch({ argv: ["--run", "a,b=c"], exec: () => assert.fail("gcloud was run"), say: () => {}, log: () => {} });
  assert.equal(bad, 1);
});
