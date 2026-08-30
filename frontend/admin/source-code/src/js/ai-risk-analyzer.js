/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: ai-risk-analyzer.js
 * description: Self-contained controller for the AI Risk Analyzer page
 *              (#ai-risk-analyzer). DOM-only — no HTML is generated in JS;
 *              the drawer's three bodies are static templates the JS shows/hides
 *              and patches via textContent.
 * author: SignalAIX

    -------------------------------------------------
    Table of Contents
    -------------------------------------------------
    01. Init & guard
    02. Toast
    03. Tabs
    04. Header actions (run / export / refresh)
    05. Risk gauge needle
    06. Drawer (settings / action / position) + alert dismiss
    07. Position table search + filters
    08. Stress-test scenario buttons
    09. Charts (riskDist / correlation / stress) + theme re-color
    ================================================== */

(function () {
  /* ------------------------------------------------------------------ */
  /* 01. Init & guard                                                   */
  /* ------------------------------------------------------------------ */
  if (!document.getElementById("ai-risk-analyzer")) return;

  const html = document.documentElement;
  const isDark = () => html.classList.contains("dark");
  const tickColor = () => (isDark() ? "#94A3B8" : "#64748B");
  const gridColor = () => (isDark() ? "rgba(255,255,255,0.06)" : "rgba(0,0,0,0.06)");
  const refreshIcons = () => window.lucide && lucide.createIcons();

  /* ------------------------------------------------------------------ */
  /* 02. Toast                                                          */
  /* ------------------------------------------------------------------ */
  const toast = document.getElementById("araToast");
  const toastTitle = document.getElementById("araToastTitle");
  const toastMsg = document.getElementById("araToastMessage");
  let toastTimer;
  function showToast(title, message) {
    if (!toast) return;
    if (title) toastTitle.textContent = title;
    if (message) toastMsg.textContent = message;
    toast.classList.add("active");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => toast.classList.remove("active"), 3000);
  }
  document.getElementById("araToastClose")?.addEventListener("click", () => toast.classList.remove("active"));
  document.querySelectorAll(".ara-toast-btn").forEach((btn) => {
    btn.addEventListener("click", () => {
      showToast(btn.dataset.toastTitle, btn.dataset.toastMsg);
      if (btn.hasAttribute("data-close")) closeDrawer();
    });
  });

  /* ------------------------------------------------------------------ */
  /* 03. Tabs                                                           */
  /* ------------------------------------------------------------------ */
  document.querySelectorAll(".ara-tab").forEach((tab) => {
    tab.addEventListener("click", () => {
      document.querySelectorAll(".ara-tab").forEach((t) => t.classList.remove("active"));
      document.querySelectorAll(".ara-pane").forEach((p) => p.classList.remove("active"));
      tab.classList.add("active");
      document.getElementById(`ara-tab-${tab.dataset.tab}`)?.classList.add("active");
      refreshIcons();
    });
  });

  /* ------------------------------------------------------------------ */
  /* 04. Header actions                                                 */
  /* ------------------------------------------------------------------ */
  document.getElementById("araRun")?.addEventListener("click", () => {
    showToast("Analysis Started", "AI risk analysis running…");
    setTimeout(() => {
      showToast("Complete", "Risk assessment updated");
      setNeedle(62);
    }, 1500);
  });
  document.querySelectorAll("#araExport, .ara-export").forEach((b) => b.addEventListener("click", () => showToast("Export Started", "Preparing report…")));
  document.getElementById("araRefreshInsights")?.addEventListener("click", () => showToast("Refreshing", "Updating AI recommendations…"));
  document.getElementById("araRunStress")?.addEventListener("click", () => showToast("Stress Test", "Running simulation…"));

  /* ------------------------------------------------------------------ */
  /* 05. Risk gauge needle                                              */
  /* ------------------------------------------------------------------ */
  const needle = document.getElementById("araRiskNeedle");
  function setNeedle(score) {
    // semicircle: 0 → -90deg, 100 → +90deg
    needle?.style.setProperty("--rotation", `${-90 + score * 1.8}deg`);
  }
  setTimeout(() => setNeedle(62), 400);

  /* ------------------------------------------------------------------ */
  /* 06. Drawer                                                         */
  /* ------------------------------------------------------------------ */
  const drawer = document.getElementById("araDrawer");
  const overlay = document.getElementById("araDrawerOverlay");
  const drawerTitle = document.getElementById("araDrawerTitle");
  const panels = drawer ? drawer.querySelectorAll(".ara-panel") : [];
  const TITLES = { settings: "Risk Analyzer Settings", action: "Take Action", position: "Position Details" };

  function showPanel(name) {
    panels.forEach((p) => (p.hidden = p.dataset.panel !== name));
    if (drawerTitle) drawerTitle.textContent = TITLES[name] || "Details";
  }
  function openDrawer(name, pair) {
    showPanel(name);
    if (name === "position" && pair) {
      const el = document.getElementById("araPosPair");
      if (el) el.textContent = pair;
    }
    overlay?.classList.add("active");
    drawer?.classList.add("active");
    refreshIcons();
  }
  function closeDrawer() {
    overlay?.classList.remove("active");
    drawer?.classList.remove("active");
  }

  document.querySelectorAll(".ara-open-drawer").forEach((btn) => {
    btn.addEventListener("click", () => openDrawer(btn.dataset.drawer, btn.dataset.pair));
  });
  drawer?.querySelectorAll(".ara-drawer-close").forEach((b) => b.addEventListener("click", closeDrawer));
  overlay?.addEventListener("click", closeDrawer);
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") closeDrawer();
  });

  // Dismiss an alert card (removes its outer card)
  document.querySelectorAll(".ara-dismiss").forEach((btn) => {
    btn.addEventListener("click", () => {
      btn.closest(".bg-panel")?.remove();
      showToast("Alert Dismissed", "The alert has been cleared");
    });
  });

  /* ------------------------------------------------------------------ */
  /* 07. Position table search + filters                                */
  /* ------------------------------------------------------------------ */
  const search = document.getElementById("araPositionSearch");
  const assetFilter = document.getElementById("araAssetFilter");
  const riskFilter = document.getElementById("araRiskFilter");
  const positionsEmpty = document.querySelector(".ara-positions-empty");

  function filterPositions() {
    const s = (search?.value || "").toLowerCase();
    const a = assetFilter?.value || "all";
    const r = riskFilter?.value || "all";
    let visible = 0;
    document.querySelectorAll(".ara-position-row").forEach((row) => {
      const show = row.textContent.toLowerCase().includes(s) && (a === "all" || row.dataset.asset === a) && (r === "all" || row.dataset.risk === r);
      row.style.display = show ? "" : "none";
      if (show) visible++;
    });
    positionsEmpty?.classList.toggle("hidden", visible !== 0);
  }
  search?.addEventListener("input", filterPositions);
  assetFilter?.addEventListener("change", filterPositions);
  riskFilter?.addEventListener("change", filterPositions);
  document.getElementById("araResetFilters")?.addEventListener("click", () => {
    if (search) search.value = "";
    if (assetFilter) assetFilter.value = "all";
    if (riskFilter) riskFilter.value = "all";
    filterPositions();
  });

  /* ------------------------------------------------------------------ */
  /* 08. Stress-test scenario buttons                                   */
  /* ------------------------------------------------------------------ */
  const scenarios = document.querySelectorAll(".ara-scenario");
  function setScenarioActive(btn, on) {
    btn.classList.toggle("active", on);
    btn.classList.toggle("border-accent", on);
    btn.classList.toggle("bg-accent/10", on);
    btn.classList.toggle("border-border", !on);
  }
  scenarios.forEach((btn) => {
    setScenarioActive(btn, btn.classList.contains("active"));
    btn.addEventListener("click", () => scenarios.forEach((b) => setScenarioActive(b, b === btn)));
  });

  /* ------------------------------------------------------------------ */
  /* 09. Charts                                                         */
  /* ------------------------------------------------------------------ */
  const charts = {};

  function initCharts() {
    if (typeof Chart === "undefined") return;

    const dist = document.getElementById("araRiskDistChart");
    if (dist && !charts.dist) {
      charts.dist = new Chart(dist.getContext("2d"), {
        type: "line",
        data: {
          labels: ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"],
          datasets: [
            { label: "Portfolio Risk", data: [45, 52, 48, 61, 55, 58, 65, 70, 62, 58, 65, 62], borderColor: "#10b981", backgroundColor: "rgba(16,185,129,0.1)", fill: true, tension: 0.4, borderWidth: 2, pointRadius: 0 },
            { label: "Market Risk", data: [40, 48, 45, 55, 50, 52, 58, 62, 55, 52, 58, 55], borderColor: "#f59e0b", backgroundColor: "rgba(245,158,11,0.1)", fill: true, tension: 0.4, borderWidth: 2, pointRadius: 0 },
            { label: "Threshold", data: [75, 75, 75, 75, 75, 75, 75, 75, 75, 75, 75, 75], borderColor: "#ef4444", borderDash: [5, 5], borderWidth: 2, fill: false, pointRadius: 0 },
          ],
        },
        options: { responsive: true, maintainAspectRatio: false, interaction: { intersect: false, mode: "index" }, plugins: { legend: { position: "top", align: "end", labels: { color: tickColor(), usePointStyle: true, pointStyle: "circle", padding: 16 } } }, scales: { y: { beginAtZero: true, max: 100, grid: { color: gridColor() }, ticks: { color: tickColor(), callback: (v) => v + "%" } }, x: { grid: { display: false }, ticks: { color: tickColor() } } } },
      });
    }

    const corr = document.getElementById("araCorrelationChart");
    if (corr && !charts.corr) {
      charts.corr = new Chart(corr.getContext("2d"), {
        type: "bar",
        data: {
          labels: ["BTC-ETH", "EUR-GBP", "BTC-SOL", "USD-JPY", "XAU-USD"],
          datasets: [{ label: "Correlation", data: [0.92, 0.78, 0.85, -0.45, -0.65], backgroundColor: (ctx) => { const v = ctx.raw; return v > 0.7 ? "rgba(239,68,68,0.8)" : v > 0.4 ? "rgba(245,158,11,0.8)" : v > 0 ? "rgba(99,102,241,0.8)" : "rgba(16,185,129,0.8)"; }, borderRadius: 8 }],
        },
        options: { responsive: true, maintainAspectRatio: false, indexAxis: "y", plugins: { legend: { display: false }, tooltip: { callbacks: { label: (ctx) => "Correlation: " + ctx.raw.toFixed(2) } } }, scales: { x: { min: -1, max: 1, grid: { color: gridColor() }, ticks: { color: tickColor() } }, y: { grid: { display: false }, ticks: { color: tickColor() } } } },
      });
    }

    const stress = document.getElementById("araStressChart");
    if (stress && !charts.stress) {
      charts.stress = new Chart(stress.getContext("2d"), {
        type: "bar",
        data: {
          labels: ["BTC/USD", "ETH/USD", "EUR/USD", "GBP/USD"],
          datasets: [
            { label: "Current", data: [43520, 26775, 54250, 38100], backgroundColor: "rgba(16,185,129,0.85)", borderRadius: 8 },
            { label: "After Crash", data: [27700, 16065, 48825, 32685], backgroundColor: "rgba(239,68,68,0.85)", borderRadius: 8 },
          ],
        },
        options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { position: "top", align: "end", labels: { color: tickColor(), usePointStyle: true, pointStyle: "circle", padding: 16 } } }, scales: { y: { beginAtZero: true, grid: { color: gridColor() }, ticks: { color: tickColor(), callback: (v) => "$" + v / 1000 + "K" } }, x: { grid: { display: false }, ticks: { color: tickColor() } } } },
      });
    }
  }

  function recolorCharts() {
    Object.values(charts).forEach((c) => {
      if (!c) return;
      const s = c.options.scales || {};
      if (s.y) { if (s.y.grid) s.y.grid.color = gridColor(); if (s.y.ticks) s.y.ticks.color = tickColor(); }
      if (s.x) { if (s.x.grid) s.x.grid.color = gridColor(); if (s.x.ticks) s.x.ticks.color = tickColor(); }
      if (c.options.plugins?.legend?.labels) c.options.plugins.legend.labels.color = tickColor();
      c.update();
    });
  }
  document.getElementById("themeToggle")?.addEventListener("click", () => setTimeout(recolorCharts, 0));

  if (document.readyState === "complete") initCharts();
  else window.addEventListener("load", initCharts);
})();
