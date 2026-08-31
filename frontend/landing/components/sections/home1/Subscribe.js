import { CtaBand } from "@/components/sections/Blocks"

/**
 * The template's newsletter strip: an input and a submit button wired to
 * nothing, which is a promise to write back that this deployment could not
 * keep. It is a pair of links now — both of which go somewhere.
 */
export default function Subscribe() {
    return (
        <CtaBand
            title="Questions? Talk to the desk."
            lede="Algorik is paper trading, structurally. If reproducibility, attribution and governance are what your evaluation turns on, we should talk."
            secondaryHref="/contact"
            secondaryLabel="Contact us"
        />
    )
}
