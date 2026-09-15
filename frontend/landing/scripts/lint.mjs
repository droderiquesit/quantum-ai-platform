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
import {
    CLAIM_ATTRIBUTE,
    CLAIM_STATUS_NAMES,
    DECLARED_ATTRIBUTE_CLAIMS,
    NUMERAL_ATTRIBUTE,
    NUMERAL_KIND_NAMES,
    isQuantitative,
} from "../lib/claims.mjs";

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
    //
    // Both spellings are checked: the JSX attribute `href="/x"` and the data
    // property `href: "/x"`. The navigation is generated from a data structure,
    // so a rule that only read attributes passed a mutation that added
    // `{ label: "Careers", href: "/careers" }` to the menu — the Playwright
    // link test caught it, this file did not, and that gap is what this
    // comment records.
    for (const match of text.matchAll(/href(?:=|:\s*)["'](\/[^"'#?]*)(?:[#?][^"']*)?["']/g)) {
        const target = match[1].length > 1 ? match[1].replace(/\/$/, "") : "/";
        if (target.startsWith("/assets/")) continue;
        if (routes.has(target) || REDIRECTED.has(target)) continue;
        fail(file, lineOf(match.index), `links to ${target}, which is not a route`);
    }
}

// --- §40.6: quantitative statements carry a status -------------------------
//
// The DOM sweep in `tests/claims.spec.mjs` is the enforcement for anything a
// reader sees rendered. Two things it structurally cannot see are checked here
// instead, and they are the two places an over-claim could otherwise hide.

{
    // 1. The annotation must come from the component that validates it.
    //
    // `Claim` throws on a status outside the four, and `Numeral` on a kind
    // outside its own — that refusal is the whole reason the attribute means
    // anything. Writing `data-claim-status="measured"` by hand on a div would
    // put a status in the DOM that nothing checked and satisfy the sweep,
    // which reads the attribute and does not care who wrote it.
    // The component that writes the attribute and the module that names it are
    // the two files allowed to spell it; everywhere else it must arrive
    // through the component.
    const allowed = new Set([join(ROOT, "components/elements/Claim.js"), join(ROOT, "lib/claims.mjs")]);
    for (const file of sources) {
        if (allowed.has(file)) continue;
        const text = readFileSync(file, "utf8");
        const lineOf = (index) => text.slice(0, index).split("\n").length;
        for (const match of text.matchAll(new RegExp(`${CLAIM_ATTRIBUTE}|${NUMERAL_ATTRIBUTE}`, "g"))) {
            fail(file, lineOf(match.index),
                `writes ${match[0]} directly — a status must come from <Claim> or <Numeral>, which refuse an unknown one`);
        }
    }
}

{
    // 2. A quantity in an attribute must be declared, and every declaration
    //    must still be in the source.
    //
    // A search-result snippet and the alternative text a blind reader hears
    // are public statements, and several of them carry numbers. Neither has an
    // element the sweep could read an attribute off, so the status is declared
    // in `lib/claims.mjs` and held to the source here — in both directions,
    // because one direction alone rots. Without the first, a new undeclared
    // claim ships; without the second, the list fills with statements the site
    // no longer makes and a reviewer reads a register of fiction.
    const declared = new Map(DECLARED_ATTRIBUTE_CLAIMS.map(([status, text]) => [text, status]));
    const seen = new Set();
    // Two shapes, kept apart on purpose. A JSX *attribute* (`label="…"`,
    // `aria-label="…"`, `alt="…"`) never becomes a text node, so the sweep can
    // never see it. An object *property* (`label: "…"`) usually does become
    // one — `lib/site.js`'s navigation labels are rendered copy and are
    // annotated where they render — so matching `label:` here would demand a
    // second, unrenderable declaration for a string the sweep already holds.
    // `description:` is the exception and is matched: Next's metadata never
    // reaches the document body.
    const ATTRIBUTE_TEXT = [
        /(?:aria-label|label|alt)\s*=\s*"((?:[^"\\]|\\.)*)"/g,
        /\bdescription\s*:\s*"((?:[^"\\]|\\.)*)"/g,
    ];
    for (const file of sources) {
        const text = readFileSync(file, "utf8");
        const lineOf = (index) => text.slice(0, index).split("\n").length;
        for (const match of ATTRIBUTE_TEXT.flatMap((pattern) => [...text.matchAll(pattern)])) {
            const value = match[1];
            if (!isQuantitative(value)) continue;
            const status = declared.get(value);
            if (!status) {
                fail(file, lineOf(match.index),
                    `states a quantity in an attribute that DECLARED_ATTRIBUTE_CLAIMS does not declare: ${JSON.stringify(value)}`);
                continue;
            }
            if (!CLAIM_STATUS_NAMES.includes(status)) {
                fail(file, lineOf(match.index), `is declared with an unknown status: ${JSON.stringify(status)}`);
            }
            seen.add(value);
        }
    }
    for (const [text] of declared) {
        if (!seen.has(text)) {
            fail(join(ROOT, "lib/claims.mjs"), null,
                `DECLARED_ATTRIBUTE_CLAIMS declares a statement no source file makes: ${JSON.stringify(text)}`);
        }
    }
}

{
    // 3. Nothing may be declared "measured".
    //
    // Nothing on this platform is deployed — `execution_nodes = {}` in every
    // environment, and no process has been shown to be scraped — so there is
    // no production observation for a public figure to be. The sweep holds the
    // same line for rendered copy; this holds it for the declarations, which
    // the sweep cannot reach. When a deployment does produce a figure, both
    // gates have to be changed deliberately, in front of a reviewer.
    for (const [status, text] of DECLARED_ATTRIBUTE_CLAIMS) {
        if (status === "measured") {
            fail(join(ROOT, "lib/claims.mjs"), null,
                `declares a measured figure while nothing is deployed: ${JSON.stringify(text)}`);
        }
    }
    // The statuses and kinds are enumerations; a typo in either file would
    // otherwise be a silently unlabelled claim.
    for (const [status] of DECLARED_ATTRIBUTE_CLAIMS) {
        if (!CLAIM_STATUS_NAMES.includes(status)) {
            fail(join(ROOT, "lib/claims.mjs"), null, `unknown claim status: ${JSON.stringify(status)}`);
        }
    }
    if (CLAIM_STATUS_NAMES.length === 0 || NUMERAL_KIND_NAMES.length === 0) {
        fail(join(ROOT, "lib/claims.mjs"), null, "the status vocabulary is empty — every annotation would be unchecked");
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
