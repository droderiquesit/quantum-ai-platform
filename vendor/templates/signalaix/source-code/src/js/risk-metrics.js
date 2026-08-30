/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: risk-metrics.js
 * description: Self-contained controller for the Risk Metrics page
 *              (#risk-metrics). DOM-only — no HTML is generated in JS; the
 *              position-details drawer is a static template the JS shows and
 *              patches via textContent. Mirrors the mockup's behaviour:
 *              tabs, asset filter chips, export menu, refresh, risk gauge needle,
 *              position-row / alert-card drawer, alert search + severity filter,
 *              dismiss, exposure search, and real Chart.js charts.
 * author: SignalAIX

    -------------------------------------------------
    Table of Contents
    -------------------------------------------------
    01. Init & guard
    02. Toast
    03. Tabs + cross-tab links
    04. Asset filter chips
    05. Export menu + refresh + add-alert + mark-all-read
    06. Risk gauge needle
    07. Drawer (position details) + open from rows/alerts
    08. Exposure search
    09. Alerts search + severity filter + dismiss
    10. Charts (riskDist / assetExposure / currencyExposure / volatility)
        + lazy init per active tab + theme re-color
    ================================================== */

(function () {
  /* ------------------------------------------------------------------ */
  /* 01. Init & guard                                                   */
  /* ------------------------------------------------------------------ */
  if (!document.getElementById("risk-metrics")) return;

  const html = document.documentElement;
  const isDark = () => html.classList.contains("dark");
  const tickColor = () => (isDark() ? "#94A3B8" : "#64748B");
  const gridColor = () => (isDark() ? "rgba(255,255,255,0.06)" : "rgba(0,0,0,0.06)");
  const refreshIcons = () => window.lucide && lucide.createIcons();

  // Glyph used in the drawer header avatar for each pair.
  const PAIR_GLYPH = {
    "EUR/USD": "€/$",
    "BTC/USD": "₿",
    "GBP/JPY": "£/¥",
    "ETH/USD": "ETH",
    "USD/CAD": "$/C",
    Portfolio: "PF",
  };

  /* ------------------------------------------------------------------ */
  /* 02. Toast                                                          */
  /* ------------------------------------------------------------------ */
  const toast = document.getElementById("rmToast");
  const toastTitle = document.getElementById("rmToastTitle");
  const toastMsg = document.getElementById("rmToastMessage");
  let toastTimer;
  function showToast(title, message) {
    if (!toast) return;
    if (title) toastTitle.textContent = title;
    if (message) toastMsg.textContent = message;
    toast.classList.add("active");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => toast.classList.remove("active"), 3000);
  }
  document.getElementById("rmToastClose")?.addEventListener("click", () => toast.classList.remove("active"));

  /* ------------------------------------------------------------------ */
  /* 03. Tabs + cross-tab links                                         */
  /* ------------------------------------------------------------------ */
  function activateTab(name) {
    document.querySelectorAll(".rm-tab").forEach((t) => t.classList.toggle("active", t.dataset.tab === name));
    document.querySelectorAll(".rm-pane").forEach((p) => p.classList.toggle("active", p.id === `rm-tab-${name}`));
    initChartsForTab(name);
    refreshIcons();
  }
  document.querySelectorAll(".rm-tab").forEach((tab) => {
    tab.addEventListener("click", () => activateTab(tab.dataset.tab));
  });
  // "Manage Alerts" / "View All" jump to another tab.
  document.querySelectorAll(".rm-tab-link").forEach((b) => b.addEventListener("click", () => activateTab(b.dataset.goTab)));
  document.getElementById("rmViewAllPositions")?.addEventListener("click", () => activateTab("exposure"));

  /* ------------------------------------------------------------------ */
  /* 04. Asset filter chips                                             */
  /* ------------------------------------------------------------------ */
  const chips = document.querySelectorAll(".rm-chip");
  chips.forEach((chip) => {
    chip.addEventListener("click", () => {
      chips.forEach((c) => {
        const on = c === chip;
        c.classList.toggle("active", on);
        c.classList.toggle("border-accent", on);
        c.classList.toggle("bg-accent/10", on);
        c.classList.toggle("text-accent", on);
        c.classList.toggle("border-border", !on);
        c.classList.toggle("bg-panel", !on);
        c.classList.toggle("text-muted", !on);
      });
      showToast("Filter Applied", `Showing ${chip.textContent.trim()}`);
    });
  });

  /* ------------------------------------------------------------------ */
  /* 05. Export menu + refresh + add-alert + mark-all-read              */
  /* ------------------------------------------------------------------ */
  const exportBtn = document.getElementById("rmExportBtn");
  const exportMenu = document.getElementById("rmExportMenu");
  exportBtn?.addEventListener("click", (e) => {
    e.stopPropagation();
    exportMenu?.classList.toggle("hidden");
  });
  document.addEventListener("click", () => exportMenu?.classList.add("hidden"));
  document.querySelectorAll(".rm-export-item").forEach((item) => {
    item.addEventListener("click", () => {
      exportMenu?.classList.add("hidden");
      showToast("Export Started", `Preparing ${item.dataset.format.toUpperCase()} report…`);
    });
  });

  const refreshBtn = document.getElementById("rmRefreshBtn");
  const refreshIcon = document.getElementById("rmRefreshIcon");
  refreshBtn?.addEventListener("click", () => {
    refreshIcon?.classList.add("animate-spin");
    showToast("Refreshing", "Updating risk data…");
    setTimeout(() => {
      refreshIcon?.classList.remove("animate-spin");
      showToast("Updated", "Risk metrics refreshed");
    }, 1000);
  });

  document.getElementById("rmAddAlertBtn")?.addEventListener("click", () => showToast("New Alert", "Alert configuration opened"));
  document.getElementById("rmMarkAllRead")?.addEventListener("click", () => showToast("Alerts", "All alerts marked as read"));

  /* ------------------------------------------------------------------ */
  /* 06. Risk gauge needle                                             */
  /* ------------------------------------------------------------------ */
  const needle = document.getElementById("rmRiskNeedle");
  function setNeedle(score) {
    // semicircle: 0 → -90deg, 100 → +90deg
    needle?.style.setProperty("--rotation", `${-90 + score * 1.8}deg`);
  }
  setTimeout(() => setNeedle(67), 400);

  /* ------------------------------------------------------------------ */
  /* 07. Drawer (position details)                                      */
  /* ------------------------------------------------------------------ */
  const drawer = document.getElementById("rmDrawer");
  const overlay = document.getElementById("rmDrawerOverlay");
  const drawerSub = document.getElementById("rmDrawerSubtitle");
  const drawerPair = document.getElementById("rmDrawerPair");
  const drawerGlyph = document.getElementById("rmDrawerGlyph");

  function openDrawer(pair) {
    if (pair) {
      if (drawerSub) drawerSub.textContent = `Risk analysis for ${pair}`;
      if (drawerPair) drawerPair.textContent = pair;
      if (drawerGlyph) drawerGlyph.textContent = PAIR_GLYPH[pair] || pair.slice(0, 3);
    }
    overlay?.classList.add("active");
    drawer?.classList.add("active");
    refreshIcons();
  }
  function closeDrawer() {
    overlay?.classList.remove("active");
    drawer?.classList.remove("active");
  }
  drawer?.querySelectorAll(".rm-drawer-close").forEach((b) => b.addEventListener("click", closeDrawer));
  overlay?.addEventListener("click", closeDrawer);
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") closeDrawer();
  });
  document.querySelectorAll(".rm-toast-btn").forEach((btn) => {
    btn.addEventListener("click", () => {
      showToast(btn.dataset.toastTitle, btn.dataset.toastMsg);
      if (btn.hasAttribute("data-close")) closeDrawer();
    });
  });

  document.querySelectorAll(".rm-position-row, .rm-alert-card").forEach((el) => {
    el.addEventListener("click", () => openDrawer(el.dataset.pair));
  });

  /* ------------------------------------------------------------------ */
  /* 08. Exposure search                                               */
  /* ------------------------------------------------------------------ */
  const exposureSearch = document.getElementById("rmExposureSearch");
  const exposureEmpty = document.getElementById("rmExposureEmpty");
  exposureSearch?.addEventListener("input", (e) => {
    const term = e.target.value.toLowerCase();
    let visible = 0;
    document.querySelectorAll("#rmExposureTable tbody tr").forEach((row) => {
      const show = row.textContent.toLowerCase().includes(term);
      row.style.display = show ? "" : "none";
      if (show) visible++;
    });
    exposureEmpty?.classList.toggle("hidden", visible !== 0);
  });

  /* ------------------------------------------------------------------ */
  /* 09. Alerts search + severity filter + dismiss                     */
  /* ------------------------------------------------------------------ */
  const alertSearch = document.getElementById("rmAlertSearch");
  const alertFilter = document.getElementById("rmAlertFilter");
  const alertsEmpty = document.getElementById("rmAlertsEmpty");

  function filterAlerts() {
    const term = (alertSearch?.value || "").toLowerCase();
    const sev = alertFilter?.value || "all";
    let visible = 0;
    document.querySelectorAll(".rm-alert-row").forEach((row) => {
      const show = row.textContent.toLowerCase().includes(term) && (sev === "all" || row.dataset.severity === sev);
      row.style.display = show ? "" : "none";
      if (show) visible++;
    });
    alertsEmpty?.classList.toggle("hidden", visible !== 0);
  }
  alertSearch?.addEventListener("input", filterAlerts);
  alertFilter?.addEventListener("change", filterAlerts);

  document.querySelectorAll(".rm-alert-dismiss").forEach((btn) => {
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      btn.closest(".rm-alert-row")?.remove();
      filterAlerts();
      showToast("Alert Dismissed", "The alert has been cleared");
    });
  });

  /* ------------------------------------------------------------------ */
  /* 10. Charts                                                        */
  /* ------------------------------------------------------------------ */
  const charts = {};

  function build(name) {
    if (typeof Chart === "undefined") return;

    if (name === "overview" && !charts.dist) {
      const el = document.getElementById("rmRiskDistChart");
      if (el)
        charts.dist = new Chart(el.getContext("2d"), {
          type: "line",
          data: {
            labels: ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug"],
            datasets: [
              { label: "Risk Score", data: [45, 52, 48, 61, 55, 67, 72, 67], borderColor: "#10b981", backgroundColor: "rgba(16,185,129,0.1)", fill: true, tension: 0.4, borderWidth: 3, pointBackgroundColor: "#10b981", pointRadius: 4, pointHoverRadius: 6 },
              { label: "VaR", data: [3200, 3800, 3500, 4100, 3900, 4500, 4800, 4850], borderColor: "#ef4444", backgroundColor: "rgba(239,68,68,0.1)", fill: true, tension: 0.4, borderWidth: 2, pointBackgroundColor: "#ef4444", pointRadius: 3, yAxisID: "y1" },
            ],
          },
          options: {
            responsive: true,
            maintainAspectRatio: false,
            interaction: { intersect: false, mode: "index" },
            plugins: { legend: { position: "top", align: "end", labels: { color: tickColor(), usePointStyle: true, pointStyle: "circle", padding: 16 } } },
            scales: {
              y: { beginAtZero: true, max: 100, grid: { color: gridColor() }, ticks: { color: tickColor() } },
              y1: { position: "right", beginAtZero: true, grid: { display: false }, ticks: { color: tickColor(), callback: (v) => "$" + v.toLocaleString() } },
              x: { grid: { display: false }, ticks: { color: tickColor() } },
            },
          },
        });
    }

    if (name === "exposure") {
      const a = document.getElementById("rmAssetExposureChart");
      if (a && !charts.asset)
        charts.asset = new Chart(a.getContext("2d"), {
          type: "doughnut",
          data: { labels: ["Forex", "Crypto", "Commodities", "Indices"], datasets: [{ data: [45, 35, 12, 8], backgroundColor: ["#10b981", "#6366f1", "#06b6d4", "#f59e0b"], borderWidth: 0, hoverOffset: 10 }] },
          options: { responsive: true, maintainAspectRatio: false, cutout: "70%", plugins: { legend: { position: "bottom", labels: { color: tickColor(), usePointStyle: true, padding: 16 } } } },
        });

      const c = document.getElementById("rmCurrencyExposureChart");
      if (c && !charts.currency)
        charts.currency = new Chart(c.getContext("2d"), {
          type: "bar",
          data: { labels: ["USD", "EUR", "GBP", "JPY", "BTC", "ETH"], datasets: [{ label: "Net Exposure", data: [35, 25, -15, -10, 28, 16], backgroundColor: (ctx) => (ctx.raw >= 0 ? "#10b981" : "#ef4444"), borderRadius: 8 }] },
          options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { display: false } }, scales: { y: { grid: { color: gridColor() }, ticks: { color: tickColor(), callback: (v) => v + "%" } }, x: { grid: { display: false }, ticks: { color: tickColor() } } } },
        });
    }

    if (name === "volatility" && !charts.vol) {
      const el = document.getElementById("rmVolatilityChart");
      if (el)
        charts.vol = new Chart(el.getContext("2d"), {
          type: "line",
          data: {
            labels: Array.from({ length: 30 }, (_, i) => `Day ${i + 1}`),
            datasets: [
              { label: "Portfolio Volatility", data: [15, 16, 14, 18, 20, 19, 22, 21, 23, 25, 24, 22, 20, 18, 19, 21, 22, 24, 26, 25, 23, 22, 24, 26, 28, 27, 25, 24, 25, 24.8], borderColor: "#6366f1", backgroundColor: "rgba(99,102,241,0.1)", fill: true, tension: 0.3, borderWidth: 2, pointRadius: 0 },
              { label: "Market Average", data: Array(30).fill(18.5), borderColor: "#94A3B8", borderDash: [5, 5], borderWidth: 1, pointRadius: 0 },
            ],
          },
          options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { position: "top", align: "end", labels: { color: tickColor(), usePointStyle: true, padding: 16 } } }, scales: { y: { grid: { color: gridColor() }, ticks: { color: tickColor(), callback: (v) => v + "%" } }, x: { grid: { display: false }, ticks: { color: tickColor(), maxTicksLimit: 10 } } } },
        });
    }
  }

  function initChartsForTab(name) {
    requestAnimationFrame(() => {
      build(name);
      // charts created while their pane was hidden need a resize once visible
      Object.values(charts).forEach((c) => c && c.resize());
    });
  }

  function recolorCharts() {
    Object.values(charts).forEach((c) => {
      if (!c) return;
      const s = c.options.scales || {};
      ["x", "y", "y1"].forEach((ax) => {
        if (s[ax]) {
          if (s[ax].grid && s[ax].grid.color) s[ax].grid.color = gridColor();
          if (s[ax].ticks) s[ax].ticks.color = tickColor();
        }
      });
      if (c.options.plugins?.legend?.labels) c.options.plugins.legend.labels.color = tickColor();
      c.update();
    });
  }
  document.getElementById("themeToggle")?.addEventListener("click", () => setTimeout(recolorCharts, 0));

  // Period select (mockup only logged; keep a toast for parity)
  document.getElementById("rmPeriodSelect")?.addEventListener("change", (e) => showToast("Period Changed", `Showing ${e.target.selectedOptions[0].text}`));

  // Init the default (Overview) tab's chart.
  if (document.readyState === "complete") initChartsForTab("overview");
  else window.addEventListener("load", () => initChartsForTab("overview"));
})();
