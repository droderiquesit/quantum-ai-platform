/**
 * What a number on this site is allowed to mean.
 *
 * This platform is paper trading and **nothing is deployed** — no execution
 * node exists in any environment, and no process has been shown to be scraped.
 * A figure on a public page that reads as an observation when it is an
 * architectural property, or worse a design target, is therefore the most
 * expensive sentence this repository can emit: it faces outward, it reaches
 * people who cannot open the repository to check it, and no compiler catches
 * prose. §40.6 requires every quantitative statement here to carry its status.
 * That requirement was upheld by authorial restraint until this file existed,
 * which is to say it was upheld by nobody after the first copy edit.
 *
 * The status is therefore structural: it is an attribute on the element that
 * renders the number, it is written by a component that refuses to render
 * without it, and `tests/claims.spec.mjs` walks the rendered DOM of every page
 * and fails on a numeral that reached a reader without one.
 *
 * This module is the single definition of *what counts as a number*, shared by
 * the components that annotate, the lint that guards the annotation, and the
 * test that enforces it. Three copies of this regular expression would
 * disagree within a month, and the one that disagreed quietly would be the one
 * in the test.
 *
 * It is `.mjs` rather than `.js` on purpose: the landing's `package.json`
 * declares no `"type": "module"`, so a `.js` file here cannot be imported by
 * the Playwright spec or by `scripts/lint.mjs`, and a detector the test cannot
 * import is a detector the test would reimplement.
 */

/**
 * The four statuses, and what each one promises a reader.
 *
 * The text is not decoration — it is rendered as the element's `title`, so a
 * reader who wonders what a number means gets the answer from the page rather
 * than from this file.
 */
export const CLAIM_STATUSES = Object.freeze({
    architecture:
        "Architecture — a property of how the platform, including this site, is built. Checkable by reading the source; not an observation of anything running.",
    target:
        "Target — a stated design goal. Nothing has produced this figure yet.",
    demo:
        "Demo — a figure from a demonstration or a simulated run, never from a production deployment.",
    measured:
        "Measured — an observed figure from a running deployment, with the observation on record.",
})

/** The statuses, as a list, for validation and for iterating in tests. */
export const CLAIM_STATUS_NAMES = Object.freeze(Object.keys(CLAIM_STATUSES))

/**
 * The kinds of numeral that assert no quantity about the platform.
 *
 * A document's section number, a step's position in a list and a copyright
 * year are all numerals a reader sees, and none of them is a claim. They still
 * have to be annotated: a rule that exempts a category by pattern-matching is
 * a rule whose exemption is the first thing a mis-annotated claim hides
 * behind. Saying "this numeral counts nothing" out loud costs one component
 * and removes the whole category of argument.
 */
export const NUMERAL_KINDS = Object.freeze({
    ordinal: "A position in a sequence — a section number, a step, a layer. It counts nothing.",
    date: "A date. It counts nothing about the platform.",
    version: "The version of a published specification. It counts nothing about the platform.",
    code: "An identifier — an HTTP status, an error reference. It counts nothing.",
})

/** The numeral kinds, as a list. */
export const NUMERAL_KIND_NAMES = Object.freeze(Object.keys(NUMERAL_KINDS))

/**
 * The words that carry a quantity in English prose, spelled out.
 *
 * Digits alone would be a rule anyone could step around without noticing, and
 * on this site most of the quantities already spell out: "eight stages",
 * "seventeen specialist agents", "two third-party libraries". A detector that
 * only read `\d` would have found almost nothing here and reported the site
 * clean, which is the worst outcome available — a green gate over an
 * unchecked page.
 */
const NUMBER_WORDS = [
    "zero",
    "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
    "eleven", "twelve", "thirteen", "fourteen", "fifteen", "sixteen",
    "seventeen", "eighteen", "nineteen",
    "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
    "hundred", "hundreds", "thousand", "thousands",
    "million", "millions", "billion", "billions", "trillion", "trillions",
    "dozen", "dozens", "twice", "thrice", "percent",
]

/**
 * **"One" is deliberately absent, and this is the rule's one honest hole.**
 *
 * In English "one" is an indefinite pronoun and a determiner far more often
 * than it is a count — "the one deliberate exception", "each one", "one that
 * catches what the others miss". Requiring an annotation on every occurrence
 * would bury the real claims under dozens of grammatical ones, and a rule
 * whose output is mostly noise is a rule that gets suppressed rather than
 * obeyed.
 *
 * The residual exposure is bounded, and worth naming here rather than hiding:
 * a quantitative claim of exactly one — "one workspace", "one address" — can
 * reach a reader unannotated. It is the weakest quantity a sentence can
 * assert, it cannot be inflated by a copy edit without changing the word to
 * something this list does catch, and an author may still annotate it by hand.
 * The same applies to "no" and "none" as negations, and to "both".
 *
 * Ordinal words — "first", "second", "third" — are absent for a different
 * reason: they order, they do not count.
 */
export const DELIBERATELY_NOT_A_QUANTITY = Object.freeze([
    "one", "no", "none", "both", "first", "second", "third",
])

/**
 * Anything that reads as a quantity: a digit run, a spelled-out number, or a
 * quantity symbol.
 *
 * The `\d` arm deliberately matches a digit anywhere, including inside a word,
 * so "3.1", "90-day" and "US30" all trip it. A number stuck to a letter is
 * exactly the shape a figure smuggled into prose takes.
 */
export const QUANTITY_PATTERN = `(?:\\d|[%×]|\\b(?:${NUMBER_WORDS.join("|")})\\b)`

/** Compiled once here. The DOM sweep compiles its own copy from this same
 *  source string inside the browser, because a RegExp cannot be passed
 *  through `page.evaluate` — the string crosses, the object does not. */
const QUANTITY = new RegExp(QUANTITY_PATTERN, "gi")

/**
 * Every quantity token in a string, with the index it was found at.
 *
 * Returns the tokens rather than a boolean so a caller can quote the offending
 * one back. A lint failure that says "this string contains a number" and makes
 * the author hunt for it is a lint failure the author disables.
 */
export function quantities(text) {
    QUANTITY.lastIndex = 0
    return [...String(text).matchAll(QUANTITY)].map((match) => ({
        token: match[0],
        index: match.index,
    }))
}

/** True when a string says something quantitative. */
export function isQuantitative(text) {
    return quantities(text).length > 0
}

/**
 * The attribute names the annotation is carried on.
 *
 * Exported so the lint, the components and the spec cannot drift on the
 * spelling — a test looking for `data-claim` while the component writes
 * `data-claim-status` passes forever and guards nothing.
 */
export const CLAIM_ATTRIBUTE = "data-claim-status"
export const NUMERAL_ATTRIBUTE = "data-numeral"

/** The selector matching an annotated ancestor, for the DOM sweep. */
export const ANNOTATED_SELECTOR = `[${CLAIM_ATTRIBUTE}],[${NUMERAL_ATTRIBUTE}]`

/**
 * The quantitative statements that reach a reader through an attribute.
 *
 * A search result's snippet and the alternative text a blind reader hears
 * instead of a diagram are public statements of the same kind as body copy,
 * and several of them carry numbers. They cannot carry a `data-` attribute:
 * an `aria-label` is a flat string, and `<meta name="description">` has no
 * element of its own to annotate. Dropping the numbers from them to satisfy
 * the sweep would be a copy downgrade caused by a tool's limits, and would
 * leave the screen-reader user with less than the sighted one.
 *
 * So they are declared here instead, and `scripts/lint.mjs` holds the
 * declaration to the source **in both directions**: a quantity-bearing
 * `aria-label`, `label` or `description` string that is not on this list
 * fails the lint, and a list entry that no longer appears in the source fails
 * it too. One direction alone rots — the first lets a new claim ship
 * undeclared, and the second alone lets the list fill with statements the site
 * stopped making.
 *
 * Yes, this is the copy written twice, which is a thing this platform
 * otherwise refuses. The difference is that these two copies are made to
 * disagree *loudly*: the lint fails on the first character of drift, which is
 * the only version of duplication that is safe.
 */
export const DECLARED_ATTRIBUTE_CLAIMS = Object.freeze([
    // Diagram alternative text — what a reader who cannot see the drawing gets.
    ["architecture", "The eight-stage decision loop: sense, understand, discover, reason, simulate, decide, act, learn"],
    ["architecture", "Three independent layers refusing a live order: infrastructure, start-up checks, and the type system"],
    ["architecture", "A seventeen-seat reasoning panel with an adversarial reviewer and no execution seat"],
    ["architecture", "Three layers refusing a live order"],
    ["architecture", "An eight-stage loop, every step recorded"],
    // The regional topology is the one quantity on this site that is a plan
    // rather than a property: ADR 0035 calls seven "the blueprint's target and
    // premature", and `execution_nodes = {}` in every environment, so no cell
    // is deployed. The label says so rather than leaving a reader to assume.
    ["target", "Seven regional cells is the target topology, each trading inside its own capital envelope; none is deployed"],
    // Search-result snippets.
    ["architecture", "What the Algorik platform serves today: an eight-stage loop, opportunities, strategies, capital envelopes, risk and kill switch, simulated execution — with honest notes on what is still in research."],
    ["architecture", "How Algorik reasons: a seventeen-agent panel with computed confidence, and quantum experiments kept only where they measurably improve on a classical baseline."],
    ["architecture", "The paper-trading boundary held at three structural layers, a hash-chained audit log, keyless workload identity, and secrets that never touch an environment variable."],
])
