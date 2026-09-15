import { CLAIM_STATUSES, NUMERAL_KINDS, isQuantitative } from "@/lib/claims.mjs"

/**
 * The only way a number reaches a reader of this site.
 *
 * §40.6 requires every quantitative statement on the public site to declare
 * whether it is architecture, a target, a demo figure or a measurement. Before
 * this component the requirement was a sentence in a status document, upheld
 * by whoever last edited the copy remembering it — which is another way of
 * saying it was upheld until the first copy edit that did not.
 *
 * Two properties make it structural rather than conventional:
 *
 *  1. **It refuses.** A `Claim` with no status, or with a status outside the
 *     four, throws during render. On a server component that is a build or
 *     request failure, not a silent fallback — the number never reaches a
 *     page. `refuse rather than guess` is the platform's rule and defaulting
 *     an unknown status to the weakest one would be exactly the guess it
 *     forbids: the author would never learn their annotation was wrong.
 *  2. **It is legible to a test.** The status lands in the DOM as
 *     `data-claim-status`, and `tests/claims.spec.mjs` walks every rendered
 *     page and fails on a numeral with no annotated ancestor. A convention
 *     nothing reads back is a convention.
 *
 * The status is also legible to a *reader*: it is the element's `title`, and
 * anything weaker than architecture carries a visible marker, because a target
 * that looks like a measurement is the failure this whole mechanism exists
 * to prevent and hiding the distinction behind a hover would reproduce it for
 * everyone who does not hover.
 */
export function Claim({ status, children, svg = false, as: Tag = svg ? "g" : "span", className = "" }) {
    const explanation = CLAIM_STATUSES[status]
    if (!explanation) {
        throw new Error(
            `Claim: unknown status ${JSON.stringify(status)}. ` +
            `A quantitative statement on the public site must declare one of: ` +
            `${Object.keys(CLAIM_STATUSES).join(", ")}. ` +
            `Nothing on this platform is deployed, so "measured" is almost certainly wrong — ` +
            `if the figure describes how the code is built, it is "architecture".`,
        )
    }
    // `title` is a tooltip in HTML and inert in SVG, so it is omitted there
    // rather than shipped as an attribute that looks like it does something.
    return (
        <Tag className={`algorik-claim ${className}`.trim()} data-claim-status={status} title={svg ? undefined : explanation}>
            {children}
            {!svg && status !== "architecture" && (
                <sup className="algorik-claim-flag" aria-label={`${status} — not a measurement`}>{status}</sup>
            )}
        </Tag>
    )
}

/**
 * A numeral that counts nothing about the platform.
 *
 * Section numbers, step positions and the copyright year are numerals a reader
 * sees and no reader could mistake for a claim — but the sweep cannot tell
 * them apart from a claim by looking, and any rule that tried to would be a
 * rule with an exemption for a shape, which is the shape an over-claim would
 * then adopt. So they are annotated too, and the annotation says out loud that
 * nothing is being counted.
 *
 * It refuses an unknown kind for the same reason `Claim` does.
 */
export function Numeral({ kind, children, svg = false, as: Tag = svg ? "g" : "span", className = "" }) {
    const explanation = NUMERAL_KINDS[kind]
    if (!explanation) {
        throw new Error(
            `Numeral: unknown kind ${JSON.stringify(kind)}. ` +
            `A numeral that is not a quantitative claim must declare one of: ` +
            `${Object.keys(NUMERAL_KINDS).join(", ")}. ` +
            `If the number does say something about the platform, it is a Claim, not a Numeral.`,
        )
    }
    return (
        <Tag className={`algorik-numeral ${className}`.trim()} data-numeral={kind} title={svg ? undefined : explanation}>
            {children}
        </Tag>
    )
}

/**
 * The public explanation of the labelling scheme.
 *
 * A label a reader cannot decode is decoration. This renders the four statuses
 * with the same text the annotations carry, from the same object, so the
 * legend cannot come to describe a scheme the site no longer uses.
 */
export function ClaimLegend() {
    return (
        <dl className="algorik-claim-legend">
            {Object.entries(CLAIM_STATUSES).map(([status, explanation]) => (
                <div key={status}>
                    <dt>
                        <span className="algorik-claim-legend-chip" data-claim-legend={status}>{status}</span>
                    </dt>
                    <dd>{explanation}</dd>
                </div>
            ))}
        </dl>
    )
}

/**
 * Re-exported so a caller that already imports the components can ask the
 * question the components ask, without reaching past them into the detector.
 */
export { isQuantitative }
