import { defineConfig, devices } from "@playwright/test";

/**
 * The landing's behavioural gate. It runs against the production build on
 * this app's own port and asserts the properties that make the front door
 * trustworthy: the posture is stated, nothing fabricated survived from the
 * demo content, and the doors lead to the portal.
 */
const PORT = Number(process.env.LANDING_PORT ?? 3500);

export default defineConfig({
  testDir: "./tests",
  fullyParallel: true,
  reporter: [["list"]],
  timeout: 45_000,
  use: {
    baseURL: `http://127.0.0.1:${PORT}`,
    trace: "retain-on-failure",
  },
  projects: [
    { name: "desktop", use: { ...devices["Desktop Chrome"], viewport: { width: 1440, height: 900 } } },
    { name: "phone", use: { ...devices["Pixel 7"] } },
  ],
  webServer: {
    command: "npm run start",
    url: `http://127.0.0.1:${PORT}`,
    reuseExistingServer: true,
    timeout: 60_000,
    env: {
      NEXT_PUBLIC_ALGORIK_PORTAL_URL: process.env.NEXT_PUBLIC_ALGORIK_PORTAL_URL ?? "http://127.0.0.1:3400",
    },
  },
});
