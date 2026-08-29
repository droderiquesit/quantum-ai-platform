import type { Metadata, Viewport } from "next";
import { AppShell } from "@/components/chrome/AppShell";
import "./globals.css";

export const metadata: Metadata = {
  title: {
    default: "QIP Command Centre — paper trading",
    template: "%s · QIP Command Centre (paper)",
  },
  description:
    "Operator console for the QIP platform. Paper trading only: no control here can send a live order.",
  robots: { index: false, follow: false },
  applicationName: "QIP Paper",
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
   * manifest. Without them the home-screen launch opens a browser chrome with
   * an address bar, which on a console whose top banner is the paper-trading
   * declaration means the declaration competes with a URL for the same strip.
   */
  appleWebApp: {
    capable: true,
    title: "QIP Paper",
    // "black-translucent" draws the page under the status bar. The banner is
    // the first thing on the page and must not sit under the clock.
    statusBarStyle: "black",
  },
  formatDetection: { telephone: false, date: false, address: false, email: false },
};

export const viewport: Viewport = {
  themeColor: "#07090c",
  colorScheme: "dark",
  width: "device-width",
  initialScale: 1,
  // The installed app runs edge to edge; every fixed control then positions
  // itself off `env(safe-area-inset-*)` rather than off the viewport edge.
  viewportFit: "cover",
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en">
      <body>
        <AppShell>{children}</AppShell>
      </body>
    </html>
  );
}
