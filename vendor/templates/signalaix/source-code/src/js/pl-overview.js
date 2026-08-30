/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: pl-overview.js
 * description: Self-contained controller for the P/L Overview page
 *              (#pl-overview). DOM-only — no HTML is generated in JS. The
 *              trade-detail drawer is a single static template patched via
 *              textContent/className from a TRADES data map; the calendar,
 *              table rows, add-trade form, export menu and toast are static
 *              markup the JS toggles/populates.
 * author: SignalAIX

    -------------------------------------------------
    Table of Contents
    -------------------------------------------------
    01. Init & guard
    02. Toast
    03. Market tabs + P/L filter chips + search + sort (filter one table)
    04. Drawers (trade-detail static template, add-trade form)
    05. Export menu, refresh, select-all, pagination, month/date
    06. Charts (P/L line w/ range, distribution doughnut, win/loss bar,
        per-open detail line) + theme re-color
    ================================================== */

(function () {
  /* ------------------------------------------------------------------ */
  /* 01. Init & guard                                                   */
  /* ------------------------------------------------------------------ */
  if (!document.getElementById("pl-overview")) return;

  const html = document.documentElement;
  const isDark = () => html.classList.contains("dark");
  const tickColor = () => (isDark() ? "#94A3B8" : "#64748B");
  const gridColor = () => (isDark() ? "rgba(255,255,255,0.05)" : "rgba(0,0,0,0.05)");
  const refreshIcons = () => window.lucide && lucide.createIcons();

  /* ------------------------------------------------------------------ */
  /* 02. Toast                                                          */
  /* ------------------------------------------------------------------ */
  const toast = document.getElementById("plToast");
  const toastTitle = document.getElementById("plToastTitle");
  const toastMsg = document.getElementById("plToastMessage");
  const toastIcon = document.getElementById("plToastIcon");
  let toastTimer;
  function showToast(title, message, type = "success") {
    if (!toast) return;
    if (title) toastTitle.textContent = title;
    if (message) toastMsg.textContent = message;
    if (toastIcon) {
      toastIcon.className =
        "w-10 h-10 rounded-xl flex items-center justify-center shrink-0 " +
        (type === "danger" ? "bg-gradient-to-br from-red-500 to-rose-600" : "bg-gradient-to-br from-accent to-teal-500");
      const i = toastIcon.querySelector("i");
      if (i) i.setAttribute("data-lucide", type === "danger" ? "trash-2" : "check");
      refreshIcons();
    }
    toast.classList.add("active");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => toast.classList.remove("active"), 3000);
  }
  document.querySelector(".pl-toast-close")?.addEventListener("click", () => toast.classList.remove("active"));

  /* ------------------------------------------------------------------ */
  /* 03. Market tabs + P/L chips + search                              */
  /* ------------------------------------------------------------------ */
  const tabs = document.querySelectorAll(".pl-tab");
  const chips = document.querySelectorAll(".pl-chip");
  const rows = Array.from(document.querySelectorAll(".pl-row"));
  const noResults = document.getElementById("plNoResults");
  let activeMarket = "all";
  let activeFilter = "all";
  let searchTerm = "";

  function applyFilters() {
    let visible = 0;
    rows.forEach((row) => {
      let show = true;
      if (activeMarket !== "all") show = row.dataset.market === activeMarket;
      if (show && activeFilter === "profits") show = row.dataset.profit === "profit";
      else if (show && activeFilter === "losses") show = row.dataset.profit === "loss";
      if (show && searchTerm) show = row.textContent.toLowerCase().includes(searchTerm);
      row.style.display = show ? "" : "none";
      if (show) visible++;
    });
    noResults?.classList.toggle("hidden", visible !== 0);
  }

  tabs.forEach((tab) => {
    tab.addEventListener("click", () => {
      activeMarket = tab.dataset.tab;
      tabs.forEach((t) => t.classList.remove("active"));
      tab.classList.add("active");
      applyFilters();
    });
  });

  chips.forEach((chip) => {
    chip.addEventListener("click", () => {
      activeFilter = chip.dataset.filter;
      chips.forEach((c) => {
        c.classList.remove("active", "border-accent", "bg-accent/15", "text-accent");
        c.classList.add("border-border", "bg-panel", "text-muted");
      });
      chip.classList.add("active", "border-accent", "bg-accent/15", "text-accent");
      chip.classList.remove("border-border", "bg-panel", "text-muted");
      applyFilters();
    });
  });

  document.getElementById("plTradeSearch")?.addEventListener("input", (e) => {
    searchTerm = e.target.value.toLowerCase();
    applyFilters();
  });

  // Sort: reorder existing row nodes (DOM-only).
  const tbody = document.getElementById("plTradesTableBody");
  function plVal(row) {
    const cell = row.children[8]?.textContent || "0";
    return parseFloat(cell.replace(/[^0-9.\-]/g, "")) * (cell.includes("-") ? -1 : 1);
  }
  function dateVal(row) {
    return Date.parse((row.children[2]?.textContent || "").replace(" ", "T")) || 0;
  }
  document.getElementById("plSortBy")?.addEventListener("change", (e) => {
    const v = e.target.value;
    const sorted = rows.slice().sort((a, b) => {
      if (v === "pl-high") return plVal(b) - plVal(a);
      if (v === "pl-low") return plVal(a) - plVal(b);
      if (v === "date-asc") return dateVal(a) - dateVal(b);
      return dateVal(b) - dateVal(a);
    });
    sorted.forEach((r) => tbody.appendChild(r));
  });

  /* ------------------------------------------------------------------ */
  /* 04. Drawers                                                        */
  /* ------------------------------------------------------------------ */
  const overlay = document.getElementById("plDrawerOverlay");
  const tradeDrawer = document.getElementById("plTradeDrawer");
  const addDrawer = document.getElementById("plAddDrawer");

  function openDrawer(drawer) {
    overlay?.classList.add("active");
    drawer?.classList.add("active");
    refreshIcons();
  }
  function closeDrawers() {
    overlay?.classList.remove("active");
    tradeDrawer?.classList.remove("active");
    addDrawer?.classList.remove("active");
  }
  document.querySelectorAll(".pl-close-drawer").forEach((b) => b.addEventListener("click", closeDrawers));
  overlay?.addEventListener("click", closeDrawers);
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      closeDrawers();
      document.getElementById("plExportMenu")?.classList.add("hidden");
    }
    if ((e.ctrlKey || e.metaKey) && e.key === "k") {
      e.preventDefault();
      document.getElementById("plTradeSearch")?.focus();
    }
  });

  // ---- Trade-detail data (deterministic, mirrors the table rows) ----
  const TRADES = {
    "TRD-001": { date: "2024-12-28 14:32", pair: "EUR/USD", type: "Long", entry: "1.1042", exit: "1.1098", lot: "1.5", pl: "+$840", plPct: "+5.1%", strategy: "AI Signal", win: true, series: [1.1042, 1.1051, 1.1038, 1.1064, 1.1079, 1.1085, 1.1098] },
    "TRD-002": { date: "2024-12-28 11:15", pair: "BTC/USDT", type: "Short", entry: "95420", exit: "94180", lot: "0.5", pl: "+$620", plPct: "+1.3%", strategy: "Pattern", win: true, series: [95420, 95210, 95380, 94920, 94560, 94350, 94180] },
    "TRD-003": { date: "2024-12-27 16:45", pair: "GBP/JPY", type: "Long", entry: "198.42", exit: "197.85", lot: "2.0", pl: "-$570", plPct: "-0.3%", strategy: "Manual", win: false, series: [198.42, 198.55, 198.31, 198.1, 197.96, 197.9, 197.85] },
    "TRD-004": { date: "2024-12-27 09:20", pair: "ETH/USDT", type: "Long", entry: "3320", exit: "3485", lot: "2.0", pl: "+$330", plPct: "+5.0%", strategy: "AI Signal", win: true, series: [3320, 3358, 3342, 3401, 3440, 3468, 3485] },
    "TRD-005": { date: "2024-12-26 15:10", pair: "USD/JPY", type: "Short", entry: "157.82", exit: "157.25", lot: "3.0", pl: "+$1,140", plPct: "+0.4%", strategy: "Trend", win: true, series: [157.82, 157.71, 157.78, 157.55, 157.42, 157.31, 157.25] },
    "TRD-006": { date: "2024-12-26 10:30", pair: "XAU/USD", type: "Long", entry: "2612.50", exit: "2598.20", lot: "1.0", pl: "-$1,430", plPct: "-0.5%", strategy: "AI Signal", win: false, series: [2612.5, 2615.1, 2608.4, 2604.2, 2601.0, 2599.5, 2598.2] },
    "TRD-007": { date: "2024-12-25 14:22", pair: "SOL/USDT", type: "Long", entry: "185.40", exit: "192.80", lot: "5.0", pl: "+$370", plPct: "+4.0%", strategy: "Pattern", win: true, series: [185.4, 187.1, 186.5, 189.2, 191.0, 192.1, 192.8] },
    "TRD-008": { date: "2024-12-24 11:45", pair: "EUR/GBP", type: "Short", entry: "0.8295", exit: "0.8262", lot: "2.5", pl: "+$412", plPct: "+0.4%", strategy: "Manual", win: true, series: [0.8295, 0.8288, 0.8291, 0.8279, 0.827, 0.8265, 0.8262] },
    "TRD-009": { date: "2024-12-24 08:15", pair: "ADA/USDT", type: "Long", entry: "0.892", exit: "0.875", lot: "1000", pl: "-$170", plPct: "-1.9%", strategy: "AI Signal", win: false, series: [0.892, 0.894, 0.889, 0.884, 0.88, 0.877, 0.875] },
    "TRD-010": { date: "2024-12-23 16:30", pair: "NAS100", type: "Long", entry: "21420", exit: "21580", lot: "1.0", pl: "+$1,600", plPct: "+0.7%", strategy: "Trend", win: true, series: [21420, 21455, 21438, 21502, 21540, 21565, 21580] },
  };
  const STRAT_CLS = {
    "AI Signal": "bg-violet-500/15 text-violet-400",
    Pattern: "bg-sky-500/15 text-sky-500",
    Trend: "bg-teal-500/15 text-teal-500",
    Manual: "bg-amber-500/15 text-amber-500",
  };

  const set = (id, val) => {
    const el = document.getElementById(id);
    if (el && val != null) el.textContent = val;
  };

  let currentDetailId = null;

  function openTradeDetail(id) {
    const t = TRADES[id];
    if (!t) return;
    currentDetailId = id;
    const winCls = t.win ? "text-emerald-500" : "text-red-500";

    const hero = document.getElementById("plDetailHero");
    if (hero) hero.className = "flex items-center gap-4 p-4 rounded-xl mb-6 " + (t.win ? "bg-emerald-500/10" : "bg-red-500/10");
    const heroIcon = document.getElementById("plDetailHeroIcon");
    if (heroIcon) {
      heroIcon.className =
        "w-14 h-14 rounded-2xl flex items-center justify-center shrink-0 " +
        (t.win ? "bg-gradient-to-br from-accent to-teal-500" : "bg-gradient-to-br from-red-500 to-rose-600");
      heroIcon.querySelector("i")?.setAttribute("data-lucide", t.win ? "trending-up" : "trending-down");
    }
    const plEl = document.getElementById("plDetailPl");
    if (plEl) { plEl.textContent = t.pl; plEl.className = "text-3xl font-bold " + winCls; }

    set("plDetailId", id);
    set("plDetailDate", t.date);
    set("plDetailPair", t.pair);

    const typeEl = document.getElementById("plDetailType");
    if (typeEl) {
      typeEl.textContent = t.type;
      typeEl.className =
        "inline-flex px-2.5 py-1 rounded-lg text-xs font-semibold " +
        (t.type === "Long" ? "bg-emerald-500/15 text-emerald-500" : "bg-red-500/15 text-red-500");
    }
    set("plDetailEntry", t.entry);
    set("plDetailExit", t.exit);
    set("plDetailLot", t.lot);
    const pctEl = document.getElementById("plDetailPlPct");
    if (pctEl) { pctEl.textContent = t.plPct; pctEl.className = "font-semibold " + (t.plPct.startsWith("-") ? "text-red-500" : "text-emerald-500"); }
    const stratEl = document.getElementById("plDetailStrategy");
    if (stratEl) { stratEl.textContent = t.strategy; stratEl.className = "inline-flex px-2.5 py-1 rounded-lg text-xs font-semibold " + (STRAT_CLS[t.strategy] || "bg-slate-500/15 text-muted"); }

    openDrawer(tradeDrawer);
    renderDetailChart(t);
  }

  document.querySelectorAll(".pl-view").forEach((btn) => {
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      openTradeDetail(btn.dataset.id);
    });
  });
  document.querySelectorAll(".pl-edit").forEach((btn) => {
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      showToast("Edit Mode", `Editing trade ${btn.dataset.id}`);
    });
  });
  document.querySelectorAll(".pl-delete").forEach((btn) => {
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      showToast("Trade Deleted", `Trade ${btn.dataset.id} has been removed`, "danger");
    });
  });
  document.querySelector(".pl-detail-edit")?.addEventListener("click", () => showToast("Edit Mode", `Editing trade ${currentDetailId || ""}`.trim()));
  document.querySelector(".pl-detail-delete")?.addEventListener("click", () => {
    showToast("Trade Deleted", `Trade ${currentDetailId || ""} has been removed`.trim(), "danger");
    closeDrawers();
  });

  // Add Trade
  document.getElementById("plAddTradeBtn")?.addEventListener("click", () => openDrawer(addDrawer));
  document.getElementById("plAddForm")?.addEventListener("submit", (e) => {
    e.preventDefault();
    closeDrawers();
    showToast("Trade Added", "New trade has been recorded");
  });

  /* ------------------------------------------------------------------ */
  /* 05. Export, refresh, select-all, pagination, month/date           */
  /* ------------------------------------------------------------------ */
  const exportBtn = document.getElementById("plExportBtn");
  const exportMenu = document.getElementById("plExportMenu");
  exportBtn?.addEventListener("click", (e) => {
    e.stopPropagation();
    exportMenu?.classList.toggle("hidden");
  });
  document.querySelectorAll(".pl-export").forEach((opt) => {
    opt.addEventListener("click", () => {
      const fmt = (opt.dataset.format || "").toUpperCase();
      exportMenu?.classList.add("hidden");
      showToast("Export Started", `Exporting data as ${fmt}…`);
    });
  });
  document.addEventListener("click", (e) => {
    if (exportBtn && !exportBtn.contains(e.target) && !exportMenu?.contains(e.target)) {
      exportMenu?.classList.add("hidden");
    }
  });

  const refreshBtn = document.getElementById("plRefreshBtn");
  refreshBtn?.addEventListener("click", () => {
    const icon = refreshBtn.querySelector("i");
    icon?.classList.add("animate-spin");
    setTimeout(() => {
      icon?.classList.remove("animate-spin");
      showToast("Refreshed", "Data has been updated");
    }, 800);
  });

  const selectAll = document.getElementById("plSelectAll");
  selectAll?.addEventListener("change", (e) => {
    document.querySelectorAll(".pl-row-cb").forEach((cb) => {
      if (cb.closest("tr").style.display !== "none") cb.checked = e.target.checked;
    });
  });

  document.getElementById("plMonthSelect")?.addEventListener("change", (e) => {
    showToast("Calendar Updated", `Showing ${e.target.value}`);
  });

  // Pagination buttons
  const pageBtns = document.querySelectorAll(".pl-page");
  pageBtns.forEach((btn) => {
    btn.addEventListener("click", () => {
      pageBtns.forEach((b) => {
        b.classList.remove("active", "border-accent", "bg-accent", "text-white");
        b.classList.add("bg-bg", "border-border", "text-muted");
      });
      btn.classList.add("active", "border-accent", "bg-accent", "text-white");
      btn.classList.remove("bg-bg", "border-border", "text-muted");
    });
  });

  // Chart range pills
  const rangeBtns = document.querySelectorAll(".pl-range");
  rangeBtns.forEach((btn) => {
    btn.addEventListener("click", () => {
      rangeBtns.forEach((b) => {
        b.classList.remove("active", "bg-accent", "text-white");
        b.classList.add("text-muted");
      });
      btn.classList.add("active", "bg-accent", "text-white");
      btn.classList.remove("text-muted");
      updatePlChart(btn.dataset.range);
    });
  });

  /* ------------------------------------------------------------------ */
  /* 06. Charts + theme re-color                                        */
  /* ------------------------------------------------------------------ */
  const charts = {};
  let detailChart = null;

  const PL_RANGES = {
    "1W": { labels: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"], data: [42000, 43200, 42800, 44500, 45200, 46800, 48329] },
    "1M": { labels: ["W1", "W2", "W3", "W4"], data: [38000, 41000, 44500, 48329] },
    "3M": { labels: ["Oct", "Nov", "Dec"], data: [32000, 40000, 48329] },
    "1Y": { labels: ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"], data: [10000, 12000, 15000, 18000, 22000, 25000, 28000, 32000, 36000, 40000, 44000, 48329] },
    ALL: { labels: ["2022", "2023", "2024"], data: [8000, 28000, 48329] },
  };

  function initCharts() {
    if (typeof Chart === "undefined") return;

    const plCanvas = document.getElementById("plChart");
    if (plCanvas && !charts.pl) {
      const ctx = plCanvas.getContext("2d");
      const grad = ctx.createLinearGradient(0, 0, 0, 290);
      grad.addColorStop(0, "rgba(16,185,129,0.3)");
      grad.addColorStop(1, "rgba(16,185,129,0)");
      const d = PL_RANGES["1W"];
      charts.pl = new Chart(ctx, {
        type: "line",
        data: {
          labels: d.labels,
          datasets: [{
            label: "Cumulative P/L",
            data: d.data,
            borderColor: "#10b981",
            backgroundColor: grad,
            borderWidth: 3,
            fill: true,
            tension: 0.4,
            pointRadius: 4,
            pointBackgroundColor: "#10b981",
            pointBorderColor: isDark() ? "#0C0F16" : "#FFFFFF",
            pointBorderWidth: 2,
            pointHoverRadius: 6,
          }],
        },
        options: {
          responsive: true,
          maintainAspectRatio: false,
          plugins: {
            legend: { display: false },
            tooltip: { callbacks: { label: (c) => `P/L: $${c.raw.toLocaleString()}` } },
          },
          scales: {
            x: { grid: { color: gridColor() }, ticks: { color: tickColor() } },
            y: { grid: { color: gridColor() }, ticks: { color: tickColor(), callback: (v) => "$" + v / 1000 + "k" } },
          },
        },
      });
    }

    const distCanvas = document.getElementById("plDistributionChart");
    if (distCanvas && !charts.dist) {
      charts.dist = new Chart(distCanvas.getContext("2d"), {
        type: "doughnut",
        data: {
          labels: ["Forex", "Crypto", "Indices", "Commodities"],
          datasets: [{ data: [62, 28, 7, 3], backgroundColor: ["#10b981", "#14b8a6", "#8b5cf6", "#f59e0b"], borderWidth: 0, hoverOffset: 8 }],
        },
        options: {
          responsive: true,
          maintainAspectRatio: false,
          cutout: "70%",
          plugins: { legend: { display: false }, tooltip: { callbacks: { label: (c) => `${c.label}: ${c.parsed}%` } } },
        },
      });
    }

    const wlCanvas = document.getElementById("plWinLossChart");
    if (wlCanvas && !charts.wl) {
      charts.wl = new Chart(wlCanvas.getContext("2d"), {
        type: "bar",
        data: {
          labels: ["Jul", "Aug", "Sep", "Oct", "Nov", "Dec"],
          datasets: [
            { label: "Wins", data: [42, 38, 48, 52, 45, 49], backgroundColor: "#10b981", borderRadius: 6, barThickness: 16 },
            { label: "Losses", data: [18, 22, 17, 15, 21, 17], backgroundColor: "#ef4444", borderRadius: 6, barThickness: 16 },
          ],
        },
        options: {
          responsive: true,
          maintainAspectRatio: false,
          plugins: {
            legend: { position: "top", align: "end", labels: { color: tickColor(), usePointStyle: true, pointStyle: "circle", padding: 20 } },
            tooltip: {},
          },
          scales: {
            x: { grid: { display: false }, ticks: { color: tickColor() } },
            y: { grid: { color: gridColor() }, ticks: { color: tickColor() } },
          },
        },
      });
    }
  }

  function updatePlChart(range) {
    if (!charts.pl) return;
    const d = PL_RANGES[range] || PL_RANGES["1W"];
    charts.pl.data.labels = d.labels;
    charts.pl.data.datasets[0].data = d.data;
    charts.pl.update();
  }

  function renderDetailChart(t) {
    if (typeof Chart === "undefined") return;
    const canvas = document.getElementById("plDetailChart");
    if (!canvas) return;
    if (detailChart) { detailChart.destroy(); detailChart = null; }
    const color = t.win ? "#10b981" : "#ef4444";
    const fill = t.win ? "rgba(16,185,129,0.12)" : "rgba(239,68,68,0.12)";
    requestAnimationFrame(() => {
      detailChart = new Chart(canvas.getContext("2d"), {
        type: "line",
        data: {
          labels: t.series.map((_, i) => i + 1),
          datasets: [{ data: t.series, borderColor: color, backgroundColor: fill, borderWidth: 2, fill: true, tension: 0.4, pointRadius: 0 }],
        },
        options: {
          responsive: true,
          maintainAspectRatio: false,
          plugins: { legend: { display: false }, tooltip: { enabled: false } },
          scales: { x: { display: false }, y: { grid: { color: gridColor() }, ticks: { color: tickColor(), maxTicksLimit: 4 } } },
        },
      });
    });
  }

  function recolorCharts() {
    Object.values(charts).forEach((c) => {
      if (!c) return;
      const s = c.options.scales || {};
      if (s.y) { if (s.y.grid) s.y.grid.color = gridColor(); if (s.y.ticks) s.y.ticks.color = tickColor(); }
      if (s.x) { if (s.x.grid && s.x.grid.color) s.x.grid.color = gridColor(); if (s.x.ticks) s.x.ticks.color = tickColor(); }
      if (c.options.plugins?.legend?.labels) c.options.plugins.legend.labels.color = tickColor();
      c.update();
    });
    if (charts.pl) {
      charts.pl.data.datasets[0].pointBorderColor = isDark() ? "#0C0F16" : "#FFFFFF";
      charts.pl.update();
    }
  }
  document.getElementById("themeToggle")?.addEventListener("click", () => setTimeout(recolorCharts, 0));

  if (typeof Chart !== "undefined") {
    initCharts();
  } else {
    window.addEventListener("load", initCharts);
  }
})();
