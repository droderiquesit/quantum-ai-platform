/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: ai-trade-journal.js
 * description: Self-contained controller for the AI Trade Journal page
 *              (#ai-trade-journal). DOM-only — no HTML is generated in JS; the
 *              drawer's panels (add/edit/view/import/ai-insights) are static
 *              templates the JS shows/hides and patches via textContent.
 * author: SignalAIX

    -------------------------------------------------
    Table of Contents
    -------------------------------------------------
    01. Init & guard
    02. Toast
    03. Tabs (category filters the one trades table; Analytics is its own pane)
    04. Search
    05. Filter + export menus
    06. Drawer (add / edit / view / import / ai-insights) + side pills
    07. Analytics charts (P/L line, win-loss doughnut, time bar) + theme re-color
    ================================================== */

(function () {
  /* ------------------------------------------------------------------ */
  /* 01. Init & guard                                                   */
  /* ------------------------------------------------------------------ */
  if (!document.getElementById("ai-trade-journal")) return;

  const html = document.documentElement;
  const isDark = () => html.classList.contains("dark");
  const tickColor = () => (isDark() ? "#94A3B8" : "#64748B");
  const gridColor = () => (isDark() ? "rgba(255,255,255,0.06)" : "rgba(0,0,0,0.06)");
  const refreshIcons = () => window.lucide && lucide.createIcons();

  /* ------------------------------------------------------------------ */
  /* 02. Toast                                                          */
  /* ------------------------------------------------------------------ */
  const toast = document.getElementById("atjToast");
  const toastTitle = document.getElementById("atjToastTitle");
  const toastMsg = document.getElementById("atjToastMessage");
  let toastTimer;
  function showToast(title, message) {
    if (!toast) return;
    if (title) toastTitle.textContent = title;
    if (message) toastMsg.textContent = message;
    toast.classList.add("active");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => toast.classList.remove("active"), 3000);
  }
  document.getElementById("atjToastClose")?.addEventListener("click", () => toast.classList.remove("active"));
  document.querySelectorAll(".atj-toast-btn").forEach((btn) => {
    btn.addEventListener("click", () => {
      showToast(btn.dataset.toastTitle, btn.dataset.toastMsg);
      if (btn.hasAttribute("data-close")) closeDrawer();
    });
  });

  /* ------------------------------------------------------------------ */
  /* 03. Tabs                                                           */
  /* ------------------------------------------------------------------ */
  const tabs = document.querySelectorAll(".atj-tab");
  const tradesPane = document.getElementById("atj-tab-trades");
  const analyticsPane = document.getElementById("atj-tab-analytics");
  const tradeRows = document.querySelectorAll(".atj-trade");
  const tradesEmpty = document.querySelector(".atj-trades-empty");
  const tradesHeading = document.getElementById("atjTradesHeading");
  const HEADINGS = {
    all: "Showing all trades",
    forex: "Showing forex trades",
    crypto: "Showing crypto trades",
    winners: "Showing winning trades",
    losers: "Showing losing trades",
  };

  function applyTradeTab(tab) {
    // category tabs filter the single table; analytics swaps panes
    const isAnalytics = tab === "analytics";
    tradesPane.classList.toggle("active", !isAnalytics);
    analyticsPane.classList.toggle("active", isAnalytics);
    if (isAnalytics) {
      initCharts();
      return;
    }
    if (tradesHeading) tradesHeading.textContent = HEADINGS[tab] || HEADINGS.all;
    let visible = 0;
    tradeRows.forEach((row) => {
      let show = true;
      if (tab === "forex" || tab === "crypto") show = row.dataset.type === tab;
      else if (tab === "winners") show = row.dataset.result === "win";
      else if (tab === "losers") show = row.dataset.result === "loss";
      // also respect the current search term
      if (show && searchTerm) show = row.textContent.toLowerCase().includes(searchTerm);
      row.style.display = show ? "" : "none";
      if (show) visible++;
    });
    tradesEmpty?.classList.toggle("hidden", visible !== 0);
  }

  let activeTab = "all";
  tabs.forEach((tab) => {
    tab.addEventListener("click", () => {
      activeTab = tab.dataset.tab;
      tabs.forEach((t) => t.classList.remove("active"));
      tab.classList.add("active");
      applyTradeTab(activeTab);
      refreshIcons();
    });
  });

  /* ------------------------------------------------------------------ */
  /* 04. Search                                                         */
  /* ------------------------------------------------------------------ */
  let searchTerm = "";
  document.getElementById("atjSearch")?.addEventListener("input", (e) => {
    searchTerm = e.target.value.toLowerCase();
    applyTradeTab(activeTab === "analytics" ? "all" : activeTab);
  });

  /* ------------------------------------------------------------------ */
  /* 05. Filter + export menus                                          */
  /* ------------------------------------------------------------------ */
  document.querySelectorAll(".atj-filter").forEach((opt) => {
    opt.addEventListener("click", () => {
      opt.closest(".dropdown-menu")?.classList.remove("active");
      showToast("Filter Applied", `Filter "${opt.textContent.trim()}" applied`);
    });
  });
  document.querySelectorAll(".atj-export").forEach((opt) => {
    opt.addEventListener("click", () => {
      const fmt = (opt.dataset.format || "").toUpperCase();
      opt.closest(".dropdown-menu")?.classList.remove("active");
      showToast("Export Started", `Exporting trades as ${fmt}…`);
      setTimeout(() => showToast("Export Complete", `Your ${fmt} file is ready for download`), 1500);
    });
  });

  /* ------------------------------------------------------------------ */
  /* 06. Drawer                                                         */
  /* ------------------------------------------------------------------ */
  const drawer = document.getElementById("atjDrawer");
  const overlay = document.getElementById("atjDrawerOverlay");
  const drawerTitle = document.getElementById("atjDrawerTitle");
  const panels = drawer ? drawer.querySelectorAll(".atj-panel") : [];
  const TITLES = { add: "Add New Trade", edit: "Edit Trade", view: "Trade Details", import: "Import Trades", "ai-insights": "AI Trading Insights" };

  function showPanel(name) {
    // add + edit share the "add" form panel
    const panelName = name === "edit" ? "add" : name;
    panels.forEach((p) => (p.hidden = p.dataset.panel !== panelName));
    if (drawerTitle) drawerTitle.textContent = TITLES[name] || "Details";
    const saveLabel = document.getElementById("atjSaveLabel");
    if (saveLabel) saveLabel.textContent = name === "edit" ? "Update Trade" : "Save Trade";
  }

  // pair badge glyphs for the view panel
  const PAIR_BADGE = { "EUR/USD": "€$", "GBP/USD": "£$", "USD/JPY": "$¥", "AUD/USD": "A$", "BTC/USDT": "₿", "ETH/USDT": "Ξ", "SOL/USDT": "◎", "XRP/USDT": "XRP" };

  function openDrawer(name, pair) {
    showPanel(name);
    if (name === "view" && pair) {
      const p = document.getElementById("atjViewPair");
      const b = document.getElementById("atjViewBadge");
      if (p) p.textContent = pair;
      if (b) b.textContent = PAIR_BADGE[pair] || pair.slice(0, 2);
    }
    overlay?.classList.add("active");
    drawer?.classList.add("active");
    refreshIcons();
  }
  function closeDrawer() {
    overlay?.classList.remove("active");
    drawer?.classList.remove("active");
  }

  document.querySelectorAll(".atj-open-drawer").forEach((btn) => {
    btn.addEventListener("click", () => openDrawer(btn.dataset.drawer, btn.dataset.pair));
  });
  drawer?.querySelectorAll(".atj-drawer-close").forEach((b) => b.addEventListener("click", closeDrawer));
  overlay?.addEventListener("click", closeDrawer);
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") closeDrawer();
  });

  // Long/Short side toggle inside the add/edit form
  drawer?.querySelectorAll(".atj-side").forEach((btn) => {
    btn.addEventListener("click", () => {
      drawer.querySelectorAll(".atj-side").forEach((b) => {
        b.classList.remove("active", "bg-accent/10", "border-accent", "text-accent");
        b.classList.add("text-muted");
      });
      btn.classList.add("active", "bg-accent/10", "border-accent", "text-accent");
      btn.classList.remove("text-muted");
    });
  });
  drawer?.querySelectorAll(".atj-side.active").forEach((b) => {
    b.classList.add("bg-accent/10", "border-accent", "text-accent");
    b.classList.remove("text-muted");
  });

  /* ------------------------------------------------------------------ */
  /* 07. Analytics charts                                               */
  /* ------------------------------------------------------------------ */
  const charts = {};
  function initCharts() {
    if (typeof Chart === "undefined") return;

    const pl = document.getElementById("atjPlChart");
    if (pl && !charts.pl) {
      charts.pl = new Chart(pl.getContext("2d"), {
        type: "line",
        data: { labels: ["Week 1", "Week 2", "Week 3", "Week 4"], datasets: [{ label: "Cumulative P/L", data: [5200, 8450, 15800, 24580], borderColor: "#10b981", backgroundColor: "rgba(16,185,129,0.1)", fill: true, tension: 0.4, borderWidth: 3, pointBackgroundColor: "#10b981", pointRadius: 5, pointHoverRadius: 7 }] },
        options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { display: false }, tooltip: { callbacks: { label: (ctx) => "P/L: $" + ctx.parsed.y.toLocaleString() } } }, scales: { y: { beginAtZero: true, grid: { color: gridColor() }, ticks: { color: tickColor(), callback: (v) => "$" + v.toLocaleString() } }, x: { grid: { display: false }, ticks: { color: tickColor() } } } },
      });
    }

    const wl = document.getElementById("atjWinLossChart");
    if (wl && !charts.wl) {
      charts.wl = new Chart(wl.getContext("2d"), {
        type: "doughnut",
        data: { labels: ["Wins", "Losses"], datasets: [{ data: [68.4, 31.6], backgroundColor: ["#10b981", "#ef4444"], borderWidth: 0, hoverOffset: 10 }] },
        options: { responsive: true, maintainAspectRatio: false, cutout: "70%", plugins: { legend: { position: "bottom", labels: { color: tickColor(), padding: 16, usePointStyle: true } }, tooltip: { callbacks: { label: (ctx) => ctx.label + ": " + ctx.parsed + "%" } } } },
      });
    }

    const time = document.getElementById("atjTimeChart");
    if (time && !charts.time) {
      charts.time = new Chart(time.getContext("2d"), {
        type: "bar",
        data: { labels: ["00-04", "04-08", "08-12", "12-16", "16-20", "20-24"], datasets: [{ label: "Trades", data: [15, 42, 128, 186, 95, 28], backgroundColor: ["rgba(16,185,129,0.4)", "rgba(16,185,129,0.5)", "rgba(16,185,129,0.7)", "rgba(16,185,129,1)", "rgba(16,185,129,0.6)", "rgba(16,185,129,0.3)"], borderRadius: 6 }] },
        options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { display: false } }, scales: { y: { beginAtZero: true, grid: { color: gridColor() }, ticks: { color: tickColor() } }, x: { grid: { display: false }, ticks: { color: tickColor() } } } },
      });
    }
  }

  function recolorCharts() {
    Object.values(charts).forEach((c) => {
      if (!c) return;
      const s = c.options.scales || {};
      if (s.y) { if (s.y.grid) s.y.grid.color = gridColor(); if (s.y.ticks) s.y.ticks.color = tickColor(); }
      if (s.x && s.x.ticks) s.x.ticks.color = tickColor();
      if (c.options.plugins?.legend?.labels) c.options.plugins.legend.labels.color = tickColor();
      c.update();
    });
  }
  document.getElementById("themeToggle")?.addEventListener("click", () => setTimeout(recolorCharts, 0));

  document.getElementById("atjPlPeriod")?.addEventListener("change", (e) => {
    showToast("Period Updated", `Showing P/L for ${e.target.options[e.target.selectedIndex].text}`);
  });
})();
