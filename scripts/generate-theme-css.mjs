/**
 * Emits the portal's theme block from @algorik/design-tokens.
 *
 * The tokens are decided in one file; this writes the CSS that file implies.
 * Hand-maintaining both is how a landing page and a portal end up two shades
 * apart — the drift is invisible in review and obvious to a user who moves
 * between them.
 *
 * Run: node scripts/generate-theme-css.mjs
 * Checked in CI by `--check`, which fails if the committed CSS has drifted.
 */
import { readFileSync, writeFileSync } from "node:fs";

const TARGET = "frontend/src/app/globals.css";
const BEGIN = "/* ALGORIK-TOKENS:BEGIN — generated from @algorik/design-tokens. Do not edit by hand. */";
const END = "/* ALGORIK-TOKENS:END */";

// The tokens file is TypeScript; rather than add a compiler to read it, the
// two theme objects are parsed out of it directly. A malformed parse throws
// loudly here rather than emitting half a theme.
const source = readFileSync("packages/design-tokens/src/index.ts", "utf8");

function scale(name) {
  const match = new RegExp(`export const ${name}: ColorScale = \\{([\\s\\S]*?)\\n\\};`).exec(source);
  if (!match) throw new Error(`could not find the '${name}' colour scale in the tokens package`);
  const entries = [...match[1].matchAll(/^\s*(\w+):\s*"([^"]+)"/gm)];
  if (entries.length === 0) throw new Error(`the '${name}' scale parsed to zero tokens`);
  return entries.map(([, key, value]) => [key, value]);
}

const kebab = (name) => name.replace(/[A-Z]/g, (letter) => `-${letter.toLowerCase()}`);
const block = (entries, indent) =>
  entries.map(([key, value]) => `${indent}--color-${kebab(key)}: ${value};`).join("\n");

const dark = scale("dark");
const lightScale = scale("light");

/*
 * Compatibility aliases.
 *
 * The semantic names above are canonical. These are the names the portal's
 * thirty-five pages were written against, kept as aliases so the rename is not
 * a thirty-five-file rewrite landing in one commit — which is how a rename
 * takes a working surface down. Each alias points at exactly one semantic
 * token, so there is still only one place a colour is decided; a page migrates
 * by changing its `var()` and nothing else.
 *
 * Delete an entry when nothing references it. `npm run tokens:unused` lists
 * the ones that are ready to go.
 */
const ALIASES = [
  ["void", "canvas"],
  ["sunken", "surface-sunken"],
  ["raised", "surface-elevated"],
  ["line", "border"],
  ["line-strong", "border-strong"],
  ["ink", "text-primary"],
  ["ink-dim", "text-muted"],
  ["ink-faint", "text-faint"],
  ["accent", "brand-primary"],
  ["accent-dim", "brand-primary-muted"],
  ["quantum", "brand-secondary"],
  ["up", "gain"],
  ["down", "loss"],
  ["warn", "warning"],
  ["halt", "critical"],
  ["paper", "paper"],
  ["paper-dim", "brand-primary-muted"],
  ["env-simulation", "simulation"],
  ["env-paper", "paper"],
  ["env-staging", "stage"],
  ["env-live", "live"],
];

const aliasBlock = (indent) =>
  ALIASES.map(([from, to]) => `${indent}--color-${from}: var(--color-${to});`).join("\n");

const generated = `${BEGIN}
:root {
  color-scheme: dark;
${block(dark, "  ")}

  /* aliases — see ALIASES in scripts/generate-theme-css.mjs */
${aliasBlock("  ")}
}

:root[data-theme="light"] {
  color-scheme: light;
${block(lightScale, "  ")}
}
${END}`;

const css = readFileSync(TARGET, "utf8");
const start = css.indexOf(BEGIN);
const finish = css.indexOf(END);
if (start === -1 || finish === -1) {
  throw new Error(`${TARGET} has no ALGORIK-TOKENS block to fill`);
}
const next = css.slice(0, start) + generated + css.slice(finish + END.length);

if (process.argv.includes("--check")) {
  if (next !== css) {
    console.error(
      `${TARGET} has drifted from packages/design-tokens.\n` +
        `Run: node scripts/generate-theme-css.mjs`,
    );
    process.exit(1);
  }
  console.log(`${TARGET} matches packages/design-tokens (${dark.length} tokens per theme).`);
} else {
  writeFileSync(TARGET, next);
  console.log(`${TARGET} regenerated: ${dark.length} tokens per theme.`);
}
