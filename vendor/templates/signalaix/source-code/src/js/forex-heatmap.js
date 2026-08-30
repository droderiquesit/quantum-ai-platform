/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: forex-heatmap.js
 * description: Self-contained controller for the Forex Heatmap page
 *              (#forex-heatmap). DOM-only — no HTML is generated in JS; the
 *              heatmap/matrix cells and the drawer body are static templates
 *              the JS shows/hides or patches via textContent.
 * author: SignalAIX

    -------------------------------------------------
    Table of Contents
    -------------------------------------------------
    01. Init & guard
    02. Toast
    03. Tabs + timeframe chips
    04. Export menu
    05. Refresh
    06. Pair drawer (opened from heatmap cells)
    07. Charts (strength radar + momentum line) + theme re-color
    ================================================== */

(function () {
  if (!document.getElementById("forex-heatmap")) return;

  const html = document.documentElement;
  const isDark = () => html.classList.contains("dark");
  const tickColor = () => (isDark() ? "#94A3B8" : "#64748B");
  const gridColor = () => (isDark() ? "rgba(255,255,255,0.08)" : "rgba(0,0,0,0.08)");
  const refreshIcons = () => window.lucide && lucide.createIcons();

  /* 02. Toast */
  const toast = document.getElementById("fhToast");
  let toastTimer;
  function showToast(title, message) {
    if (!toast) return;
    if (title) document.getElementById("fhToastTitle").textContent = title;
    if (message) document.getElementById("fhToastMessage").textContent = message;
    toast.classList.add("active");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => toast.classList.remove("active"), 3000);
  }
  document.getElementById("fhToastClose")?.addEventListener("click", () => toast.classList.remove("active"));
  document.querySelectorAll(".fh-toast-btn").forEach((b) => b.addEventListener("click", () => showToast(b.dataset.toastTitle, b.dataset.toastMsg)));

  /* 03. Tabs */
  document.querySelectorAll(".fh-tab").forEach((tab) => {
    tab.addEventListener("click", () => {
      document.querySelectorAll(".fh-tab").forEach((t) => t.classList.remove("active"));
      document.querySelectorAll(".fh-pane").forEach((p) => p.classList.remove("active"));
      tab.classList.add("active");
      document.getElementById(`fh-tab-${tab.dataset.tab}`)?.classList.add("active");
      initCharts();
      refreshIcons();
    });
  });

  // Timeframe chips
  const tfChips = document.querySelectorAll(".fh-tf");
  function setTfActive(btn, on) {
    btn.classList.toggle("active", on);
    btn.classList.toggle("bg-accent/10", on);
    btn.classList.toggle("border-accent", on);
    btn.classList.toggle("text-accent", on);
    btn.classList.toggle("text-muted", !on);
  }
  tfChips.forEach((btn) => {
    setTfActive(btn, btn.classList.contains("active"));
    btn.addEventListener("click", () => {
      tfChips.forEach((b) => setTfActive(b, b === btn));
      showToast("Timeframe Changed", `Switched to ${btn.dataset.tf}`);
    });
  });

  /* 04. Export */
  document.querySelectorAll(".fh-export").forEach((item) => {
    item.addEventListener("click", () => {
      const fmt = (item.dataset.format || "").toUpperCase();
      item.closest(".dropdown-menu")?.classList.remove("active");
      showToast("Export Started", `Exporting heatmap as ${fmt}…`);
    });
  });

  /* 05. Refresh */
  document.getElementById("fhRefresh")?.addEventListener("click", () => {
    const icon = document.getElementById("fhRefreshIcon");
    icon?.classList.add("animate-spin");
    setTimeout(() => icon?.classList.remove("animate-spin"), 900);
    const stamp = document.getElementById("fhLastUpdate");
    if (stamp) stamp.textContent = "Just now";
    showToast("Data Refreshed", "All data has been updated");
  });

  /* 06. Pair drawer */
  const drawer = document.getElementById("fhDrawer");
  const overlay = document.getElementById("fhDrawerOverlay");

  // per-pair signal/confidence (deterministic, no Math.random)
  const PAIRS = {
    "EUR/USD": { signal: "BUY", cls: "text-emerald-500", conf: "87%" },
    "GBP/USD": { signal: "BUY", cls: "text-emerald-500", conf: "74%" },
    "USD/JPY": { signal: "SELL", cls: "text-red-500", conf: "82%" },
    "USD/CHF": { signal: "SELL", cls: "text-red-500", conf: "69%" },
    "AUD/USD": { signal: "BUY", cls: "text-emerald-500", conf: "92%" },
    "NZD/USD": { signal: "HOLD", cls: "text-amber-500", conf: "58%" },
    "USD/CAD": { signal: "SELL", cls: "text-red-500", conf: "71%" },
    "EUR/GBP": { signal: "HOLD", cls: "text-amber-500", conf: "55%" },
    "EUR/JPY": { signal: "BUY", cls: "text-emerald-500", conf: "90%" },
    "GBP/JPY": { signal: "BUY", cls: "text-emerald-500", conf: "85%" },
    "AUD/JPY": { signal: "BUY", cls: "text-emerald-500", conf: "63%" },
    "CHF/JPY": { signal: "SELL", cls: "text-red-500", conf: "66%" },
  };

  function openPairDrawer(pair, change) {
    const meta = PAIRS[pair] || PAIRS["EUR/USD"];
    const set = (id, val) => { const el = document.getElementById(id); if (el) el.textContent = val; };
    set("fhDrawerPair", pair);
    set("fhDrawerConfidence", meta.conf);
    const sig = document.getElementById("fhDrawerSignal");
    if (sig) { sig.textContent = meta.signal; sig.className = "font-semibold " + meta.cls; }
    const chg = document.getElementById("fhDrawerChange");
    if (chg) chg.textContent = change || "";
    overlay?.classList.add("active");
    drawer?.classList.add("active");
    refreshIcons();
  }
  function closeDrawer() {
    overlay?.classList.remove("active");
    drawer?.classList.remove("active");
  }
  document.querySelectorAll(".fh-pair, .fh-cell").forEach((el) => {
    el.addEventListener("click", () => openPairDrawer(el.dataset.pair, el.dataset.change));
  });
  drawer?.querySelectorAll(".fh-drawer-close").forEach((b) => b.addEventListener("click", closeDrawer));
  overlay?.addEventListener("click", closeDrawer);
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") closeDrawer();
  });

  /* 07. Charts */
  const charts = {};
  function initCharts() {
    if (typeof Chart === "undefined") return;
    const tick = tickColor(), grid = gridColor();

    const strength = document.getElementById("fhStrengthChart");
    if (strength && !charts.strength) {
      charts.strength = new Chart(strength.getContext("2d"), {
        type: "radar",
        data: { labels: ["USD", "EUR", "GBP", "AUD", "CAD", "NZD", "CHF", "JPY"], datasets: [{ label: "Current Strength", data: [82, 68, 61, 55, 48, 42, 35, 22], borderColor: "#10b981", backgroundColor: "rgba(16,185,129,0.2)", borderWidth: 2, pointBackgroundColor: "#10b981" }] },
        options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { display: false } }, scales: { r: { beginAtZero: true, max: 100, grid: { color: grid }, angleLines: { color: grid }, pointLabels: { color: tick }, ticks: { display: false } } } },
      });
    }
    const momentum = document.getElementById("fhMomentumChart");
    if (momentum && !charts.momentum) {
      charts.momentum = new Chart(momentum.getContext("2d"), {
        type: "line",
        data: { labels: ["00:00", "04:00", "08:00", "12:00", "16:00", "20:00", "24:00"], datasets: [{ label: "Momentum", data: [12, 19, 15, 25, 22, 30, 28], borderColor: "#6366f1", backgroundColor: "rgba(99,102,241,0.1)", fill: true, tension: 0.4, borderWidth: 2, pointBackgroundColor: "#6366f1", pointRadius: 0 }] },
        options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { display: false } }, scales: { y: { grid: { color: grid }, ticks: { color: tick } }, x: { grid: { display: false }, ticks: { color: tick } } } },
      });
    }
  }

  function recolorCharts() {
    Object.values(charts).forEach((c) => {
      if (!c) return;
      const s = c.options.scales || {};
      if (s.r) { if (s.r.grid) s.r.grid.color = gridColor(); if (s.r.angleLines) s.r.angleLines.color = gridColor(); if (s.r.pointLabels) s.r.pointLabels.color = tickColor(); }
      if (s.y) { if (s.y.grid) s.y.grid.color = gridColor(); if (s.y.ticks) s.y.ticks.color = tickColor(); }
      if (s.x && s.x.ticks) s.x.ticks.color = tickColor();
      c.update();
    });
  }
  document.getElementById("themeToggle")?.addEventListener("click", () => setTimeout(recolorCharts, 0));
})();
