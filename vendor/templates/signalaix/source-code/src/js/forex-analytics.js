/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: forex-analytics.html
 * description: SignalAIX - Forex Analytics Page Controller
 *              Self-contained: mirrors the reference mockup's functionality
 *              (5 tabs, lazy tab charts, pair-detail / filter / export drawers,
 *               pair-type filter chips, activity table search, toast).
 *              All markup lives in forex-analytics.html; this file only modifies
 *              the DOM (text/values/classes) — it never injects HTML strings.
 * author: SignalAIX

    -------------------------------------------------
     01. Init & DOM refs (guarded by #forex-analytics)
     -------------------------------------------------
     02. Toast
     -------------------------------------------------
     03. Drawers (pair / filter / export) + overlay/esc
     -------------------------------------------------
     04. Tabs + lazy tab charts
     -------------------------------------------------
     05. Pair-detail drawer (populate + per-open chart)
     -------------------------------------------------
     06. Pair-type filter chips
     -------------------------------------------------
     07. Activity table search
     -------------------------------------------------
     08. Charts (overview + tab charts) + theme re-color
     -------------------------------------------------
    ================================================== */

document.addEventListener("DOMContentLoaded", () => {
  /**
   * ======================================
   * 01. Init & DOM refs
   * ======================================
   */
  const page = document.getElementById("forex-analytics");
  if (!page) return; // Guard: only run on the Forex Analytics page

  const html = document.documentElement;
  const overlay = document.getElementById("faDrawerOverlay");
  const pairDrawer = document.getElementById("faPairDrawer");
  const filterDrawer = document.getElementById("faFilterDrawer");
  const exportDrawer = document.getElementById("faExportDrawer");
  const toast = document.getElementById("faToast");

  const refreshIcons = () => window.lucide?.createIcons?.();

  /**
   * ======================================
   * 02. Toast
   * ======================================
   */
  let toastTimer = null;
  function showToast(title, message) {
    if (!toast) return;
    document.getElementById("faToastTitle").textContent = title;
    document.getElementById("faToastMessage").textContent = message;
    toast.classList.add("active");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => toast.classList.remove("active"), 3000);
  }
  document
    .getElementById("faToastClose")
    ?.addEventListener("click", () => toast.classList.remove("active"));

  /**
   * ======================================
   * 03. Drawers (pair / filter / export)
   * ======================================
   */
  function openDrawer(drawer) {
    drawer?.classList.add("active");
    overlay?.classList.add("active");
  }
  function closeDrawers() {
    pairDrawer?.classList.remove("active");
    filterDrawer?.classList.remove("active");
    exportDrawer?.classList.remove("active");
    overlay?.classList.remove("active");
  }

  overlay?.addEventListener("click", closeDrawers);
  document
    .querySelectorAll(".fa-close-drawer")
    .forEach((b) => b.addEventListener("click", closeDrawers));

  document
    .querySelector(".fa-open-filter")
    ?.addEventListener("click", () => openDrawer(filterDrawer));
  document
    .querySelector(".fa-open-export")
    ?.addEventListener("click", () => openDrawer(exportDrawer));

  document
    .getElementById("faNewAnalysisBtn")
    ?.addEventListener("click", () =>
      showToast("New Analysis", "Creating new analysis report...")
    );

  // Filter drawer actions
  document.getElementById("faResetFilters")?.addEventListener("click", () => {
    filterDrawer
      ?.querySelectorAll('input[type="checkbox"]')
      .forEach((cb) => (cb.checked = true));
    showToast("Filters Reset", "All filters have been cleared");
  });
  document.getElementById("faApplyFilters")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Filters Applied", "Your filters have been applied");
  });

  // Export drawer action
  document.getElementById("faExportData")?.addEventListener("click", () => {
    const fmt =
      document.querySelector('input[name="faExportFormat"]:checked')?.value ||
      "csv";
    closeDrawers();
    showToast("Export Started", `Exporting data as ${fmt.toUpperCase()}...`);
  });

  // Toast buttons inside the pair drawer (Add to Watchlist / Set Alert)
  document.querySelectorAll(".fa-toast-btn").forEach((btn) => {
    btn.addEventListener("click", () =>
      showToast(
        btn.dataset.toastTitle || "Done",
        btn.dataset.toastMsg || "Action completed"
      )
    );
  });

  /**
   * ======================================
   * 04. Tabs + lazy tab charts
   * ======================================
   */
  const tabs = document.querySelectorAll(".fa-tab");
  const panels = document.querySelectorAll(".fa-panel");
  const ACTIVE = ["bg-accent", "text-white"];
  const IDLE = ["text-muted", "hover:text-text"];

  function setActiveTab(tabId) {
    tabs.forEach((t) => {
      const on = t.dataset.tab === tabId;
      t.classList.toggle("bg-accent", on);
      t.classList.toggle("text-white", on);
      t.classList.toggle("text-muted", !on);
      t.classList.toggle("hover:text-text", !on);
    });
    panels.forEach((p) => p.classList.add("hidden"));
    document.getElementById(`fa-${tabId}-panel`)?.classList.remove("hidden");
    initTabCharts(tabId);
  }

  tabs.forEach((tab) =>
    tab.addEventListener("click", () => setActiveTab(tab.dataset.tab))
  );

  /**
   * ======================================
   * 05. Pair-detail drawer
   * ======================================
   */
  const PAIR_DATA = {
    "EUR/USD": { price: "1.0842", change: "-0.18%", high: "1.0891", low: "1.0812", trend: "Bearish", signal: "Sell" },
    "GBP/USD": { price: "1.2654", change: "+0.32%", high: "1.2698", low: "1.2601", trend: "Bullish", signal: "Buy" },
    "USD/JPY": { price: "149.82", change: "+0.85%", high: "150.12", low: "148.92", trend: "Bullish", signal: "Buy" },
    "AUD/USD": { price: "0.6521", change: "+0.56%", high: "0.6548", low: "0.6485", trend: "Bullish", signal: "Buy" },
    "USD/CAD": { price: "1.3542", change: "-0.24%", high: "1.3589", low: "1.3512", trend: "Neutral", signal: "Hold" },
    "USD/CHF": { price: "0.8845", change: "-0.12%", high: "0.8901", low: "0.8821", trend: "Bearish", signal: "Sell" },
    "NZD/USD": { price: "0.5982", change: "+0.21%", high: "0.6011", low: "0.5950", trend: "Bullish", signal: "Buy" },
    "EUR/GBP": { price: "0.8612", change: "-0.28%", high: "0.8645", low: "0.8590", trend: "Bearish", signal: "Sell" },
    "EUR/JPY": { price: "162.45", change: "+1.12%", high: "162.80", low: "160.90", trend: "Bullish", signal: "Buy" },
    "GBP/JPY": { price: "189.42", change: "+1.24%", high: "189.90", low: "187.10", trend: "Bullish", signal: "Buy" },
    "AUD/JPY": { price: "97.65", change: "+0.45%", high: "97.92", low: "96.80", trend: "Bullish", signal: "Buy" },
    "USD/TRY": { price: "32.45", change: "+0.85%", high: "32.60", low: "32.10", trend: "Bullish", signal: "Buy" },
  };

  const pd = {
    name: document.getElementById("faPairName"),
    price: document.getElementById("faPdPrice"),
    change: document.getElementById("faPdChange"),
    changeIcon: document.getElementById("faPdChangeIcon"),
    changeBadge: document.getElementById("faPdChangeBadge"),
    high: document.getElementById("faPdHigh"),
    low: document.getElementById("faPdLow"),
    trend: document.getElementById("faPdTrend"),
    signal: document.getElementById("faPdSignal"),
    insight: document.getElementById("faPdInsight"),
  };

  function openPairDrawer(pair) {
    const d = PAIR_DATA[pair] || PAIR_DATA["EUR/USD"];
    const isPositive = d.change.startsWith("+");

    pd.name.textContent = pair;
    pd.price.textContent = d.price;
    pd.change.textContent = d.change;
    pd.high.textContent = d.high;
    pd.low.textContent = d.low;
    pd.trend.textContent = d.trend;
    pd.signal.textContent = d.signal;

    pd.changeIcon.setAttribute("data-lucide", isPositive ? "arrow-up" : "arrow-down");
    pd.changeBadge.className =
      "inline-flex items-center gap-1 px-3 py-1 rounded-full text-sm font-semibold " +
      (isPositive ? "bg-emerald-500/20 text-emerald-500" : "bg-red-500/20 text-red-500");

    pd.signal.className =
      "font-semibold " +
      (d.signal === "Buy"
        ? "text-emerald-500"
        : d.signal === "Sell"
        ? "text-red-500"
        : "text-amber-500");

    pd.insight.textContent = `${pair} shows consolidation near key support. Watch for breakout above ${d.high} for bullish continuation or breakdown below ${d.low} for bearish move.`;

    openDrawer(pairDrawer);
    refreshIcons();
    renderPairChart(pair, isPositive);
  }

  // Open from table rows, eye buttons, and pair cards (all carry data-pair)
  document.querySelectorAll(".fa-view-pair").forEach((el) => {
    el.addEventListener("click", (e) => {
      e.stopPropagation();
      openPairDrawer(el.dataset.pair);
    });
  });
  document.querySelectorAll(".fa-row").forEach((row) => {
    row.addEventListener("click", () => openPairDrawer(row.dataset.pair));
  });

  /**
   * ======================================
   * 06. Pair-type filter chips
   * ======================================
   */
  const pairFilters = document.querySelectorAll(".fa-pair-filter");
  const pairCards = document.querySelectorAll(".fa-pair-card");
  pairFilters.forEach((chip) => {
    chip.addEventListener("click", () => {
      const f = chip.dataset.filter;
      pairFilters.forEach((c) => {
        const on = c === chip;
        c.classList.toggle("border-accent", on);
        c.classList.toggle("bg-accent/15", on);
        c.classList.toggle("text-accent", on);
        c.classList.toggle("border-border", !on);
        c.classList.toggle("text-muted", !on);
      });
      pairCards.forEach((card) => {
        card.classList.toggle("hidden", f !== "all" && card.dataset.type !== f);
      });
      showToast("Filter Changed", `Showing ${f} pairs`);
    });
  });

  /**
   * ======================================
   * 07. Activity table search
   * ======================================
   */
  const tableSearch = document.getElementById("faTableSearch");
  const activityRows = document.querySelectorAll("#faActivityTable tbody tr");
  const noActivity = document.getElementById("faNoActivity");
  tableSearch?.addEventListener("input", function () {
    const term = this.value.toLowerCase();
    let visible = 0;
    activityRows.forEach((row) => {
      const match = row.textContent.toLowerCase().includes(term);
      row.classList.toggle("hidden", !match);
      if (match) visible++;
    });
    noActivity?.classList.toggle("hidden", visible !== 0);
  });

  /**
   * ======================================
   * 08. Charts
   * ======================================
   */
  if (typeof Chart === "undefined") {
    refreshIcons();
    return;
  }

  const themeColors = () => {
    const isDark = html.classList.contains("dark");
    return {
      grid: isDark ? "rgba(255,255,255,0.05)" : "rgba(0,0,0,0.05)",
      text: isDark ? "#94A3B8" : "#64748B",
    };
  };

  let marketChart, technicalChart, rsiMacdChart, signalStatsChart, performanceChart, pairPerformanceChart, pairChart;

  // Overview chart (always present on load)
  function initOverviewChart() {
    const ctx = document.getElementById("faMarketOverviewChart");
    if (!ctx || marketChart) return;
    const { grid, text } = themeColors();
    marketChart = new Chart(ctx, {
      type: "line",
      data: {
        labels: ["00:00", "04:00", "08:00", "12:00", "16:00", "20:00", "24:00"],
        datasets: [
          { label: "EUR/USD", data: [1.0825, 1.0842, 1.0858, 1.0832, 1.0865, 1.0848, 1.0842], borderColor: "#6366F1", backgroundColor: "rgba(99,102,241,0.1)", borderWidth: 2, fill: true, tension: 0.4, pointRadius: 0, pointHoverRadius: 6 },
          { label: "GBP/USD", data: [1.262, 1.2635, 1.2668, 1.2642, 1.2685, 1.2662, 1.2654], borderColor: "#10B981", backgroundColor: "rgba(16,185,129,0.1)", borderWidth: 2, fill: true, tension: 0.4, pointRadius: 0, pointHoverRadius: 6 },
          { label: "USD/JPY", data: [149.2, 149.35, 149.58, 149.42, 149.78, 149.65, 149.82], borderColor: "#F59E0B", backgroundColor: "rgba(245,158,11,0.1)", borderWidth: 2, fill: true, tension: 0.4, pointRadius: 0, pointHoverRadius: 6, yAxisID: "y1" },
        ],
      },
      options: {
        responsive: true,
        maintainAspectRatio: false,
        interaction: { mode: "index", intersect: false },
        plugins: { legend: { position: "top", align: "end", labels: { color: text, usePointStyle: true, padding: 20 } } },
        scales: {
          y: { position: "left", grid: { color: grid }, ticks: { color: text } },
          y1: { position: "right", grid: { display: false }, ticks: { color: text } },
          x: { grid: { display: false }, ticks: { color: text } },
        },
      },
    });
  }

  function initTabCharts(tabId) {
    const { grid, text } = themeColors();

    if (tabId === "analysis") {
      const techCtx = document.getElementById("faTechnicalChart");
      if (techCtx && !technicalChart) {
        technicalChart = new Chart(techCtx, {
          type: "line",
          data: {
            labels: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
            datasets: [
              { label: "Price", data: [1.0825, 1.0858, 1.0842, 1.0875, 1.0832, 1.0865, 1.0848], borderColor: "#6366F1", borderWidth: 2, fill: false, tension: 0.4 },
              { label: "SMA 20", data: [1.082, 1.0835, 1.0845, 1.0852, 1.0848, 1.0855, 1.086], borderColor: "#10B981", borderWidth: 2, borderDash: [5, 5], fill: false, tension: 0.4 },
              { label: "SMA 50", data: [1.0815, 1.0822, 1.083, 1.0838, 1.0845, 1.085, 1.0855], borderColor: "#F59E0B", borderWidth: 2, borderDash: [5, 5], fill: false, tension: 0.4 },
            ],
          },
          options: {
            responsive: true,
            maintainAspectRatio: false,
            plugins: { legend: { position: "top", labels: { color: text, usePointStyle: true } } },
            scales: { y: { grid: { color: grid }, ticks: { color: text } }, x: { grid: { display: false }, ticks: { color: text } } },
          },
        });
      }
      const rsiCtx = document.getElementById("faRsiMacdChart");
      if (rsiCtx && !rsiMacdChart) {
        rsiMacdChart = new Chart(rsiCtx, {
          type: "bar",
          data: {
            labels: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
            datasets: [
              {
                label: "MACD Histogram",
                data: [0.0012, 0.0018, -0.0005, 0.0022, -0.0015, 0.0008, -0.0012],
                backgroundColor: (c) => (c.raw >= 0 ? "rgba(16,185,129,0.6)" : "rgba(239,68,68,0.6)"),
                borderRadius: 4,
              },
            ],
          },
          options: {
            responsive: true,
            maintainAspectRatio: false,
            plugins: { legend: { display: false } },
            scales: { y: { grid: { color: grid }, ticks: { color: text } }, x: { grid: { display: false }, ticks: { color: text } } },
          },
        });
      }
    }

    if (tabId === "signals") {
      const ctx = document.getElementById("faSignalStatsChart");
      if (ctx && !signalStatsChart) {
        signalStatsChart = new Chart(ctx, {
          type: "doughnut",
          data: { labels: ["Profitable", "Stop Loss", "Active"], datasets: [{ data: [19, 5, 3], backgroundColor: ["#10B981", "#EF4444", "#6366F1"], borderWidth: 0 }] },
          options: {
            responsive: true,
            maintainAspectRatio: false,
            plugins: { legend: { position: "bottom", labels: { color: text, usePointStyle: true, padding: 15 } } },
            cutout: "70%",
          },
        });
      }
    }

    if (tabId === "performance") {
      const perfCtx = document.getElementById("faPerformanceChart");
      if (perfCtx && !performanceChart) {
        performanceChart = new Chart(perfCtx, {
          type: "bar",
          data: { labels: ["Jan", "Feb", "Mar", "Apr", "May", "Jun"], datasets: [{ label: "Profit ($)", data: [8500, 12200, 9800, 15400, 11200, 12450], backgroundColor: "rgba(16,185,129,0.8)", borderRadius: 8 }] },
          options: {
            responsive: true,
            maintainAspectRatio: false,
            plugins: { legend: { display: false } },
            scales: {
              y: { grid: { color: grid }, ticks: { color: text, callback: (v) => "$" + v.toLocaleString() } },
              x: { grid: { display: false }, ticks: { color: text } },
            },
          },
        });
      }
      const pairPerfCtx = document.getElementById("faPairPerformanceChart");
      if (pairPerfCtx && !pairPerformanceChart) {
        pairPerformanceChart = new Chart(pairPerfCtx, {
          type: "bar",
          data: { labels: ["EUR/USD", "GBP/USD", "USD/JPY", "AUD/USD", "USD/CAD"], datasets: [{ label: "Win Rate (%)", data: [78, 85, 72, 81, 68], backgroundColor: ["#6366F1", "#10B981", "#F59E0B", "#0EA5E9", "#EC4899"], borderRadius: 8 }] },
          options: {
            responsive: true,
            maintainAspectRatio: false,
            indexAxis: "y",
            plugins: { legend: { display: false } },
            scales: {
              x: { grid: { color: grid }, ticks: { color: text, callback: (v) => v + "%" }, max: 100 },
              y: { grid: { display: false }, ticks: { color: text } },
            },
          },
        });
      }
    }
  }

  // Per-open pair price chart in the drawer
  function renderPairChart(pair, isPositive) {
    const ctx = document.getElementById("faPairChart");
    if (!ctx) return;
    if (pairChart) pairChart.destroy();
    const { grid, text } = themeColors();
    const base = parseFloat(PAIR_DATA[pair]?.price) || 1;
    const seed = [...pair].reduce((a, c) => a + c.charCodeAt(0), 0);
    const data = Array.from({ length: 12 }, (_, i) => {
      const wobble = Math.sin((seed + i) * 0.9) * base * 0.004;
      const drift = (isPositive ? 1 : -1) * base * 0.0008 * i;
      return +(base + wobble + drift).toFixed(4);
    });
    const color = isPositive ? "#10B981" : "#EF4444";
    requestAnimationFrame(() => {
      pairChart = new Chart(ctx, {
        type: "line",
        data: {
          labels: data.map((_, i) => i + 1),
          datasets: [{ data, borderColor: color, backgroundColor: isPositive ? "rgba(16,185,129,0.12)" : "rgba(239,68,68,0.12)", borderWidth: 2, fill: true, tension: 0.4, pointRadius: 0 }],
        },
        options: {
          responsive: true,
          maintainAspectRatio: false,
          plugins: { legend: { display: false } },
          scales: { y: { grid: { color: grid }, ticks: { color: text } }, x: { grid: { display: false }, ticks: { color: text } } },
        },
      });
    });
  }

  // Theme re-color
  function updateChartTheme() {
    const { grid, text } = themeColors();
    [marketChart, technicalChart, rsiMacdChart, signalStatsChart, performanceChart, pairPerformanceChart, pairChart].forEach((chart) => {
      if (!chart) return;
      if (chart.options.scales?.y) {
        chart.options.scales.y.grid.color = grid;
        chart.options.scales.y.ticks.color = text;
      }
      if (chart.options.scales?.y1) chart.options.scales.y1.ticks.color = text;
      if (chart.options.scales?.x) chart.options.scales.x.ticks.color = text;
      if (chart.options.plugins?.legend?.labels) chart.options.plugins.legend.labels.color = text;
      chart.update();
    });
  }
  document.getElementById("themeToggle")?.addEventListener("click", () => setTimeout(updateChartTheme, 50));

  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") closeDrawers();
  });

  // Boot
  initOverviewChart();
  refreshIcons();
});
