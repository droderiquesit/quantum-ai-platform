import type { MetadataRoute } from "next";

/**
 * The installable app, for a phone home screen on either platform.
 *
 * There is no native application and no app store listing. This is the same
 * console, installed as a progressive web app — which is what lets one codebase
 * carry the paper-trading guarantees onto a phone. A second, native client
 * would be a second place those guarantees could drift, and the frontend rules
 * put no trading or risk logic in the client for exactly that reason.
 *
 * `start_url` carries `?surface=installed` so the console can tell a launch
 * from the home screen apart from a browser visit without guessing at
 * `display-mode`, which is unreliable on iOS.
 */
export default function manifest(): MetadataRoute.Manifest {
  return {
    name: "Algorik — paper trading",
    short_name: "Algorik",
    // Read on the install sheet, where it may be the only sentence a person
    // sees before adding this to a home screen. It says the boundary first.
    description:
      "Algorik customer portal — paper trading. Simulated execution only: no control in this application can submit a live order.",
    id: "/?surface=installed",
    start_url: "/?surface=installed",
    scope: "/",
    display: "standalone",
    orientation: "portrait-primary",
    background_color: "#050709",
    theme_color: "#050709",
    categories: ["finance", "productivity"],
    dir: "ltr",
    lang: "en",
    icons: [
      { src: "/brand/android-chrome-192x192.png", sizes: "192x192", type: "image/png", purpose: "any" },
      { src: "/brand/android-chrome-512x512.png", sizes: "512x512", type: "image/png", purpose: "any" },
      {
        src: "/brand/maskable-192.png",
        sizes: "192x192",
        type: "image/png",
        purpose: "maskable",
      },
      {
        src: "/brand/maskable-512.png",
        sizes: "512x512",
        type: "image/png",
        purpose: "maskable",
      },
    ],
    shortcuts: [
      {
        name: "Risk",
        short_name: "Risk",
        url: "/risk?surface=installed",
        description: "Exposure, concentration, limits and the kill switch",
      },
      {
        name: "Opportunities",
        short_name: "Opps",
        url: "/signals?surface=installed",
        description: "The opportunity queue and the signal stream",
      },
      {
        name: "Orders",
        short_name: "Orders",
        url: "/orders?surface=installed",
        description: "Order lifecycle, fills and refusals",
      },
    ],
  };
}
