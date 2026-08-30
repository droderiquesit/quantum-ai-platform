/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: ai-strategy-builder.js
 * description: Self-contained controller for the AI Strategy Builder page
 *              (#ai-strategy-builder). DOM-only — the drawer bodies are static
 *              templates the JS shows/hides; no HTML is generated in JS.
 * author: SignalAIX

    -------------------------------------------------
    Table of Contents
    -------------------------------------------------
    01. Init & guard
    02. Toast
    03. Tabs
    04. Builder sliders (risk / confidence)
    05. Template search
    06. My-strategies filters + search
    07. Drawer (create / indicators / code / backtest / settings)
    08. Charts (performance + equity) + theme re-color
    ================================================== */

(function () {
  if (!document.getElementById("ai-strategy-builder")) return;

  const html = document.documentElement;
  const isDark = () => html.classList.contains("dark");
  const tickColor = () => (isDark() ? "#94A3B8" : "#64748B");
  const gridColor = () => (isDark() ? "rgba(255,255,255,0.06)" : "rgba(0,0,0,0.06)");
  const refreshIcons = () => window.lucide && lucide.createIcons();

  /* 02. Toast */
  const toast = document.getElementById("asbToast");
  let toastTimer;
  function showToast(title, message) {
    if (!toast) return;
    if (title) document.getElementById("asbToastTitle").textContent = title;
    if (message) document.getElementById("asbToastMessage").textContent = message;
    toast.classList.add("active");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => toast.classList.remove("active"), 3000);
  }
  document.getElementById("asbToastClose")?.addEventListener("click", () => toast.classList.remove("active"));
  document.querySelectorAll(".asb-toast-btn").forEach((b) =>
    b.addEventListener("click", () => {
      showToast(b.dataset.toastTitle, b.dataset.toastMsg);
      if (b.hasAttribute("data-close")) closeDrawer();
    })
  );

  /* 03. Tabs */
  document.querySelectorAll(".asb-tab").forEach((tab) => {
    tab.addEventListener("click", () => {
      document.querySelectorAll(".asb-tab").forEach((t) => t.classList.remove("active"));
      document.querySelectorAll(".asb-pane").forEach((p) => p.classList.remove("active"));
      tab.classList.add("active");
      document.getElementById(`asb-tab-${tab.dataset.tab}`)?.classList.add("active");
      initCharts();
      refreshIcons();
    });
  });

  /* 04. Builder sliders */
  const risk = document.getElementById("asbRisk");
  const riskVal = document.getElementById("asbRiskVal");
  risk?.addEventListener("input", () => { if (riskVal) riskVal.textContent = `${parseFloat(risk.value).toFixed(1)}%`; });
  const conf = document.getElementById("asbConf");
  const confVal = document.getElementById("asbConfVal");
  conf?.addEventListener("input", () => { if (confVal) confVal.textContent = `${conf.value}%`; });

  /* 05. Template search */
  const tplSearch = document.getElementById("asbTemplateSearch");
  const tplEmpty = document.querySelector(".asb-template-empty");
  tplSearch?.addEventListener("input", () => {
    const term = tplSearch.value.toLowerCase();
    let visible = 0;
    document.querySelectorAll(".asb-template").forEach((card) => {
      const show = card.dataset.name.includes(term);
      card.style.display = show ? "" : "none";
      if (show) visible++;
    });
    tplEmpty?.classList.toggle("hidden", visible !== 0);
  });

  /* 06. My-strategies filters + search */
  const sfilters = document.querySelectorAll(".asb-sfilter");
  const strategyRows = document.querySelectorAll(".asb-strategy");
  const strategyEmpty = document.querySelector(".asb-strategy-empty");
  const strategySearch = document.getElementById("asbStrategySearch");
  let activeStatus = "all";

  function setFilterActive(btn, on) {
    btn.classList.toggle("active", on);
    btn.classList.toggle("bg-accent/10", on);
    btn.classList.toggle("border-accent", on);
    btn.classList.toggle("text-accent", on);
    btn.classList.toggle("text-muted", !on);
  }
  function applyStrategyFilter() {
    const term = (strategySearch?.value || "").toLowerCase();
    let visible = 0;
    strategyRows.forEach((row) => {
      const show = (activeStatus === "all" || row.dataset.status === activeStatus) && (!term || row.dataset.name.includes(term));
      row.style.display = show ? "" : "none";
      if (show) visible++;
    });
    strategyEmpty?.classList.toggle("hidden", visible !== 0);
  }
  sfilters.forEach((btn) => {
    setFilterActive(btn, btn.classList.contains("active"));
    btn.addEventListener("click", () => {
      activeStatus = btn.dataset.status;
      sfilters.forEach((b) => setFilterActive(b, b === btn));
      applyStrategyFilter();
    });
  });
  strategySearch?.addEventListener("input", applyStrategyFilter);

  /* 07. Drawer */
  const drawer = document.getElementById("asbDrawer");
  const overlay = document.getElementById("asbDrawerOverlay");
  const drawerTitle = document.getElementById("asbDrawerTitle");
  const panels = drawer ? drawer.querySelectorAll(".asb-panel") : [];
  const TITLES = { create: "New Strategy", indicators: "Add Indicators", code: "Strategy Code", backtest: "Quick Backtest", settings: "Strategy Settings" };

  function showPanel(name) {
    panels.forEach((p) => (p.hidden = p.dataset.panel !== name));
    if (drawerTitle) drawerTitle.textContent = TITLES[name] || "Details";
  }
  function openDrawer(name) {
    showPanel(name);
    overlay?.classList.add("active");
    drawer?.classList.add("active");
    refreshIcons();
  }
  function closeDrawer() {
    overlay?.classList.remove("active");
    drawer?.classList.remove("active");
  }
  document.querySelectorAll(".asb-open-drawer").forEach((btn) => btn.addEventListener("click", () => openDrawer(btn.dataset.drawer)));
  drawer?.querySelectorAll(".asb-drawer-close").forEach((b) => b.addEventListener("click", closeDrawer));
  overlay?.addEventListener("click", closeDrawer);
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") closeDrawer();
  });

  /* 08. Charts */
  const charts = {};
  function initCharts() {
    if (typeof Chart === "undefined") return;
    const tick = tickColor(), grid = gridColor();

    const perf = document.getElementById("asbPerformanceChart");
    if (perf && !charts.perf) {
      charts.perf = new Chart(perf.getContext("2d"), {
        type: "line",
        data: { labels: ["W1", "W2", "W3", "W4", "W5", "W6", "W7", "W8", "W9", "W10", "W11", "W12"], datasets: [{ label: "ROI %", data: [0, 2.3, 4.1, 3.8, 6.2, 8.5, 7.9, 10.2, 12.4, 11.8, 14.6, 18.4], borderColor: "#6366f1", backgroundColor: "rgba(99,102,241,0.1)", fill: true, tension: 0.4, borderWidth: 2, pointRadius: 0 }] },
        options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { display: false } }, scales: { y: { grid: { color: grid }, ticks: { color: tick, callback: (v) => v + "%" } }, x: { grid: { display: false }, ticks: { color: tick } } } },
      });
    }
    const eq = document.getElementById("asbEquityChart");
    if (eq && !charts.eq) {
      charts.eq = new Chart(eq.getContext("2d"), {
        type: "line",
        data: { labels: ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"], datasets: [{ label: "Equity", data: [10000, 10450, 10890, 11250, 11680, 12340, 12780, 13150, 13580, 14020, 14450, 14856], borderColor: "#10b981", backgroundColor: "rgba(16,185,129,0.1)", fill: true, tension: 0.4, borderWidth: 2, pointRadius: 0 }] },
        options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { display: false } }, scales: { y: { grid: { color: grid }, ticks: { color: tick, callback: (v) => "$" + v / 1000 + "k" } }, x: { grid: { display: false }, ticks: { color: tick } } } },
      });
    }
  }

  function recolorCharts() {
    Object.values(charts).forEach((c) => {
      if (!c) return;
      const s = c.options.scales || {};
      if (s.y) { if (s.y.grid) s.y.grid.color = gridColor(); if (s.y.ticks) s.y.ticks.color = tickColor(); }
      if (s.x && s.x.ticks) s.x.ticks.color = tickColor();
      c.update();
    });
  }
  document.getElementById("themeToggle")?.addEventListener("click", () => setTimeout(recolorCharts, 0));

  // performance chart is on the default (builder) tab — init after Chart.js loads
  if (document.readyState === "complete") initCharts();
  else window.addEventListener("load", initCharts);
})();
