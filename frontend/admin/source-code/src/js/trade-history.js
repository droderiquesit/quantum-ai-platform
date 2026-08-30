/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: trade-history.js
 * description: Self-contained controller for the Trade History page
 *              (#trade-history). DOM-only — no HTML is generated in JS; the
 *              trade-detail drawer is a single static template patched via
 *              textContent/className from a TRADES data map, and the filter
 *              drawer + toast + export menu are static markup the JS toggles.
 * author: SignalAIX

    -------------------------------------------------
    Table of Contents
    -------------------------------------------------
    01. Init & guard
    02. Toast
    03. Tabs (category filters the one trades table) + search
    04. Filter drawer (apply / reset / active-filter chips)
    05. Trade-detail drawer (static template patched from TRADES)
    06. Export menu, per-page, refresh, sorting, pagination
    07. Charts (P/L line, distribution doughnut) + theme re-color
    ================================================== */

(function () {
  /* ------------------------------------------------------------------ */
  /* 01. Init & guard                                                   */
  /* ------------------------------------------------------------------ */
  if (!document.getElementById("trade-history")) return;

  const html = document.documentElement;
  const isDark = () => html.classList.contains("dark");
  const tickColor = () => (isDark() ? "#94A3B8" : "#64748B");
  const gridColor = () => (isDark() ? "rgba(255,255,255,0.05)" : "rgba(0,0,0,0.05)");
  const refreshIcons = () => window.lucide && lucide.createIcons();

  /* ------------------------------------------------------------------ */
  /* 02. Toast                                                          */
  /* ------------------------------------------------------------------ */
  const toast = document.getElementById("thToast");
  const toastTitle = document.getElementById("thToastTitle");
  const toastMsg = document.getElementById("thToastMessage");
  let toastTimer;
  function showToast(title, message) {
    if (!toast) return;
    if (title) toastTitle.textContent = title;
    if (message) toastMsg.textContent = message;
    toast.classList.add("active");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => toast.classList.remove("active"), 3000);
  }
  document.querySelector(".th-toast-close")?.addEventListener("click", () => toast.classList.remove("active"));

  /* ------------------------------------------------------------------ */
  /* 03. Tabs + search                                                  */
  /* ------------------------------------------------------------------ */
  const tabs = document.querySelectorAll(".th-tab");
  const rows = Array.from(document.querySelectorAll(".th-row"));
  const noResults = document.getElementById("thNoResults");
  let activeTab = "all";
  let searchTerm = "";

  function applyFilters() {
    let visible = 0;
    rows.forEach((row) => {
      let show = true;
      if (activeTab === "forex" || activeTab === "crypto") show = row.dataset.type === activeTab;
      else if (activeTab === "winning") show = row.dataset.result === "win";
      else if (activeTab === "losing") show = row.dataset.result === "loss";
      if (show && searchTerm) show = row.textContent.toLowerCase().includes(searchTerm);
      row.style.display = show ? "" : "none";
      if (show) visible++;
    });
    noResults?.classList.toggle("hidden", visible !== 0);
  }

  tabs.forEach((tab) => {
    tab.addEventListener("click", () => {
      activeTab = tab.dataset.tab;
      tabs.forEach((t) => t.classList.remove("active"));
      tab.classList.add("active");
      applyFilters();
    });
  });

  document.getElementById("thTableSearch")?.addEventListener("input", (e) => {
    searchTerm = e.target.value.toLowerCase();
    applyFilters();
  });

  /* ------------------------------------------------------------------ */
  /* 04. Filter drawer                                                  */
  /* ------------------------------------------------------------------ */
  const overlay = document.getElementById("thDrawerOverlay");
  const filterDrawer = document.getElementById("thFilterDrawer");
  const tradeDrawer = document.getElementById("thTradeDrawer");

  function openDrawer(drawer) {
    overlay?.classList.add("active");
    drawer?.classList.add("active");
    refreshIcons();
  }
  function closeDrawers() {
    overlay?.classList.remove("active");
    filterDrawer?.classList.remove("active");
    tradeDrawer?.classList.remove("active");
  }

  document.getElementById("thFilterBtn")?.addEventListener("click", () => openDrawer(filterDrawer));
  document.querySelectorAll(".th-close-drawer").forEach((b) => b.addEventListener("click", closeDrawers));
  overlay?.addEventListener("click", closeDrawers);

  const activeFilters = document.getElementById("thActiveFilters");
  document.getElementById("thApplyFilters")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Filters Applied", "Trade history has been filtered");
    activeFilters?.classList.remove("hidden");
    activeFilters?.classList.add("flex");
  });

  document.getElementById("thResetFilters")?.addEventListener("click", () => {
    filterDrawer.querySelectorAll(".th-filter-cb").forEach((cb) => (cb.checked = true));
    filterDrawer.querySelectorAll('input[type="date"], input[type="number"], input[type="text"]').forEach((i) => (i.value = ""));
    showToast("Filters Reset", "All filters have been cleared");
  });

  // active-filter chip removal
  function updateActiveFilterVisibility() {
    if (activeFilters && activeFilters.querySelectorAll(".th-active-chip").length === 0) {
      activeFilters.classList.add("hidden");
      activeFilters.classList.remove("flex");
    }
  }
  document.querySelectorAll(".th-remove-filter").forEach((btn) => {
    btn.addEventListener("click", () => {
      btn.closest(".th-active-chip")?.remove();
      updateActiveFilterVisibility();
    });
  });
  document.getElementById("thClearFilters")?.addEventListener("click", () => {
    activeFilters?.classList.add("hidden");
    activeFilters?.classList.remove("flex");
    showToast("Filters Cleared", "All filters have been removed");
  });

  /* ------------------------------------------------------------------ */
  /* 05. Trade-detail drawer                                            */
  /* ------------------------------------------------------------------ */
  // Deterministic data for every static row's detail view.
  const TRADES = {
    "EUR/USD": { badge: "€$", grad: "from-blue-500 to-indigo-600", market: "Forex", marketCls: "bg-blue-500/15 text-blue-500", type: "Scalp", typeCls: "bg-sky-500/15 text-sky-500", pl: "+$170.00", pips: "+34 pips", win: true, dir: "BUY", entry: "1.0842", exit: "1.0876", sl: "1.0820", tp: "1.0890", lot: "0.50", dur: "2h 15m", source: "AI Generated", srcIcon: "sparkles", srcCls: "text-violet-400", entryTime: "Dec 28, 2024 • 14:32:15", exitTime: "Dec 28, 2024 • 16:47:30", rr: "1:1.54", risk: "22 pips", reward: "34 pips", analysis: "Strong bullish momentum detected with MACD crossover confirmation. RSI showed oversold recovery at entry. Price action formed a double bottom pattern near support level.", notes: "Perfect entry on the retest of the broken resistance. Market conditions were favorable with low volatility." },
    "BTC/USDT": { badge: "₿", grad: "from-amber-500 to-orange-500", market: "Crypto", marketCls: "bg-orange-500/15 text-orange-500", type: "Swing", typeCls: "bg-amber-500/15 text-amber-500", pl: "+$160.50", pips: "+1,070 pips", win: true, dir: "SELL", entry: "43,250", exit: "42,180", sl: "43,800", tp: "41,500", lot: "0.15", dur: "6h 45m", source: "Manual", srcIcon: "user", srcCls: "text-blue-500", entryTime: "Dec 28, 2024 • 11:18:42", exitTime: "Dec 28, 2024 • 18:03:42", rr: "1:1.94", risk: "550 pips", reward: "1,070 pips", analysis: "Bearish divergence confirmed on the 4H chart with a rejection at key resistance. Volume spike on the breakdown validated the short setup.", notes: "Took profit before the next support cluster. Disciplined exit on the rejection candle." },
    "GBP/USD": { badge: "£$", grad: "from-blue-500 to-indigo-600", market: "Forex", marketCls: "bg-blue-500/15 text-blue-500", type: "Day", typeCls: "bg-emerald-500/15 text-emerald-500", pl: "-$105.00", pips: "-35 pips", win: false, dir: "BUY", entry: "1.2715", exit: "1.2680", sl: "1.2680", tp: "1.2790", lot: "0.30", dur: "3h 22m", source: "AI Generated", srcIcon: "sparkles", srcCls: "text-violet-400", entryTime: "Dec 27, 2024 • 16:45:33", exitTime: "Dec 27, 2024 • 20:07:33", rr: "1:2.14", risk: "35 pips", reward: "75 pips", analysis: "Setup invalidated as price broke below the demand zone. Stop loss hit on a bearish news catalyst that reversed momentum.", notes: "Stop loss respected. Avoided a larger drawdown by keeping risk tight." },
    "ETH/USDT": { badge: "Ξ", grad: "from-amber-500 to-orange-500", market: "Crypto", marketCls: "bg-orange-500/15 text-orange-500", type: "Scalp", typeCls: "bg-sky-500/15 text-sky-500", pl: "+$142.50", pips: "+57 pips", win: true, dir: "BUY", entry: "2,285", exit: "2,342", sl: "2,255", tp: "2,360", lot: "0.25", dur: "1h 48m", source: "Bot Executed", srcIcon: "bot", srcCls: "text-cyan-500", entryTime: "Dec 27, 2024 • 09:12:08", exitTime: "Dec 27, 2024 • 11:00:08", rr: "1:1.90", risk: "30 pips", reward: "57 pips", analysis: "Bot executed a momentum scalp on the EMA crossover. Quick continuation off the 4H support held cleanly.", notes: "Automated entry and exit performed as backtested. Clean trade." },
    "USD/JPY": { badge: "$¥", grad: "from-blue-500 to-indigo-600", market: "Forex", marketCls: "bg-blue-500/15 text-blue-500", type: "Swing", typeCls: "bg-amber-500/15 text-amber-500", pl: "+$180.90", pips: "+67 pips", win: true, dir: "SELL", entry: "148.52", exit: "147.85", sl: "148.95", tp: "147.60", lot: "0.40", dur: "8h 30m", source: "AI Generated", srcIcon: "sparkles", srcCls: "text-violet-400", entryTime: "Dec 26, 2024 • 22:05:19", exitTime: "Dec 27, 2024 • 06:35:19", rr: "1:1.56", risk: "43 pips", reward: "67 pips", analysis: "AI flagged an overbought condition with a bearish engulfing at resistance. Yen strength on safe-haven flows confirmed the bias.", notes: "Held overnight through the Asian session. Trailed the stop into profit." },
    "SOL/USDT": { badge: "◎", grad: "from-amber-500 to-orange-500", market: "Crypto", marketCls: "bg-orange-500/15 text-orange-500", type: "Day", typeCls: "bg-emerald-500/15 text-emerald-500", pl: "+$135.00", pips: "+6.75 pips", win: true, dir: "BUY", entry: "98.45", exit: "105.20", sl: "95.00", tp: "106.00", lot: "2.00", dur: "5h 12m", source: "Manual", srcIcon: "user", srcCls: "text-blue-500", entryTime: "Dec 26, 2024 • 15:38:44", exitTime: "Dec 26, 2024 • 20:50:44", rr: "1:1.96", risk: "3.45 pips", reward: "6.75 pips", analysis: "Breakout above range high with strong volume. Bullish market structure supported the continuation move.", notes: "Manual breakout entry. Scaled out near the target zone." },
    "AUD/USD": { badge: "A$", grad: "from-blue-500 to-indigo-600", market: "Forex", marketCls: "bg-blue-500/15 text-blue-500", type: "Scalp", typeCls: "bg-sky-500/15 text-sky-500", pl: "-$126.00", pips: "-36 pips", win: false, dir: "SELL", entry: "0.6842", exit: "0.6878", sl: "0.6878", tp: "0.6790", lot: "0.35", dur: "0h 45m", source: "Bot Executed", srcIcon: "bot", srcCls: "text-cyan-500", entryTime: "Dec 25, 2024 • 10:22:11", exitTime: "Dec 25, 2024 • 11:07:11", rr: "1:1.44", risk: "36 pips", reward: "52 pips", analysis: "Bot short failed as commodity-linked currency rallied on risk-on sentiment. Stop hit quickly on the reversal.", notes: "Small scalp loss within risk parameters. Strategy still net positive." },
    "XRP/USDT": { badge: "XRP", grad: "from-amber-500 to-orange-500", market: "Crypto", marketCls: "bg-orange-500/15 text-orange-500", type: "Swing", typeCls: "bg-amber-500/15 text-amber-500", pl: "+$159.00", pips: "+0.0318 pips", win: true, dir: "BUY", entry: "0.5824", exit: "0.6142", sl: "0.5650", tp: "0.6200", lot: "500", dur: "12h 18m", source: "AI Generated", srcIcon: "sparkles", srcCls: "text-violet-400", entryTime: "Dec 24, 2024 • 19:55:28", exitTime: "Dec 25, 2024 • 08:13:28", rr: "1:1.83", risk: "0.0174 pips", reward: "0.0318 pips", analysis: "AI detected accumulation with a bullish flag breakout. Sentiment shift on positive regulatory news fueled the move.", notes: "Patient swing held through consolidation. Closed near the upper target." },
  };

  const set = (id, val) => {
    const el = document.getElementById(id);
    if (el && val != null) el.textContent = val;
  };

  function openTradeDetail(pair) {
    const t = TRADES[pair];
    if (!t) return;
    const winCls = t.win ? "text-emerald-500" : "text-red-500";
    const dirCls = t.dir === "BUY" ? "text-emerald-500" : "text-red-500";
    const dirIcon = t.dir === "BUY" ? "arrow-up-right" : "arrow-down-right";

    // badge wrapper gradient
    const badgeEl = document.getElementById("thDetailBadge");
    if (badgeEl) {
      badgeEl.textContent = t.badge;
      const wrap = badgeEl.parentElement;
      wrap.className = "w-14 h-14 rounded-xl bg-gradient-to-br " + t.grad + " flex items-center justify-center shrink-0";
    }
    set("thDetailPair", pair);

    const marketEl = document.getElementById("thDetailMarket");
    if (marketEl) { marketEl.textContent = t.market; marketEl.className = "inline-flex px-2.5 py-1 rounded-lg text-xs font-semibold " + t.marketCls; }
    const typeEl = document.getElementById("thDetailType");
    if (typeEl) { typeEl.textContent = t.type; typeEl.className = "inline-flex px-2.5 py-1 rounded-lg text-xs font-semibold " + t.typeCls; }

    const plEl = document.getElementById("thDetailPl");
    if (plEl) { plEl.textContent = t.pl; plEl.className = "text-2xl font-bold " + winCls; }
    const pipsEl = document.getElementById("thDetailPips");
    if (pipsEl) { pipsEl.textContent = t.pips; pipsEl.className = "text-sm " + winCls; }

    // direction block
    const dirEl = document.getElementById("thDetailDirection");
    if (dirEl) {
      dirEl.className = "flex items-center gap-1.5 " + dirCls;
      dirEl.querySelector("i")?.setAttribute("data-lucide", dirIcon);
      const dirSpan = dirEl.querySelector("span");
      if (dirSpan) dirSpan.textContent = t.dir;
    }
    // source block
    const srcEl = document.getElementById("thDetailSource");
    if (srcEl) {
      const srcIcon = srcEl.querySelector("i");
      const srcSpan = srcEl.querySelector("span");
      if (srcIcon) { srcIcon.setAttribute("data-lucide", t.srcIcon); srcIcon.className = "w-4 h-4 " + t.srcCls + " shrink-0"; }
      if (srcSpan) { srcSpan.textContent = t.source; srcSpan.className = "font-semibold " + t.srcCls; }
    }

    set("thDetailEntry", t.entry);
    set("thDetailExit", t.exit);
    set("thDetailSl", t.sl);
    set("thDetailTp", t.tp);
    set("thDetailLot", t.lot);
    set("thDetailDuration", t.dur);
    set("thDetailEntryTime", t.entryTime);
    set("thDetailExitTime", t.exitTime);
    set("thDetailRr", t.rr);
    set("thDetailRisk", t.risk);
    set("thDetailReward", t.reward);
    set("thDetailAnalysis", t.analysis);
    set("thDetailNotes", t.notes);

    openDrawer(tradeDrawer);
  }

  document.querySelectorAll(".th-view-trade").forEach((btn) => {
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      openTradeDetail(btn.dataset.pair);
    });
  });
  document.querySelectorAll(".th-replay").forEach((btn) => {
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      showToast("Replaying Trade", `Replaying ${btn.dataset.pair} trade…`);
    });
  });
  document.querySelector(".th-copy-trade")?.addEventListener("click", () => showToast("Trade Copied", "Trade parameters copied to clipboard"));
  document.querySelector(".th-replay-trade")?.addEventListener("click", () => showToast("Replaying Trade", "Trade replay started"));

  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") closeDrawers();
  });

  /* ------------------------------------------------------------------ */
  /* 06. Export menu, per-page, refresh, sorting, pagination            */
  /* ------------------------------------------------------------------ */
  const exportBtn = document.getElementById("thExportBtn");
  const exportMenu = document.getElementById("thExportMenu");
  exportBtn?.addEventListener("click", (e) => {
    e.stopPropagation();
    exportMenu?.classList.toggle("hidden");
  });
  document.querySelectorAll(".th-export").forEach((opt) => {
    opt.addEventListener("click", () => {
      const fmt = (opt.dataset.format || "").toUpperCase();
      exportMenu?.classList.add("hidden");
      showToast("Export Started", `Exporting trades as ${fmt}…`);
      setTimeout(() => showToast("Export Complete", `Your ${fmt} file is ready for download`), 1500);
    });
  });
  document.addEventListener("click", (e) => {
    if (exportBtn && !exportBtn.contains(e.target) && !exportMenu?.contains(e.target)) {
      exportMenu?.classList.add("hidden");
    }
  });

  document.getElementById("thPerPage")?.addEventListener("change", (e) => {
    showToast("Updated", `Showing ${e.target.value} trades per page`);
  });

  const refreshBtn = document.getElementById("thRefreshBtn");
  refreshBtn?.addEventListener("click", () => {
    const icon = refreshBtn.querySelector("i");
    icon?.classList.add("animate-spin");
    setTimeout(() => {
      icon?.classList.remove("animate-spin");
      showToast("Refreshed", "Trade history updated");
    }, 1000);
  });

  // Column sorting (visual + toast; mirrors mockup behavior)
  document.querySelectorAll(".th-sortable").forEach((header) => {
    header.addEventListener("click", () => {
      const sortKey = header.dataset.sort;
      const isAsc = header.dataset.dir === "asc";
      document.querySelectorAll(".th-sortable").forEach((h) => delete h.dataset.dir);
      header.dataset.dir = isAsc ? "desc" : "asc";
      showToast("Sorted", `Sorted by ${sortKey} ${isAsc ? "descending" : "ascending"}`);
    });
  });

  // Numbered pagination buttons
  const pageBtns = document.querySelectorAll(".th-page");
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
  const rangeBtns = document.querySelectorAll(".th-range");
  rangeBtns.forEach((btn) => {
    btn.addEventListener("click", () => {
      rangeBtns.forEach((b) => {
        b.classList.remove("active", "border-accent", "bg-accent/15", "text-accent");
        b.classList.add("border-border", "bg-panel", "text-muted");
      });
      btn.classList.add("active", "border-accent", "bg-accent/15", "text-accent");
      btn.classList.remove("border-border", "bg-panel", "text-muted");
      updatePlChart(btn.dataset.range);
    });
  });

  /* ------------------------------------------------------------------ */
  /* 07. Charts + theme re-color                                        */
  /* ------------------------------------------------------------------ */
  const charts = {};

  const PL_DATA = {
    "7d": { labels: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"], cum: [0, 320, 520, 380, 680, 920, 1150], daily: [0, 320, 200, -140, 300, 240, 230] },
    "30d": { labels: ["W1", "W2", "W3", "W4"], cum: [0, 1150, 2480, 4280], daily: [0, 1150, 1330, 1800] },
    "90d": { labels: ["Jan", "Feb", "Mar"], cum: [0, 4280, 9650], daily: [0, 4280, 5370] },
  };

  function initCharts() {
    if (typeof Chart === "undefined") return;

    const pl = document.getElementById("thPlChart");
    if (pl && !charts.pl) {
      const d = PL_DATA["7d"];
      charts.pl = new Chart(pl.getContext("2d"), {
        type: "line",
        data: {
          labels: d.labels,
          datasets: [
            { label: "Cumulative P/L", data: d.cum, borderColor: "#10b981", backgroundColor: "rgba(16,185,129,0.1)", borderWidth: 3, fill: true, tension: 0.4, pointRadius: 5, pointBackgroundColor: "#10b981", pointBorderColor: isDark() ? "#0C1222" : "#FFFFFF", pointBorderWidth: 2 },
            { label: "Daily P/L", data: d.daily, borderColor: "#6366f1", backgroundColor: "transparent", borderWidth: 2, borderDash: [5, 5], fill: false, tension: 0.4, pointRadius: 4, pointBackgroundColor: "#6366f1" },
          ],
        },
        options: {
          responsive: true,
          maintainAspectRatio: false,
          interaction: { intersect: false, mode: "index" },
          plugins: {
            legend: { position: "top", align: "end", labels: { color: tickColor(), usePointStyle: true, pointStyle: "circle", padding: 16, font: { size: 12 } } },
            tooltip: { callbacks: { label: (ctx) => `${ctx.dataset.label}: $${ctx.parsed.y.toLocaleString()}` } },
          },
          scales: {
            y: { beginAtZero: true, grid: { color: gridColor() }, ticks: { color: tickColor(), callback: (v) => "$" + v } },
            x: { grid: { display: false }, ticks: { color: tickColor() } },
          },
        },
      });
    }

    const dist = document.getElementById("thDistributionChart");
    if (dist && !charts.dist) {
      charts.dist = new Chart(dist.getContext("2d"), {
        type: "doughnut",
        data: {
          labels: ["Forex Wins", "Forex Losses", "Crypto Wins", "Crypto Losses"],
          datasets: [{ data: [45, 18, 28, 9], backgroundColor: ["#10b981", "rgba(16,185,129,0.3)", "#f97316", "rgba(249,115,22,0.3)"], borderWidth: 0, hoverOffset: 8 }],
        },
        options: {
          responsive: true,
          maintainAspectRatio: false,
          cutout: "65%",
          plugins: {
            legend: { position: "bottom", labels: { color: tickColor(), usePointStyle: true, pointStyle: "circle", padding: 14, font: { size: 11 } } },
            tooltip: { callbacks: { label: (ctx) => `${ctx.label}: ${ctx.parsed}%` } },
          },
        },
      });
    }
  }

  function updatePlChart(range) {
    if (!charts.pl) return;
    const d = PL_DATA[range] || PL_DATA["7d"];
    charts.pl.data.labels = d.labels;
    charts.pl.data.datasets[0].data = d.cum;
    charts.pl.data.datasets[1].data = d.daily;
    charts.pl.update();
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
    if (charts.pl) {
      charts.pl.data.datasets[0].pointBorderColor = isDark() ? "#0C1222" : "#FFFFFF";
      charts.pl.update();
    }
  }
  document.getElementById("themeToggle")?.addEventListener("click", () => setTimeout(recolorCharts, 0));

  // Chart.js loads with defer; init once ready.
  if (typeof Chart !== "undefined") {
    initCharts();
  } else {
    window.addEventListener("load", initCharts);
  }
})();
