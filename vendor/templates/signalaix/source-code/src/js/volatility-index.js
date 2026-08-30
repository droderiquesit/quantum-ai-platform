/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: volatility-index.js
 * description: Self-contained controller for the Volatility Index page
 *              (#volatility-index). DOM-only — table rows, cards, heatmap and
 *              drawer bodies are static templates the JS shows/hides/filters;
 *              no HTML is generated in JS.
 * author: SignalAIX

    -------------------------------------------------
    Table of Contents
    -------------------------------------------------
    01. Init & guard
    02. Toast
    03. Tabs (lazy-init tab charts)
    04. Category chips + table search
    05. Drawer (alerts / analysis) + alert-type picker
    06. Charts (volatility / forex / crypto / historical) + theme re-color
    ================================================== */

(function () {
  if (!document.getElementById("volatility-index")) return;

  const htmlEl = document.documentElement;
  const isDark = () => htmlEl.classList.contains("dark");
  const tickColor = () => (isDark() ? "#94A3B8" : "#64748B");
  const gridColor = () => (isDark() ? "rgba(255,255,255,0.06)" : "rgba(0,0,0,0.06)");
  const refreshIcons = () => window.lucide && lucide.createIcons();

  /* 02. Toast */
  const toast = document.getElementById("viToast");
  let toastTimer;
  function showToast(title, message) {
    if (!toast) return;
    if (title) document.getElementById("viToastTitle").textContent = title;
    if (message) document.getElementById("viToastMessage").textContent = message;
    toast.classList.add("active");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => toast.classList.remove("active"), 3000);
  }
  document.getElementById("viToastClose")?.addEventListener("click", () => toast.classList.remove("active"));
  document.querySelectorAll(".vi-toast-btn").forEach((b) =>
    b.addEventListener("click", () => {
      showToast(b.dataset.toastTitle, b.dataset.toastMsg);
      if (b.hasAttribute("data-close")) closeDrawer();
    })
  );

  /* 03. Tabs */
  document.querySelectorAll(".vi-tab").forEach((tab) => {
    tab.addEventListener("click", () => {
      document.querySelectorAll(".vi-tab").forEach((t) => t.classList.remove("active"));
      document.querySelectorAll(".tab-content").forEach((c) => c.classList.remove("active"));
      tab.classList.add("active");
      const id = tab.dataset.tab;
      document.getElementById(id)?.classList.add("active");
      if (id === "forex") initForexChart();
      if (id === "crypto") initCryptoChart();
      if (id === "historical") initHistoricalChart();
      refreshIcons();
    });
  });
  document.querySelector(".vi-view-all")?.addEventListener("click", () => {
    document.querySelector('.vi-tab[data-tab="overview"]')?.click();
  });

  /* 04. Category chips + table search */
  const rows = Array.from(document.querySelectorAll(".vi-row"));
  let activeCat = "all";
  let searchTerm = "";
  function applyFilter() {
    rows.forEach((r) => {
      const ok = (activeCat === "all" || r.dataset.cat === activeCat) && (!searchTerm || r.textContent.toLowerCase().includes(searchTerm));
      r.style.display = ok ? "" : "none";
    });
  }
  document.querySelectorAll(".vi-cat").forEach((chip) => {
    chip.addEventListener("click", () => {
      document.querySelectorAll(".vi-cat").forEach((c) => {
        c.classList.remove("active", "bg-accent/10", "border-accent", "text-accent");
        c.classList.add("text-muted");
      });
      chip.classList.add("active", "bg-accent/10", "border-accent", "text-accent");
      chip.classList.remove("text-muted");
      activeCat = chip.dataset.cat;
      applyFilter();
    });
  });
  document.getElementById("viTableSearch")?.addEventListener("input", (e) => {
    searchTerm = e.target.value.toLowerCase();
    applyFilter();
  });

  /* 05. Drawer */
  const overlay = document.getElementById("viOverlay");
  const drawer = document.getElementById("viDrawer");
  const drawerTitle = document.getElementById("viDrawerTitle");
  const panels = drawer ? drawer.querySelectorAll(".vi-panel") : [];
  const TITLES = { alerts: "Create Volatility Alert", analysis: "AI Volatility Analysis" };

  function openDrawer(name) {
    panels.forEach((p) => (p.hidden = p.dataset.panel !== name));
    if (drawerTitle) drawerTitle.textContent = TITLES[name] || "Details";
    overlay?.classList.add("active");
    drawer?.classList.add("active");
    refreshIcons();
  }
  function closeDrawer() {
    overlay?.classList.remove("active");
    drawer?.classList.remove("active");
  }
  document.querySelectorAll(".vi-open-drawer").forEach((btn) => btn.addEventListener("click", () => openDrawer(btn.dataset.drawer)));
  drawer?.querySelectorAll(".vi-drawer-close").forEach((b) => b.addEventListener("click", closeDrawer));
  overlay?.addEventListener("click", closeDrawer);
  document.addEventListener("keydown", (e) => { if (e.key === "Escape") closeDrawer(); });

  document.querySelectorAll(".vi-alert-type").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll(".vi-alert-type").forEach((b) => b.classList.remove("border-accent", "bg-accent/10"));
      btn.classList.add("border-accent", "bg-accent/10");
    });
  });

  /* 06. Charts */
  const charts = {};
  function initVolatility() {
    if (typeof Chart === "undefined" || charts.vol) return;
    const el = document.getElementById("viVolatilityChart");
    if (!el) return;
    charts.vol = new Chart(el.getContext("2d"), {
      type: "line",
      data: {
        labels: ["00:00", "04:00", "08:00", "12:00", "16:00", "20:00", "24:00"],
        datasets: [
          { label: "VIX", data: [22.5, 23.1, 24.2, 25.8, 24.1, 23.5, 24.68], borderColor: "#6366F1", backgroundColor: "rgba(99,102,241,0.1)", fill: true, tension: 0.4, borderWidth: 2 },
          { label: "Crypto Vol", data: [38.2, 40.5, 42.1, 45.3, 43.8, 41.2, 42.87], borderColor: "#F59E0B", backgroundColor: "rgba(245,158,11,0.1)", fill: true, tension: 0.4, borderWidth: 2 },
          { label: "Forex Vol", data: [15.8, 16.2, 17.5, 19.2, 18.1, 17.8, 18.42], borderColor: "#10B981", backgroundColor: "rgba(16,185,129,0.1)", fill: true, tension: 0.4, borderWidth: 2 },
        ],
      },
      options: { responsive: true, maintainAspectRatio: false, interaction: { intersect: false, mode: "index" }, plugins: { legend: { position: "top", align: "end", labels: { color: tickColor(), usePointStyle: true, pointStyle: "circle", padding: 16, font: { size: 12 } } } }, scales: { y: { grid: { color: gridColor() }, ticks: { color: tickColor() } }, x: { grid: { display: false }, ticks: { color: tickColor() } } } },
    });
  }
  function barChart(id, key, labels, vals, colors) {
    if (typeof Chart === "undefined" || charts[key]) return;
    const el = document.getElementById(id);
    if (!el) return;
    charts[key] = new Chart(el.getContext("2d"), {
      type: "bar",
      data: { labels, datasets: [{ data: vals, backgroundColor: colors, borderRadius: 8, borderSkipped: false }] },
      options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { display: false } }, scales: { y: { beginAtZero: true, grid: { color: gridColor() }, ticks: { color: tickColor() } }, x: { grid: { display: false }, ticks: { color: tickColor() } } } },
    });
  }
  function initForexChart() {
    barChart("viForexChart", "forex", ["EUR/USD", "GBP/USD", "USD/JPY", "AUD/USD", "USD/CAD", "EUR/GBP"], [12.45, 15.67, 8.32, 18.94, 11.23, 9.87], ["rgba(245,158,11,0.8)", "rgba(245,158,11,0.8)", "rgba(16,185,129,0.8)", "rgba(249,115,22,0.8)", "rgba(16,185,129,0.8)", "rgba(16,185,129,0.8)"]);
  }
  function initCryptoChart() {
    barChart("viCryptoChart", "crypto", ["BTC/USD", "ETH/USD", "XRP/USD", "SOL/USD", "ADA/USD", "DOGE/USD"], [42.87, 38.45, 35.62, 48.23, 32.15, 55.89], ["rgba(239,68,68,0.8)", "rgba(239,68,68,0.8)", "rgba(249,115,22,0.8)", "rgba(239,68,68,0.8)", "rgba(249,115,22,0.8)", "rgba(239,68,68,0.8)"]);
  }
  function initHistoricalChart() {
    if (typeof Chart === "undefined" || charts.hist) return;
    const el = document.getElementById("viHistoricalChart");
    if (!el) return;
    const labels = [];
    const vals = [];
    for (let i = 30; i >= 0; i--) {
      labels.push(`D-${i}`);
      vals.push(15 + ((i * 7 + 11) % 20));
    }
    charts.hist = new Chart(el.getContext("2d"), {
      type: "line",
      data: { labels, datasets: [{ label: "Historical VIX", data: vals, borderColor: "#6366F1", backgroundColor: "rgba(99,102,241,0.1)", fill: true, tension: 0.4, borderWidth: 2, pointRadius: 0 }] },
      options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { position: "top", align: "end", labels: { color: tickColor(), usePointStyle: true, pointStyle: "circle" } } }, scales: { y: { grid: { color: gridColor() }, ticks: { color: tickColor() } }, x: { grid: { display: false }, ticks: { color: tickColor(), maxRotation: 45, minRotation: 45 } } } },
    });
  }

  function recolor() {
    Object.values(charts).forEach((c) => {
      if (!c || typeof c.update !== "function") return;
      const s = c.options.scales || {};
      if (s.y) { if (s.y.grid) s.y.grid.color = gridColor(); if (s.y.ticks) s.y.ticks.color = tickColor(); }
      if (s.x && s.x.ticks) s.x.ticks.color = tickColor();
      if (c.options.plugins?.legend?.labels) c.options.plugins.legend.labels.color = tickColor();
      c.update();
    });
  }
  document.getElementById("themeToggle")?.addEventListener("click", () => setTimeout(recolor, 0));

  if (document.readyState === "complete") initVolatility();
  else window.addEventListener("load", initVolatility);
})();
