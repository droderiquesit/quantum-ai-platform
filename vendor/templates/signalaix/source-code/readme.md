# SignalAIX — AI Forex & Crypto Trading Signals HTML Template

Thank you for purchasing **SignalAIX**! This documentation will help you get started.

SignalAIX is a static, multi-page **admin/dashboard HTML template** for an AI-powered
**Forex & Crypto trading-signals / market-prediction** platform. It ships ~40 fully-built
pages across 8 menu sections (Dashboard, Live Signals, AI Intelligence, Trading Tools,
Market Analytics, Portfolio, Watchlists, Automation, Account) with live-style charts,
filterable tables, slide-in drawers, and a light/dark theme.

## 🚀 Features

- **Modern stack** — HTML5, Webpack 5, and **Tailwind CSS v4** (CSS-first config, no `tailwind.config.js`).
- **No framework, no jQuery** — pages are plain HTML enhanced with small **vanilla JS** modules.
- **Light / Dark mode** — emerald-accent theme driven by CSS variables; preference saved to `localStorage`.
- **Fully responsive down to 320px** — every page is built and tested at 320px with no horizontal scroll;
  layout adapts to the real content width (the sidebar collapses to an icon rail, then off-canvas on mobile).
- **Real Chart.js charts** — line, bar, doughnut, radar, gauges, sparklines (not images), all re-coloring on theme toggle.
- **Rich interactive UI** — slide-in drawers, toasts, tabs, multi-filter + search, sortable tables,
  segmented toggles, range sliders, copy-to-clipboard, and a video player.
- **Shared layout** — one sidebar + top header partial inlined into every page at build time.
- **Auto page discovery** — drop a new `.html` in `src/` and it's built automatically (no config edit).
- **Lucide icons** via lightweight inline SVG.

## 📁 Project Structure

```bash
signalaix-html/
├── src/                     # Source files (edit these)
│   ├── *.html               # ~40 dashboard pages (one file per page)
│   ├── partials/            # Shared layout inlined at build time
│   │   ├── sidebar.html      #   left navigation (all menu sections)
│   │   └── top-header.html   #   top bar (search, theme toggle, profile)
│   ├── css/
│   │   ├── common.css        #   Tailwind import + theme tokens + minimal behavior layer
│   │   └── loader.css        #   page loader styles
│   ├── js/
│   │   ├── index.js          #   single entry point (imports CSS + all page scripts)
│   │   ├── common.js         #   app-wide systems (theme, drawers, toasts, etc.)
│   │   ├── sidebar.js        #   sidebar collapse / mobile open / active nav
│   │   ├── header.js         #   top-header behavior
│   │   └── <page>.js         #   one self-contained controller per interactive page
│   └── assets/              # images, fonts, favicon (copied verbatim to dist/)
├── dist/                    # Build output (generated — do not edit)
├── webpack.config.js        # Webpack config (auto-discovers pages, inlines partials)
├── watch-new-files.js       # dev helper: restarts the server when pages/partials change
├── package.json             # dependencies and scripts
└── readme.md                # this file
```

## 🛠️ Installation & Setup

1. **Install Node.js** (v18+ recommended).
2. **Install dependencies** in the project root:
   ```bash
   npm install
   ```
3. **Start the dev server** (hot reload):
   ```bash
   npm start
   ```
   The site runs at **`http://localhost:5001`**.

   > Use **`npm run dev`** instead of `npm start` if you are **adding or deleting** an `.html` page or
   > editing a partial — it watches for those changes and restarts the server so the new page is picked up.

4. **Build for production:**
   ```bash
   npm run build
   ```
   The optimized, minified files are written to the **`dist/`** folder, ready to upload to any static host.

## 🎨 Customization

### Colors & theme

All theme colors are CSS variables in **`src/css/common.css`**, inside the `@theme { … }` block (light mode)
and the `.dark { … }` override (dark mode). The primary accent is emerald `#10b981`:

```css
@theme {
  --color-accent: #10b981;   /* primary accent (brand emerald) */
  --color-bg:     #f8fafc;   /* page background */
  --color-panel:  #ffffff;   /* cards / surfaces */
  --color-text:   #0f172a;   /* primary text */
  --color-muted:  #64748b;   /* secondary text */
  --color-border: #e2e8f0;   /* borders */
}
.dark { --color-bg: #050709; --color-panel: #0a0d12; /* …dark overrides… */ }
```

Change a value once and it updates everywhere (the UI uses Tailwind classes like `bg-accent`, `text-text`,
`border-border`, `bg-panel` that map to these tokens). Light/dark switch automatically.

### Styling components

Component styling is written as **Tailwind utility classes directly in the HTML** — there are no custom
component CSS files to hunt through. Edit the markup in `src/*.html` to restyle a card, button, badge, etc.
`common.css` only holds the design tokens plus the minimal CSS the JS needs to toggle interactive state
(drawers, tabs, toggles, sliders).

### Layout (sidebar / header)

Shared layout lives in **`src/partials/`**. Editing `sidebar.html` or `top-header.html` updates **every**
page (after a rebuild). Menu links live in `sidebar.html`.

### Adding a page

Drop a new `.html` file into `src/` (copy an existing page as a starting point — keep the
`<include-sidebar />` and `<include-top-header />` tags and the two CDN `<script>` tags at the bottom).
Webpack discovers it automatically. If the page needs its own JavaScript, add `src/js/<page>.js` and
import it in `src/js/index.js`. Restart with `npm run dev` so the new page is detected.

## 📦 Credits & Libraries

- **Tailwind CSS v4** — https://tailwindcss.com/
- **Chart.js** — https://www.chartjs.org/
- **Lucide Icons** — https://lucide.dev/
- **Swiper** — https://swiperjs.com/
- **Webpack 5** — https://webpack.js.org/
- **Fonts** — Inter & Rajdhani (Google Fonts)

## 📞 Support

If you have a question that's beyond the scope of this help file, please reach out via the contact form
on my ThemeForest user page. Thanks so much for your purchase!
