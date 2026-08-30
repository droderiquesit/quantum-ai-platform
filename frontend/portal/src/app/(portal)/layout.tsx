import { AppShell } from "@/components/chrome/AppShell";

/**
 * The portal group: everything an authenticated operator sees.
 *
 * The shell — banner, header, navigation, status bar, tab bar — lives here
 * rather than in the root layout so the marketing and authentication groups
 * can exist without wearing a trading console's chrome. Route groups do not
 * appear in URLs, so every portal path is exactly what it was before this
 * file existed.
 */
export default function PortalLayout({ children }: { children: React.ReactNode }) {
  return <AppShell>{children}</AppShell>;
}
