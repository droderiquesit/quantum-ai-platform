/**
 * The Algorik lockup.
 *
 * The template's `logo.png` slot held the lockup flattened onto an opaque
 * white plate: on the footer's #F7F7F7 ground it rendered as a white
 * rectangle around the logo. This is the approved transparent brand file from
 * `frontend/packages/brand/assets/` — the same artwork the portal ships, so
 * the two surfaces cannot drift apart.
 *
 * The lockup's ink is navy and there is no white-ink variant in the brand
 * package (the file named "white" is the same navy lockup on a white ground).
 * On the dark mobile menu it therefore sits on a light chip rather than being
 * recoloured: inventing a second lockup is how a brand ends up with two.
 */
const RATIO = 512 / 188

export default function Brand({ height = 34, onDark = false, alt = "Algorik" }) {
    const image = (
        <img
            src="/assets/brand/algorik-logo.png"
            alt={alt}
            width={Math.round(height * RATIO)}
            height={height}
            style={{ height, width: "auto", display: "block" }}
        />
    )
    return onDark ? <span className="algorik-brand-chip">{image}</span> : image
}
