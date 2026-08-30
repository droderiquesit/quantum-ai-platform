import Link from "next/link"

/**
 * The landing navigation, trimmed to pages that exist and say true things.
 * The template's mega-menu enumerated demo routes; a public site that links
 * to lorem is marketing debt from day one.
 */
export default function Menu() {
    return (
        <>
            <ul className="navigation clearfix">
                <li><Link href="/">Home</Link></li>
                <li><Link href="/platform">Platform</Link></li>
                <li><Link href="/about">Company</Link></li>
                <li><Link href="/contact">Contact</Link></li>
            </ul>
        </>
    )
}
