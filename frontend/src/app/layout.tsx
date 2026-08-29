import type { Metadata, Viewport } from "next";
import { AppShell } from "@/components/chrome/AppShell";
import "./globals.css";

export const metadata: Metadata = {
  title: {
    default: "PEOS Quantum AI — paper trading",
    template: "%s · PEOS Quantum AI (paper)",
  },
  description:
    "Operator console for the PEOS Quantum AI platform. Paper trading only: no control here can send a live order.",
  robots: { index: false, follow: false },
  applicationName: "PEOS Quantum AI",
  manifest: "/manifest.webmanifest",
  icons: {
    icon: [
      { url: "/icon.svg", type: "image/svg+xml" },
      { url: "/icons/icon-192.png", sizes: "192x192", type: "image/png" },
      { url: "/icons/icon-512.png", sizes: "512x512", type: "image/png" },
    ],
    apple: [{ url: "/icons/apple-touch-icon.png", sizes: "180x180", type: "image/png" }],
  },
  /**
   * iOS installs from Safari's share sheet and reads these rather than the
   * manifest. Without them the home-screen launch opens browser chrome with an
   * address bar competing with the paper-trading declaration for the same strip.
   */
  appleWebApp: {
    capable: true,
    title: "PEOS Quantum AI",
    statusBarStyle: "black",
  },
  formatDetection: { telephone: false, date: false, address: false, email: false },
};

export const viewport: Viewport = {
  themeColor: "#07060e",
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
const THEME_BOOT = `try{if(localStorage.getItem("peos.theme")==="light")document.documentElement.dataset.theme="light"}catch(e){}`;

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en" suppressHydrationWarning>
      <body>
        <script dangerouslySetInnerHTML={{ __html: THEME_BOOT }} />
        <AppShell>{children}</AppShell>
      </body>
    </html>
  );
}
