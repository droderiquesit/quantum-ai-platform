import type { Metadata, Viewport } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: {
    default: "Algorik — paper trading",
    template: "%s · Algorik (paper)",
  },
  description:
    "Algorik customer portal. Paper trading only: no control here can send a live order.",
  robots: { index: false, follow: false },
  applicationName: "Algorik",
  manifest: "/manifest.webmanifest",
  icons: {
    icon: [
      { url: "/brand/favicon.ico", sizes: "any" },
      { url: "/brand/android-chrome-192x192.png", sizes: "192x192", type: "image/png" },
      { url: "/brand/android-chrome-512x512.png", sizes: "512x512", type: "image/png" },
    ],
    apple: [{ url: "/brand/apple-touch-icon.png", sizes: "180x180", type: "image/png" }],
  },
  /**
   * iOS installs from Safari's share sheet and reads these rather than the
   * manifest. Without them the home-screen launch opens browser chrome with an
   * address bar competing with the paper-trading declaration for the same strip.
   */
  appleWebApp: {
    capable: true,
    title: "Algorik",
    statusBarStyle: "black",
  },
  formatDetection: { telephone: false, date: false, address: false, email: false },
};

export const viewport: Viewport = {
  themeColor: "#050709",
  width: "device-width",
  initialScale: 1,
  // The installed app runs edge to edge; fixed controls position themselves
  // off env(safe-area-inset-*) rather than off the viewport edge.
  viewportFit: "cover",
};

/**
 * Applies a stored light-theme choice before first paint.
 *
 * Inline and blocking on purpose: run after hydration this would paint dark
 * first and flash to light on every navigation for everyone who chose light.
 * Dark is the default and stores no attribute, so a browser with storage
 * denied simply gets the default.
 */
import { ThemeSync } from "@/components/chrome/ThemeSync";

const THEME_BOOT = `try{document.documentElement.dataset.theme=localStorage.getItem("algorik.theme")==="light"?"light":"dark"}catch(e){document.documentElement.dataset.theme="dark"}`;

/**
 * The root layout carries only what every surface shares: the document, the
 * theme boot, and the metadata defaults. Chrome belongs to the route groups —
 * the portal wears the console shell, marketing wears its own header, and the
 * auth pages wear almost nothing, which is the point of the split.
 */
export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en" suppressHydrationWarning>
      <body>
        <script dangerouslySetInnerHTML={{ __html: THEME_BOOT }} />
        <ThemeSync />
        {children}
      </body>
    </html>
  );
}
