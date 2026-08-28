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
};

export const viewport: Viewport = {
  themeColor: "#07090c",
  colorScheme: "dark",
  width: "device-width",
  initialScale: 1,
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
