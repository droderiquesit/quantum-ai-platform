/**
 * The fleet dispatcher (ADR 0102), for the owner's desktop.
 *
 *   node scripts/fleet/dispatch.mjs --packets <dir> --run <id> \
 *     --project <project-id> --region <region> --bucket <fleet-bucket> \
 *     --daily-usd-ceiling 25 --slot-usd-cap 0.5 [--execute]
 *
 * It judges a directory of packet files with the worker's own functions and
 * then **prints** the commands that would upload them and start the Job. It
 * runs them only under `--execute`. Printing is the default because starting
 * forty tasks that send source to a provider is a decision, and a decision is
 * something a person reads before it happens.
 *
 * ## What it refuses
 *
 * - `gs://<bucket>/HALT` exists, or whether it exists cannot be established.
 * - Any packet the worker's validator refuses, has no price row for, or whose
 *   worst case is over the slot cap. One bad packet refuses the dispatch:
 *   starting thirty-nine tasks beside one that will fail is thirty-nine tasks
 *   nobody decided to run on their own.
 * - The day's ledger total plus the **summed** worst case of the whole
 *   dispatch would pass 80% of the daily ceiling. Each worker checks its own
 *   packet against the day as well, but forty workers starting together all
 *   read the same total, so only the sum taken here sees the dispatch.
 * - More than forty packets. `gcloud run jobs execute` has no parallelism
 *   flag: the rung is the Job's own setting, which Terraform refuses above
 *   forty. Capping the task count here means no execution this program starts
 *   can run more than forty at once whatever the Job says.
 *   ponytail: per execution only. It does not ask whether an earlier
 *   execution is still running, so two overlapping dispatches can exceed
 *   forty between them; list running executions here before the ladder
 *   reaches a rung where one dispatch no longer waits for the last.
 * - A run id that already holds packets, or two packets with one `packet_id`.
 *
 * ## What it reads the bucket with
 *
 * The owner's own `gcloud` session: `gcloud auth print-access-token`, held in
 * memory for the three reads and never printed. No key file, no stored
 * credential. Every value that reaches a printed command is validated first,
 * because `--update-env-vars FLEET_RUN=a,FLEET_DAILY_USD_CEILING=1000` is
 * what an unvalidated run id is.
 *
 * Dev tooling. Node built-ins only.
 */
import { execFileSync } from "node:child_process";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { GCP_PROJECT_ID } from "../model-gateway.mjs";
import { BUCKET_NAME, PRICES, RUN_ID, assess, bucketStore, ceilingRefusal, dayTotalMicroUsd, usd, usdToMicro } from "./worker.mjs";

/** The owner's ceiling on tasks at once (ADR 0102 decision 1), held here as tasks per dispatch. */
export const MAX_TASKS = 40;

const REGION = /^[a-z]+-[a-z]+[0-9]$/;

const OPTIONS = {
  packets: { type: "string" },
  run: { type: "string" },
  project: { type: "string" },
  region: { type: "string" },
  bucket: { type: "string" },
  "daily-usd-ceiling": { type: "string" },
  "slot-usd-cap": { type: "string" },
  execute: { type: "boolean", default: false },
};

/** The command line, validated. Nothing has a default: a ceiling the operator did not state is a guess. */
export function settingsFrom(argv) {
  const problems = [];
  let values = {};
  try {
    ({ values } = parseArgs({ args: argv, options: OPTIONS }));
  } catch (cause) {
    return { problems: [String(cause.message)] };
  }
  const take = (name, pattern, what) => {
    const value = values[name];
    if (typeof value === "string" && pattern.test(value)) return value;
    problems.push(`--${name} is ${value === undefined ? "missing" : `'${value}'`}; it must be ${what}`);
    return undefined;
  };
  const money = (name) => {
    const micro = usdToMicro(values[name]);
    if (micro) return micro;
    problems.push(`--${name} is ${values[name] === undefined ? "missing" : `'${values[name]}'`}; it must be a positive USD amount as plain decimal text, such as 25 or 0.5`);
    return undefined;
  };
  return {
    directory: take("packets", /./, "the directory holding the packet .json files"),
    run: take("run", RUN_ID, "a run id of lower-case letters, digits and hyphens that no earlier dispatch used"),
    project: take("project", GCP_PROJECT_ID, "the Google Cloud project id the Job runs in"),
    region: take("region", REGION, "the Job's region, such as us-east4"),
    bucket: take("bucket", BUCKET_NAME, "the fleet bucket's name (terraform output bucket_name)"),
    ceilingMicro: money("daily-usd-ceiling"),
    slotCapMicro: money("slot-usd-cap"),
    execute: values.execute,
    problems,
  };
}

/**
 * The dispatch, decided from facts already gathered: the commands, or every
 * reason there are none.
 *
 * `packets` is `[{ file, packet }]` in task-index order. `commands` are
 * argument vectors, so `--execute` never passes anything through a shell.
 */
export function planDispatch({ settings, packets, halted, runHasPackets, dayTotalMicro, prices = PRICES }) {
  const { run, project, region, bucket, ceilingMicro, slotCapMicro } = settings;
  const problems = [];
  if (halted) {
    problems.push(`gs://${bucket}/HALT exists, so the fleet is halted and nothing is dispatched. Find out who halted it and why; deleting the object resumes dispatch`);
  }
  if (runHasPackets) {
    problems.push(`gs://${bucket}/packets/${run}/ already holds packets. A run id is used once: choose a new --run`);
  }
  if (packets.length === 0) problems.push("the directory holds no .json packet; there is nothing to dispatch");
  if (packets.length > MAX_TASKS) {
    problems.push(`${packets.length} packets is more than ${MAX_TASKS}, the owner's ceiling on tasks at once (ADR 0102 decision 1). Split the directory and dispatch it as separate runs, one after another`);
  }

  let worstMicro = 0;
  const owners = new Map();
  for (const { file, packet } of packets) {
    const judged = assess(packet, { prices, slotCapMicro });
    for (const problem of judged.problems) problems.push(`${file}: ${problem}`);
    if (judged.problems.length > 0) continue;
    worstMicro += judged.worstMicro;
    if (owners.has(packet.packet_id)) {
      problems.push(`${file}: packet_id '${packet.packet_id}' is also ${owners.get(packet.packet_id)}'s, and the ledger could not tell the two apart. Give each packet its own id`);
    }
    owners.set(packet.packet_id, file);
  }
  if (problems.length === 0) {
    const over = ceilingRefusal({ dayTotalMicro, worstMicro, ceilingMicro });
    if (over) problems.push(`for the ${packets.length} packet(s) together, ${over}`);
  }
  if (problems.length > 0) return { problems, commands: [] };

  return {
    problems: [],
    worstMicro,
    commands: [
      // `--no-clobber` beside the run-id refusal above: that one is a read
      // taken a moment ago, this one is the write itself declining.
      ...packets.map(({ file }, index) => ["gcloud", "storage", "cp", "--no-clobber", file, `gs://${bucket}/packets/${run}/${index}.json`]),
      ["gcloud", "run", "jobs", "execute", "fleet", "--project", project, "--region", region, "--tasks", String(packets.length), "--update-env-vars", `FLEET_RUN=${run}`],
    ],
  };
}

/** One argument as a POSIX shell would need it written. */
const quoted = (argument) => (/^[A-Za-z0-9_@%+=:,./-]+$/.test(argument) ? argument : `'${argument.replaceAll("'", "'\\''")}'`);

/** Every .json file in a directory, in name order: that order is the task index. */
function readPackets(directory) {
  return readdirSync(directory)
    .filter((name) => name.endsWith(".json"))
    .sort()
    .map((name) => {
      const file = join(directory, name);
      try {
        return { file, packet: JSON.parse(readFileSync(file, "utf8")) };
      } catch {
        return { file, packet: undefined };
      }
    });
}

function runCommand(command, { capture = false } = {}) {
  return execFileSync(command[0], command.slice(1), {
    encoding: "utf8",
    stdio: capture ? ["ignore", "pipe", "inherit"] : "inherit",
    ...(capture ? { timeout: 30_000 } : {}),
  });
}

/** The command line, end to end. Returns the process exit code. */
export async function dispatch({
  argv,
  exec = runCommand,
  storeFor = (bucket, token) => bucketStore({ bucket, token: async () => token }),
  now = () => new Date(),
  say = console.log,
  log = console.error,
}) {
  const refuse = (problems) => {
    for (const problem of problems) log(`refused: ${problem}`);
    log("nothing was uploaded and nothing was started.");
    return 1;
  };
  const settings = settingsFrom(argv);
  if (settings.problems.length > 0) return refuse(settings.problems);

  const packets = readPackets(settings.directory);
  const store = storeFor(settings.bucket, exec(["gcloud", "auth", "print-access-token"], { capture: true }).trim());
  const day = now().toISOString().slice(0, 10);
  const dayTotalMicro = await dayTotalMicroUsd(store, day);
  const plan = planDispatch({
    settings,
    packets,
    halted: await store.exists("HALT"),
    runHasPackets: (await store.list(`packets/${settings.run}/`)).length > 0,
    dayTotalMicro,
  });
  if (plan.problems.length > 0) return refuse(plan.problems);

  say(`# run ${settings.run}: ${packets.length} packet(s), summed worst case ${usd(plan.worstMicro)} USD; the ${day} ledger stands at ${usd(dayTotalMicro)} USD against a ${usd(settings.ceilingMicro)} USD ceiling.`);
  packets.forEach(({ file, packet }, index) => say(`# task ${index}: ${packet.packet_id} (${packet.role}, ${packet.model}) from ${file}`));
  for (const command of plan.commands) say(command.map(quoted).join(" "));
  if (!settings.execute) {
    say("# nothing was run. Read the lines above and run them yourself, or repeat this command with --execute.");
    return 0;
  }
  for (const command of plan.commands) exec(command);
  return 0;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  process.exit(
    await dispatch({ argv: process.argv.slice(2) }).catch((cause) => {
      console.error(`failed: ${cause?.message ?? cause}`);
      return 1;
    }),
  );
}
