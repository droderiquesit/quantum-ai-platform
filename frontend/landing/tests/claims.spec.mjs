/**
 * §40.6: every quantitative statement on the public site declares its status.
 *
 * The requirement is not stylistic. This is a paper-trading research platform
 * with **nothing deployed** — `execution_nodes = {}` in every environment, no
 * process has been shown to be scraped — so a figure on a public page that
 * reads as an observation is a false statement about the system made to people
 * who cannot check it. It is the outward-facing form of the failure
 * `.claude/rules/00-enterprise-governance.md` exists to prevent, and unlike an
 * agent's report to a reviewer, nobody downstream ever asks it for evidence.
 *
 * Until this file existed the rule was, in the delivery register's own words,
 * "upheld by authorial restraint rather than by anything a test can check".
 * Authorial restraint survives exactly as long as the author who had it.
 *
 * The sweep below reads the rendered DOM rather than the source, because the
 * copy arrives from data arrays, from shared blocks and from a client
 * component that animates a counter — a source-level rule would have to
 * understand all of that, and the one thing it could never see is what a
 * visitor actually reads.
 */
import { expect, test } from "@playwright/test"
import {
    ANNOTATED_SELECTOR,
    CLAIM_ATTRIBUTE,
    CLAIM_STATUS_NAMES,
    NUMERAL_ATTRIBUTE,
    NUMERAL_KIND_NAMES,
    QUANTITY_PATTERN,
    quantities,
} from "../lib/claims.mjs"

/** Every page this site serves. Kept in step with `landing.spec.mjs`; a page
 *  missing from this list is a page the sweep never reads. */
const PAGES = [
    "/",
    "/platform",
    "/technology",
    "/security",
    "/institutional",
    "/developers",
    "/company",
    "/contact",
    "/legal",
    "/legal/terms",
    "/legal/privacy",
    "/legal/risk-disclosures",
    // The 404 body is a page a visitor reaches and a crawler indexes, and it
    // renders a numeral of its own. A sweep that only read the routes somebody
    // meant to publish would miss the one page nobody re-reads.
    "/no-such-page-anywhere",
]

/**
 * An annotation wrapping half a page would silence the sweep without anybody
 * editing this file, so an annotated element is held to a size. A quantitative
 * statement is a phrase or a sentence; this bound is generous enough for the
 * longest sentence on the site and far too small to blanket a section.
 */
const LONGEST_REASONABLE_CLAIM = 500

/**
 * Walk the rendered text of a page and return every quantity that reached a
 * reader without an annotated ancestor, plus the annotations that were found.
 *
 * The regular expression is rebuilt inside the browser from the same source
 * string the application and the lint compile — a RegExp object cannot cross
 * `page.evaluate`, but the string can, and two hand-copied patterns would
 * drift with the quieter of the two inside the test.
 */
async function sweep(page, path) {
    await page.goto(path, { waitUntil: "networkidle" })
    return page.evaluate(
        ({ pattern, selector, claimAttribute, numeralAttribute }) => {
            const quantity = new RegExp(pattern, "gi")
            const NOT_PROSE = new Set(["SCRIPT", "STYLE", "NOSCRIPT", "TEMPLATE"])

            const describe = (element) => {
                const parts = []
                for (let node = element; node && node.tagName; node = node.parentElement) {
                    const name = String(node.tagName).toLowerCase()
                    const cls = typeof node.className === "string" && node.className
                        ? "." + node.className.trim().split(/\s+/).join(".")
                        : ""
                    parts.unshift(name + cls)
                    if (name === "body") break
                }
                return parts.slice(-4).join(" > ")
            }

            const unannotated = []
            const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT)
            for (let node = walker.nextNode(); node; node = walker.nextNode()) {
                const parent = node.parentElement
                if (!parent) continue
                let excluded = false
                for (let a = parent; a; a = a.parentElement) {
                    if (NOT_PROSE.has(String(a.tagName).toUpperCase())) { excluded = true; break }
                }
                if (excluded) continue
                const text = node.nodeValue || ""
                quantity.lastIndex = 0
                const tokens = [...text.matchAll(quantity)].map((m) => m[0])
                if (tokens.length === 0) continue
                if (parent.closest(selector)) continue
                unannotated.push({ tokens, text: text.trim().slice(0, 200), where: describe(parent) })
            }

            const annotated = [...document.querySelectorAll(selector)].map((element) => ({
                status: element.getAttribute(claimAttribute),
                kind: element.getAttribute(numeralAttribute),
                text: (element.textContent || "").trim(),
                where: describe(element),
            }))

            return { unannotated, annotated }
        },
        {
            pattern: QUANTITY_PATTERN,
            selector: ANNOTATED_SELECTOR,
            claimAttribute: CLAIM_ATTRIBUTE,
            numeralAttribute: NUMERAL_ATTRIBUTE,
        },
    )
}

test("the quantity detector fires on the sentences this site is made of", () => {
    // Assert the premise before the property. Every sweep below is a filter
    // over what this pattern finds, and a pattern that found nothing would
    // report twelve clean pages while reading none of them. The examples are
    // real copy from the site, in both spellings a number arrives in.
    for (const sentence of ["Eight stages, every cycle", "59 Rust crates", "two third-party libraries", "ninety percent"]) {
        expect(quantities(sentence).length, `the detector reads "${sentence}" as carrying no quantity`)
            .toBeGreaterThan(0)
    }
    // And that it is not simply true of everything: a rule that fires on all
    // prose would be satisfied by annotating the whole site and would then
    // distinguish nothing.
    for (const sentence of ["Algorik never submits a live order.", "Refusals are answers"]) {
        expect(quantities(sentence).length, `the detector reads "${sentence}" as a quantity`).toBe(0)
    }
})

test.describe("every quantitative statement declares its status", () => {
    for (const path of PAGES) {
        test(`${path} renders no numeral a reader could mistake for a measurement`, async ({ page }) => {
            const { unannotated, annotated } = await sweep(page, path)
            // Compared as one rendered report rather than as an array: a deep
            // equality failure on two hundred objects buries the sentences an
            // author has to fix under the diff of the objects describing them.
            const report = unannotated
                .map((hit) => `  [${hit.tokens.join(" ")}] ${hit.where}\n      "${hit.text}"`)
                .join("\n")
            expect(
                report,
                `${path} states a quantity with no architecture/target/demo/measured status`,
            ).toBe("")
            // The premise: this page was actually read. A page that rendered
            // nothing would pass the assertion above and prove nothing, and
            // every page on this site does state at least one quantity.
            expect(annotated.length, `${path} carries no annotated numeral at all — the sweep read nothing`)
                .toBeGreaterThan(0)
        })
    }
})

test("an annotation covers a statement, not a section", async ({ page }) => {
    // The cheapest way to defeat the sweep is not to delete it: it is to put
    // `data-claim-status` on a wrapper and let everything inside inherit an
    // answer nobody gave. Two properties stop that. An annotation must contain
    // a quantity — a decorative one is a hole waiting for a number to be typed
    // into it — and it must be the size of a statement.
    let checked = 0
    for (const path of PAGES) {
        const { annotated } = await sweep(page, path)
        for (const element of annotated) {
            expect(quantities(element.text).length,
                `${path}: an annotation at ${element.where} covers no quantity: "${element.text.slice(0, 120)}"`)
                .toBeGreaterThan(0)
            expect(element.text.length,
                `${path}: an annotation at ${element.where} covers ${element.text.length} characters — that is a blanket, not a statement`)
                .toBeLessThanOrEqual(LONGEST_REASONABLE_CLAIM)
            const value = element.status ?? element.kind
            expect([...CLAIM_STATUS_NAMES, ...NUMERAL_KIND_NAMES],
                `${path}: an annotation at ${element.where} declares "${value}", which is not a status this site defines`)
                .toContain(value)
            checked += 1
        }
    }
    expect(checked, "no annotation was examined on any page — the premise of this test is wrong").toBeGreaterThan(0)
})

test("nothing on this site claims to have been measured, because nothing has been", async ({ page }) => {
    // `measured` is the one status this platform cannot currently earn.
    // Nothing is deployed — `execution_nodes = {}` in every environment — and
    // no process has been shown to be scraped, so there is no production
    // observation for a public figure to be. This test is where that fact is
    // held: publishing a measured figure means deleting this test, which means
    // a reviewer is shown the evidence for it. That is the whole point of
    // making it fail loudly rather than leaving `measured` as a status anyone
    // may reach for.
    for (const path of PAGES) {
        const { annotated } = await sweep(page, path)
        const measured = annotated.filter((element) => element.status === "measured")
        expect(
            measured,
            `${path} publishes a figure labelled "measured": ${measured.map((m) => m.text).join(" | ")}. ` +
            `Nothing on this platform is deployed. If a deployment has since produced this figure, ` +
            `replace this test with one that names the observation.`,
        ).toEqual([])
    }
})

test("the labelling scheme is explained where a reader will look for it", async ({ page }) => {
    // A label a reader cannot decode is decoration. The risk disclosures are
    // the document that governs how everything else on the site is read, so
    // the legend lives there rather than in a footnote nobody reaches.
    await page.goto("/legal/risk-disclosures")
    const legend = page.locator(".algorik-claim-legend")
    await expect(legend, "the risk disclosures do not explain how a number on this site is labelled").toBeAttached()
    for (const status of CLAIM_STATUS_NAMES) {
        await expect(
            legend.locator(`[data-claim-legend="${status}"]`),
            `the legend does not define "${status}"`,
        ).toBeAttached()
    }
})
