/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: market-overview.js
 * description: Self-contained controller for the Market Overview page
 *              (#market-overview). DOM-only — no HTML is generated in JS;
 *              all markup lives in market-overview.html as static templates.
 * author: SignalAIX

    -------------------------------------------------
    Table of Contents
    -------------------------------------------------
    01. Init & guard
    02. Toast
    03. Market tabs (Forex / Crypto / Indices)
    04. Filters + search (trend / signals + text)
    05. Export menu
    06. Top Movers tab toggle
    07. Economic-events impact filter
    08. Refresh
    09. Drawer (7 panels) + in-drawer pills
    10. Pair-details chart (per open)
    11. Performance + Accuracy charts (+ theme re-color)
    ================================================== */

(function () {
  /* ------------------------------------------------------------------ */
  /* 01. Init & guard                                                   */
  /* ------------------------------------------------------------------ */
  if (!document.getElementById("market-overview")) return;

  const isDark = () => document.documentElement.classList.contains("dark");

  /* ------------------------------------------------------------------ */
  /* 02. Toast                                                          */
  /* ------------------------------------------------------------------ */
  const toast = document.getElementById("moToast");
  const toastTitle = document.getElementById("moToastTitle");
  const toastMsg = document.getElementById("moToastMessage");
  let toastTimer;

  function showToast(title, message) {
    if (!toast) return;
    if (title) toastTitle.textContent = title;
    if (message) toastMsg.textContent = message;
    toast.classList.add("active");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => toast.classList.remove("active"), 3000);
  }
  document.getElementById("moToastClose")?.addEventListener("click", () => toast.classList.remove("active"));

  // Generic toast triggers (drawer buttons carry data-toast-title / -msg / -close)
  document.querySelectorAll(".mo-toast-btn").forEach((btn) => {
    btn.addEventListener("click", () => {
      showToast(btn.dataset.toastTitle, btn.dataset.toastMsg);
      if (btn.hasAttribute("data-close")) closeDrawer();
    });
  });

  /* ------------------------------------------------------------------ */
  /* 03. Market tabs                                                    */
  /* ------------------------------------------------------------------ */
  const marketTabs = document.querySelectorAll(".mo-market-tab");
  let activeMarket = "forex";

  function setTabActive(tab, on) {
    tab.classList.toggle("active", on);
    tab.classList.toggle("bg-accent", on);
    tab.classList.toggle("text-white", on);
    tab.classList.toggle("text-muted", !on);
  }

  function switchMarketTab(market) {
    activeMarket = market;
    marketTabs.forEach((t) => setTabActive(t, t.dataset.tab === market));
    document.querySelectorAll(".mo-tbody").forEach((tb) => tb.classList.toggle("hidden", tb.dataset.market !== market));
    // re-apply current filter + search to the now-visible table
    applyMarketFilter();
  }

  marketTabs.forEach((t) => {
    setTabActive(t, t.dataset.tab === activeMarket);
    t.addEventListener("click", () => switchMarketTab(t.dataset.tab));
  });

  /* ------------------------------------------------------------------ */
  /* 04. Filters + search                                               */
  /* ------------------------------------------------------------------ */
  const filterBtns = document.querySelectorAll(".mo-filter");
  const searchInput = document.getElementById("moSearch");
  const emptyMsg = document.querySelector(".mo-empty");
  let activeFilter = "all";

  function setFilterActive(btn, on) {
    btn.classList.toggle("active", on);
    btn.classList.toggle("bg-accent/10", on);
    btn.classList.toggle("border-accent", on);
    btn.classList.toggle("text-accent", on);
    btn.classList.toggle("text-muted", !on);
  }

  function applyMarketFilter() {
    const term = (searchInput?.value || "").toLowerCase();
    const tbody = document.querySelector(`.mo-tbody[data-market="${activeMarket}"]`);
    if (!tbody) return;
    let visible = 0;
    tbody.querySelectorAll("tr").forEach((row) => {
      let show = true;
      if (activeFilter === "bullish") show = row.dataset.trend === "bullish";
      else if (activeFilter === "bearish") show = row.dataset.trend === "bearish";
      else if (activeFilter === "signals") show = row.dataset.signal === "true";
      if (show && term) show = (row.dataset.pair || "").toLowerCase().includes(term);
      row.style.display = show ? "" : "none";
      if (show) visible++;
    });
    if (emptyMsg) emptyMsg.classList.toggle("hidden", visible !== 0);
  }

  filterBtns.forEach((btn) => {
    setFilterActive(btn, btn.dataset.filter === activeFilter);
    btn.addEventListener("click", () => {
      activeFilter = btn.dataset.filter;
      filterBtns.forEach((b) => setFilterActive(b, b === btn));
      applyMarketFilter();
    });
  });
  searchInput?.addEventListener("input", applyMarketFilter);

  /* ------------------------------------------------------------------ */
  /* 05. Export menu                                                    */
  /* ------------------------------------------------------------------ */
  document.querySelectorAll(".mo-export").forEach((item) => {
    item.addEventListener("click", () => {
      const fmt = (item.dataset.format || "").toUpperCase();
      item.closest(".dropdown-menu")?.classList.remove("active");
      showToast("Export Started", `Exporting market data as ${fmt}…`);
    });
  });

  /* ------------------------------------------------------------------ */
  /* 06. Top Movers tab toggle                                          */
  /* ------------------------------------------------------------------ */
  const moversTabs = document.querySelectorAll(".mo-movers-tab");
  function setMoversActive(btn, on) {
    btn.classList.toggle("active", on);
    btn.classList.toggle("bg-accent", on);
    btn.classList.toggle("text-white", on);
    btn.classList.toggle("text-muted", !on);
  }
  moversTabs.forEach((btn) => {
    setMoversActive(btn, btn.classList.contains("active"));
    btn.addEventListener("click", () => {
      const which = btn.dataset.movers;
      moversTabs.forEach((b) => setMoversActive(b, b === btn));
      document.querySelectorAll(".mo-movers-pane").forEach((p) => p.classList.toggle("hidden", p.dataset.movers !== which));
    });
  });

  /* ------------------------------------------------------------------ */
  /* 07. Economic-events impact filter                                  */
  /* ------------------------------------------------------------------ */
  const eventFilters = document.querySelectorAll(".mo-event-filter");
  let activeImpact = "all";
  function setEventFilterActive(btn, on) {
    btn.classList.toggle("active", on);
    btn.classList.toggle("bg-accent/10", on);
    btn.classList.toggle("border-accent", on);
    btn.classList.toggle("text-accent", on);
    btn.classList.toggle("text-muted", !on);
  }
  eventFilters.forEach((btn) => {
    setEventFilterActive(btn, btn.dataset.impact === activeImpact);
    btn.addEventListener("click", () => {
      activeImpact = btn.dataset.impact;
      eventFilters.forEach((b) => setEventFilterActive(b, b === btn));
      document.querySelectorAll("#moEventsBody tr").forEach((row) => {
        row.style.display = activeImpact === "all" || row.dataset.impact === activeImpact ? "" : "none";
      });
    });
  });

  /* ------------------------------------------------------------------ */
  /* 08. Refresh                                                        */
  /* ------------------------------------------------------------------ */
  document.getElementById("moRefresh")?.addEventListener("click", () => {
    document.getElementById("moLastUpdated").textContent = "Just now";
    showToast("Data Refreshed", "Market data has been updated");
  });

  /* ------------------------------------------------------------------ */
  /* 09. Drawer (7 panels) + in-drawer pills                            */
  /* ------------------------------------------------------------------ */
  const drawer = document.getElementById("moDrawer");
  const overlay = document.getElementById("moDrawerOverlay");
  const drawerTitle = document.getElementById("moDrawerTitle");
  const panels = drawer ? drawer.querySelectorAll(".mo-panel") : [];

  const TITLES = {
    "ai-insights": "AI Market Insights",
    signals: "Active Signals",
    "ai-generator": "AI Signal Generator",
    alerts: "Create Alert",
    scanner: "Market Scanner",
    compare: "Compare Pairs",
    calendar: "Economic Calendar",
    pair: "Pair Details",
  };

  function showPanel(name) {
    panels.forEach((p) => (p.hidden = p.dataset.panel !== name));
    if (drawerTitle) drawerTitle.textContent = TITLES[name] || "Details";
  }

  function openDrawer(name) {
    showPanel(name);
    overlay?.classList.add("active");
    drawer?.classList.add("active");
    if (window.lucide) lucide.createIcons();
  }

  function closeDrawer() {
    overlay?.classList.remove("active");
    drawer?.classList.remove("active");
  }

  document.querySelectorAll(".mo-open-drawer").forEach((btn) => {
    btn.addEventListener("click", () => openDrawer(btn.dataset.drawer));
  });
  drawer?.querySelectorAll(".mo-drawer-close").forEach((b) => b.addEventListener("click", closeDrawer));
  overlay?.addEventListener("click", closeDrawer);
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") closeDrawer();
  });

  // In-drawer selectable pill groups (timeframe / risk / market / signal-type)
  drawer?.querySelectorAll(".mo-pill").forEach((pill) => {
    pill.addEventListener("click", () => {
      const group = pill.parentElement;
      group.querySelectorAll(".mo-pill").forEach((p) => {
        p.classList.remove("active", "bg-accent/10", "border-accent", "text-accent");
        p.classList.add("text-muted");
      });
      pill.classList.add("active", "bg-accent/10", "border-accent", "text-accent");
      pill.classList.remove("text-muted");
    });
  });
  // initialize pre-selected pills' look
  drawer?.querySelectorAll(".mo-pill.active").forEach((p) => {
    p.classList.add("bg-accent/10", "border-accent", "text-accent");
    p.classList.remove("text-muted");
  });

  /* ------------------------------------------------------------------ */
  /* 10. Pair-details (open via table external-link) + per-open chart   */
  /* ------------------------------------------------------------------ */
  const PAIR_META = {
    "EUR/USD": { badge: "EUR", price: "1.0852", change: "+0.45%", signal: "BUY", conf: "85%", base: 1.085 },
    "GBP/USD": { badge: "GBP", price: "1.2678", change: "+0.32%", signal: "BUY", conf: "78%", base: 1.268 },
    "USD/JPY": { badge: "USD", price: "149.82", change: "-0.18%", signal: "SELL", conf: "72%", base: 149.8 },
    "AUD/USD": { badge: "AUD", price: "0.6542", change: "+0.28%", signal: "HOLD", conf: "65%", base: 0.654 },
    "USD/CAD": { badge: "USD", price: "1.3521", change: "-0.15%", signal: "SELL", conf: "70%", base: 1.352 },
    "EUR/GBP": { badge: "EUR", price: "0.8562", change: "+0.12%", signal: "HOLD", conf: "58%", base: 0.856 },
    "NZD/USD": { badge: "NZD", price: "0.6125", change: "+0.38%", signal: "BUY", conf: "82%", base: 0.612 },
    "USD/CHF": { badge: "USD", price: "0.8742", change: "-0.22%", signal: "SELL", conf: "75%", base: 0.874 },
    "BTC/USD": { badge: "BTC", price: "67,245.80", change: "+2.45%", signal: "BUY", conf: "88%", base: 67000 },
    "ETH/USD": { badge: "ETH", price: "3,542.60", change: "+3.12%", signal: "BUY", conf: "85%", base: 3540 },
    "SOL/USD": { badge: "SOL", price: "142.35", change: "+5.82%", signal: "BUY", conf: "82%", base: 142 },
    "XRP/USD": { badge: "XRP", price: "0.5842", change: "-1.25%", signal: "HOLD", conf: "62%", base: 0.584 },
    "ADA/USD": { badge: "ADA", price: "0.4521", change: "+1.85%", signal: "BUY", conf: "75%", base: 0.452 },
    "DOT/USD": { badge: "DOT", price: "7.85", change: "+2.15%", signal: "HOLD", conf: "68%", base: 7.85 },
    "S&P 500": { badge: "S&P", price: "5,234.18", change: "+0.85%", signal: "BUY", conf: "80%", base: 5234 },
    NASDAQ: { badge: "NAS", price: "16,428.82", change: "+1.12%", signal: "BUY", conf: "82%", base: 16428 },
    "DOW 30": { badge: "DOW", price: "39,127.14", change: "+0.45%", signal: "HOLD", conf: "72%", base: 39127 },
    DAX: { badge: "DAX", price: "18,245.50", change: "-0.32%", signal: "HOLD", conf: "65%", base: 18245 },
    "FTSE 100": { badge: "FTS", price: "8,142.85", change: "+0.28%", signal: "BUY", conf: "75%", base: 8142 },
  };

  const pairBadge = document.getElementById("moPairBadge");
  const pairName = document.getElementById("moPairName");
  const pairPrice = document.getElementById("moPairPrice");
  const pairChange = document.getElementById("moPairChange");
  const pairSignal = document.getElementById("moPairSignal");
  const pairConfidence = document.getElementById("moPairConfidence");
  let pairChart;

  function openPairDetails(pair) {
    const m = PAIR_META[pair] || PAIR_META["EUR/USD"];
    const up = m.change.startsWith("+");
    if (pairBadge) pairBadge.textContent = m.badge;
    if (pairName) pairName.textContent = pair;
    if (pairPrice) pairPrice.textContent = m.price;
    if (pairChange) {
      pairChange.textContent = m.change;
      pairChange.className = "font-mono font-semibold " + (up ? "text-emerald-500" : "text-red-500");
    }
    if (pairSignal) {
      pairSignal.textContent = m.signal;
      pairSignal.className = "font-semibold " + (m.signal === "BUY" ? "text-emerald-500" : m.signal === "SELL" ? "text-red-500" : "text-amber-500");
    }
    if (pairConfidence) pairConfidence.textContent = m.conf;
    openDrawer("pair");
    renderPairChart(m, up);
  }

  function renderPairChart(m, up) {
    if (typeof Chart === "undefined") return;
    const canvas = document.getElementById("moPairChart");
    if (!canvas) return;
    if (pairChart) pairChart.destroy();
    const color = up ? "#10b981" : "#ef4444";
    const fill = up ? "rgba(16,185,129,0.12)" : "rgba(239,68,68,0.12)";
    // deterministic wiggle around the base price (no Math.random — keeps re-renders stable)
    const wiggle = [-0.4, 0.2, -0.1, 0.5, 0.3, 0.8, 0.6, 1];
    const data = wiggle.map((w) => +(m.base * (1 + (w * 0.004 * (up ? 1 : -1)))).toFixed(m.base < 10 ? 4 : 2));
    requestAnimationFrame(() => {
      const tick = isDark() ? "#94A3B8" : "#64748B";
      const grid = isDark() ? "#1A1F2E" : "#E2E8F0";
      pairChart = new Chart(canvas.getContext("2d"), {
        type: "line",
        data: {
          labels: ["", "", "", "", "", "", "", ""],
          datasets: [{ data, borderColor: color, backgroundColor: fill, fill: true, tension: 0.4, pointRadius: 0, borderWidth: 2 }],
        },
        options: {
          responsive: true,
          maintainAspectRatio: false,
          plugins: { legend: { display: false } },
          scales: {
            x: { grid: { display: false }, ticks: { display: false } },
            y: { grid: { color: grid }, ticks: { color: tick, maxTicksLimit: 4 } },
          },
        },
      });
    });
  }

  document.querySelectorAll(".mo-pair-details").forEach((btn) => {
    btn.addEventListener("click", () => openPairDetails(btn.dataset.pair));
  });

  /* ------------------------------------------------------------------ */
  /* 11. Performance + Accuracy charts (+ theme re-color)               */
  /* ------------------------------------------------------------------ */
  let performanceChart, accuracyChart;

  function initCharts() {
    if (typeof Chart === "undefined") return;
    const tick = isDark() ? "#94A3B8" : "#64748B";
    const grid = isDark() ? "#1A1F2E" : "#E2E8F0";

    const perf = document.getElementById("moPerformanceChart");
    if (perf) {
      performanceChart = new Chart(perf.getContext("2d"), {
        type: "line",
        data: {
          labels: ["00:00", "04:00", "08:00", "12:00", "16:00", "20:00", "24:00"],
          datasets: [
            { label: "EUR/USD", data: [1.082, 1.0835, 1.0842, 1.0855, 1.0848, 1.0862, 1.0852], borderColor: "#10b981", backgroundColor: "rgba(16,185,129,0.1)", fill: true, tension: 0.4, pointRadius: 0, borderWidth: 2 },
            { label: "GBP/USD", data: [1.264, 1.2655, 1.2668, 1.268, 1.2672, 1.269, 1.2678], borderColor: "#6366f1", backgroundColor: "rgba(99,102,241,0.1)", fill: true, tension: 0.4, pointRadius: 0, borderWidth: 2 },
          ],
        },
        options: {
          responsive: true,
          maintainAspectRatio: false,
          plugins: { legend: { position: "top", labels: { color: tick, usePointStyle: true, pointStyle: "circle" } } },
          scales: { x: { grid: { color: grid }, ticks: { color: tick } }, y: { grid: { color: grid }, ticks: { color: tick } } },
        },
      });
    }

    const acc = document.getElementById("moAccuracyChart");
    if (acc) {
      accuracyChart = new Chart(acc.getContext("2d"), {
        type: "bar",
        data: {
          labels: ["Week 1", "Week 2", "Week 3", "Week 4"],
          datasets: [
            { label: "Win Rate %", data: [72, 78, 75, 82], backgroundColor: "rgba(16,185,129,0.85)", borderRadius: 8 },
            { label: "AI Confidence %", data: [68, 75, 72, 80], backgroundColor: "rgba(99,102,241,0.85)", borderRadius: 8 },
          ],
        },
        options: {
          responsive: true,
          maintainAspectRatio: false,
          plugins: { legend: { position: "top", labels: { color: tick, usePointStyle: true, pointStyle: "circle" } } },
          scales: { x: { grid: { display: false }, ticks: { color: tick } }, y: { grid: { color: grid }, ticks: { color: tick }, max: 100 } },
        },
      });
    }
  }

  function recolorCharts() {
    const tick = isDark() ? "#94A3B8" : "#64748B";
    const grid = isDark() ? "#1A1F2E" : "#E2E8F0";
    [performanceChart, accuracyChart, pairChart].forEach((c) => {
      if (!c) return;
      if (c.options.scales.x.grid) c.options.scales.x.grid.color = grid;
      if (c.options.scales.x.ticks) c.options.scales.x.ticks.color = tick;
      if (c.options.scales.y.grid) c.options.scales.y.grid.color = grid;
      if (c.options.scales.y.ticks) c.options.scales.y.ticks.color = tick;
      if (c.options.plugins.legend?.labels) c.options.plugins.legend.labels.color = tick;
      c.update();
    });
  }

  document.getElementById("themeToggle")?.addEventListener("click", () => setTimeout(recolorCharts, 0));

  // Performance period selector — just a toast (data is illustrative)
  document.getElementById("moPerformanceSelect")?.addEventListener("change", (e) => {
    showToast("Timeframe Updated", `Showing ${e.target.options[e.target.selectedIndex].text} performance`);
  });

  // Charts depend on the CDN Chart.js (loaded with defer) — init after load.
  if (document.readyState === "complete") initCharts();
  else window.addEventListener("load", initCharts);

  // Apply the default filter on first paint.
  applyMarketFilter();
})();
