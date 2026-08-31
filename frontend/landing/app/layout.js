import "../public/assets/css/bootstrap.css"
import "../public/assets/css/style.css"
import "./algorik.css"
import 'swiper/css'
import "swiper/css/pagination"
import 'swiper/css/free-mode'
import { barlow, firaSans } from '@/lib/font'

export const metadata = {
    title: {
        default: "Algorik — AI and quantum research for investment decisions",
        template: "%s — Algorik",
    },
    description:
        "Algorik senses global markets, reasons with a panel of AI agents, and executes only against simulators. Paper trading, hash-chained auditability, quantum research measured against a classical baseline.",
    applicationName: "Algorik",
    robots: { index: true, follow: true },
    icons: {
        icon: "/favicon.ico",
        apple: "/apple-touch-icon.png",
    },
}

export default function RootLayout({ children }) {
    return (
        <html lang="en" className={`${firaSans.variable} ${barlow.variable}`}>
            <body>
                {/* A keyboard visitor should not have to tab the whole navigation
                    on every page to reach the page. */}
                <a className="algorik-skip" href="#main-content">Skip to content</a>
                {children}
            </body>
        </html>
    )
}
