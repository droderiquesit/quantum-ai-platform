import "@/node_modules/react-modal-video/css/modal-video.css"
import "../public/assets/css/bootstrap.css"
import "../public/assets/css/style.css"
import 'swiper/css'
import "swiper/css/pagination"
import 'swiper/css/free-mode';
import { barlow, firaSans } from '@/lib/font'
export const metadata = {
    title: "Algorik — AI & quantum research trading platform",
    description: "Algorik senses global markets, reasons with AI agents, and executes only against simulators. Paper trading, hash-chained auditability, quantum research with a classical baseline.",
}

export default function RootLayout({ children }) {
    return (
        <html lang="en" className={`${firaSans.variable} ${barlow.variable}`}>
            <body>{children}</body>
        </html>
    )
}
