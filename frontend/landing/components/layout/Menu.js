import Link from "next/link"
import { NAV } from "@/lib/site"

/**
 * The desktop navigation, generated from one declaration of the site's
 * information architecture. It used to be a hand-written list that named four
 * of the site's pages; the rest were reachable only by guessing the URL.
 */
export default function Menu() {
    return (
        <ul className="navigation clearfix">
            {NAV.map((item) => (
                <li key={item.label} className={item.children ? "dropdown" : undefined}>
                    <Link href={item.href}>{item.label}</Link>
                    {item.children && (
                        <ul>
                            {item.children.map((child) => (
                                <li key={child.href}><Link href={child.href}>{child.label}</Link></li>
                            ))}
                        </ul>
                    )}
                </li>
            ))}
        </ul>
    )
}
