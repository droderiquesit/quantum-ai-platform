#!/usr/bin/env node
/**
 * The landing's lint.
 *
 * `npm run lint` was `next lint`, which Next 16 removed — the script did not
 * lint, it errored with "Invalid project directory provided, no such
 * directory: .../lint", and CI never ran it, so nothing noticed. This replaces
 * it with checks that hold the specific failures this site has already
 * shipped, and it adds no dependency to do it: the landing's tree is governed
 * like Cargo.toml (frontend/CLAUDE.md, ADR 0002/0009).
 *
 * ESLint proper is a separate, reviewable decision — `eslint` and
 * `eslint-config-next` are devDependencies of the portal and could be adopted
 * here too. That addition is not made unilaterally.
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";

const ROOT = resolve(import.meta.dirname, "..");
const SOURCE_DIRS = ["app", "components", "lib"];
const PUBLIC_DIR = join(ROOT, "public");

const failures = [];
const fail = (file, line, message) =>
    failures.push(`${relative(ROOT, file)}${line ? `:${line}` : ""}  ${message}`);

function walk(dir, predicate = () => true) {
    const out = [];
    for (const entry of readdirSync(dir)) {
        const full = join(dir, entry);
        if (statSync(full).isDirectory()) out.push(...walk(full, predicate));
        else if (predicate(full)) out.push(full);
    }
    return out;
}

const sources = SOURCE_DIRS.flatMap((dir) =>
    walk(join(ROOT, dir), (f) => f.endsWith(".js") || f.endsWith(".jsx") || f.endsWith(".mjs")),
);

/** Every route the app serves, derived from the app directory itself. */
const routes = new Set(
    walk(join(ROOT, "app"), (f) => /(^|\/)page\.js$/.test(f)).map((f) => {
        const rel = relative(join(ROOT, "app"), f).replace(/\/?page\.js$/, "");
        return "/" + rel;
    }).map((r) => (r === "/" ? "/" : r.replace(/\/$/, ""))),
);
routes.add("/");
/** Redirects declared in next.config.js are destinations that resolve too. */
const REDIRECTED = new Set(["/about", "/error"]);

const RULES = [
    {
        // Next resolves a relative src against the current route, so
        // `src="assets/x.png"` is correct only while every route is one
        // segment deep. The first nested route silently 404s every image.
        name: "asset references are absolute",
        test: (text) => [...text.matchAll(/(?:src=|url\(|href=)["']?assets\//g)],
        message: 'relative asset reference — must start with "/assets/"',
    },
    {
        name: "no template .html destinations",
        test: (text) => [...text.matchAll(/(?:href|action)=["'][^"']*\.html["']/g)],
        message: "link to a static template page that does not exist here",
    },
    {
        name: "no template demo routes",
        test: (text) => [...text.matchAll(/["']\/?index-[0-9]/g)],
        message: "link to a template demo route (index-N)",
    },
    {
        name: "no vendor branding",
        test: (text) => [...text.matchAll(/[Ff]or[Tt]radex/g)],
        message: "the template vendor's brand name is not Algorik's to ship",
    },
    {
        name: "no placeholder copy",
        test: (text) => [...text.matchAll(/lorem ipsum/gi)],
        message: "placeholder copy",
    },
    {
        // React silently ignores `class` and logs an error. The template's
        // preloader did this on every route transition.
        name: "JSX uses className",
        test: (text) => [...text.matchAll(/<[a-zA-Z][^>]*\sclass=["']/g)],
        message: "`class=` in JSX — React wants `className=`",
    },
    {
        // A form on this site would either submit somewhere real (there is
        // nowhere) or drop the message. Neither belongs on a public site for a
        // platform whose whole claim is that nothing is quietly discarded.
        name: "no forms",
        test: (text) => [...text.matchAll(/<form[\s>]/g)],
        message: "a <form> with no verified delivery path",
    },
    {
        // The browser receives nothing the public may not see.
        name: "only the public portal URL is read from the environment",
        test: (text) => [...text.matchAll(/process\.env\.(?!NEXT_PUBLIC_ALGORIK_PORTAL_URL)([A-Za-z_]+)/g)],
        message: "reads an environment variable other than the public portal URL",
    },
];

for (const file of sources) {
    const text = readFileSync(file, "utf8");
    const lineOf = (index) => text.slice(0, index).split("\n").length;
    for (const rule of RULES) {
        for (const match of rule.test(text)) {
            fail(file, lineOf(match.index), `${rule.message} — ${JSON.stringify(match[0])}`);
        }
    }

    // Every asset the source names must exist under public/.
    for (const match of text.matchAll(/(?:src=|url\()["']?(\/assets\/[^"')\s]+)/g)) {
        try {
            statSync(join(PUBLIC_DIR, match[1]));
        } catch {
            fail(file, lineOf(match.index), `asset does not exist in public/: ${match[1]}`);
        }
    }

    // Every internal destination must be a route this app serves. A nav that
    // links to a page nobody built is the defect this whole file exists for.
    for (const match of text.matchAll(/href=["'](\/[^"'#?]*)(?:[#?][^"']*)?["']/g)) {
        const target = match[1].length > 1 ? match[1].replace(/\/$/, "") : "/";
        if (target.startsWith("/assets/")) continue;
        if (routes.has(target) || REDIRECTED.has(target)) continue;
        fail(file, lineOf(match.index), `links to ${target}, which is not a route`);
    }
}

// The posture label is not optional: it is required wherever posture is shown,
// and the header shows posture on every page.
const site = readFileSync(join(ROOT, "lib/site.js"), "utf8");
if (!/POSTURE\s*=\s*"PAPER TRADING"/.test(site)) {
    fail(join(ROOT, "lib/site.js"), null, "the PAPER TRADING posture label is missing or reworded");
}

if (failures.length) {
    console.error(`landing lint: ${failures.length} problem(s)\n`);
    for (const line of failures) console.error("  " + line);
    process.exit(1);
}
console.log(`landing lint: clean — ${sources.length} files, ${routes.size} routes checked`);
