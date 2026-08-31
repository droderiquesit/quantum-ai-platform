/**
 * One declaration of the public information architecture.
 *
 * The header, the mobile menu and the footer all read this file. They used to
 * carry three hand-written copies of the same list, which is how a site ends
 * up with a footer link to a page the nav never mentions and a nav entry that
 * 404s — the exact failure the Playwright link test now refuses.
 */

export const PORTAL = process.env.NEXT_PUBLIC_ALGORIK_PORTAL_URL ?? "http://127.0.0.1:3400"

/** Where the portal's doors are. Sign-in lives there; the landing never authenticates. */
export const SIGN_IN = `${PORTAL}/sign-in`
export const SIGN_UP = `${PORTAL}/sign-up`

/**
 * The address is a placeholder pending domain setup and mail to it is not yet
 * delivered. It is printed with that caveat rather than hidden, and rather
 * than a contact form that would accept a message and drop it.
 */
export const CONTACT_EMAIL = "contact@algorik.ai"

/** The posture label. Rendered wherever posture is shown, in these words. */
export const POSTURE = "PAPER TRADING"

/** Primary navigation, in order. Children render as a dropdown on desktop. */
export const NAV = [
    { label: "Home", href: "/" },
    {
        label: "Platform",
        href: "/platform",
        children: [
            { label: "Platform overview", href: "/platform" },
            { label: "Technology", href: "/technology" },
            { label: "Developers", href: "/developers" },
        ],
    },
    { label: "Security", href: "/security" },
    { label: "Institutional", href: "/institutional" },
    {
        label: "Company",
        href: "/company",
        children: [
            { label: "About Algorik", href: "/company" },
            { label: "Contact", href: "/contact" },
            { label: "Legal & disclosures", href: "/legal" },
        ],
    },
    { label: "Contact", href: "/contact" },
]

/** Every internal destination the chrome offers, flattened — the mobile menu
 *  renders this list whole, because a nested menu hides pages on the width
 *  where most visitors arrive. */
export const NAV_FLAT = [
    { label: "Home", href: "/" },
    { label: "Platform", href: "/platform" },
    { label: "Technology", href: "/technology" },
    { label: "Security", href: "/security" },
    { label: "Institutional", href: "/institutional" },
    { label: "Developers", href: "/developers" },
    { label: "Company", href: "/company" },
    { label: "Contact", href: "/contact" },
    { label: "Legal & disclosures", href: "/legal" },
]

export const FOOTER_COLUMNS = [
    {
        title: "Platform",
        links: [
            { label: "Platform overview", href: "/platform" },
            { label: "Technology", href: "/technology" },
            { label: "Developers", href: "/developers" },
            { label: "Security", href: "/security" },
        ],
    },
    {
        title: "Company",
        links: [
            { label: "About Algorik", href: "/company" },
            { label: "Institutional", href: "/institutional" },
            { label: "Contact", href: "/contact" },
        ],
    },
    {
        title: "Legal",
        links: [
            { label: "Legal & disclosures", href: "/legal" },
            { label: "Risk disclosures", href: "/legal/risk-disclosures" },
            { label: "Terms of service", href: "/legal/terms" },
            { label: "Privacy policy", href: "/legal/privacy" },
        ],
    },
    {
        title: "Trust",
        links: [
            { label: "Paper-trading boundary", href: "/security#boundary" },
            { label: "Hash-chained audit log", href: "/security#audit" },
            { label: "A classical baseline", href: "/technology#quantum" },
            { label: "The eight-stage loop", href: "/platform#loop" },
        ],
    },
]
