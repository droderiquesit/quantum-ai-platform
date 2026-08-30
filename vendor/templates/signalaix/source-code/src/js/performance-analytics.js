/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: performance-analytics.html
 * description: SignalAIX - Performance Analytics Page Controller
 *              Self-contained; mirrors the reference mockup's functionality
 *              (date range, export menu, generate-report / filter-trades /
 *               strategy-details / trade-details drawers, main tabs with
 *               lazy-init per-tab charts, equity-period chips, returns-view
 *               chips, asset filter, year filter, recent-trades search, toast).
 *              All markup lives in performance-analytics.html; this file only
 *              modifies the DOM (text/values/classes/visibility) and renders
 *              Chart.js — it never injects HTML strings.
 * author: SignalAIX

    -------------------------------------------------
    Table of Contents
    -------------------------------------------------
    01. Init & guard
    02. Toast
    03. Export menu + date range
    04. Drawers (report / filter / strategy / trade)
    05. Main tabs (lazy chart init per tab)
    06. Equity period + returns view + asset filter + year filter chips
    07. Recent-trades search
    08. Strategy details + trade details populate
    09. Charts (sparklines + per-tab) + theme re-color
    ================================================== */

(function () {
  /* ------------------------------------------------------------------ */
  /* 01. Init & guard                                                   */
  /* ------------------------------------------------------------------ */
  if (!document.getElementById("performance-analytics")) return;

  const html = document.documentElement;
  const isDark = () => html.classList.contains("dark");
  const tickColor = () => (isDark() ? "#94A3B8" : "#64748B");
  const gridColor = () => (isDark() ? "rgba(255,255,255,0.06)" : "rgba(0,0,0,0.06)");
  const refreshIcons = () => window.lucide && window.lucide.createIcons();

  /* ------------------------------------------------------------------ */
  /* 02. Toast                                                          */
  /* ------------------------------------------------------------------ */
  const toast = document.getElementById("pa2Toast");
  const toastTitle = document.getElementById("pa2ToastTitle");
  const toastMsg = document.getElementById("pa2ToastMessage");
  let toastTimer;
  function showToast(title, message) {
    if (!toast) return;
    if (title) toastTitle.textContent = title;
    if (message) toastMsg.textContent = message;
    toast.classList.add("active");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => toast.classList.remove("active"), 3000);
  }
  document.querySelector(".pa2-toast-close")?.addEventListener("click", () => toast?.classList.remove("active"));

  /* ------------------------------------------------------------------ */
  /* 03. Export menu + date range                                       */
  /* ------------------------------------------------------------------ */
  const exportBtn = document.getElementById("pa2ExportBtn");
  const exportMenu = document.getElementById("pa2ExportMenu");
  exportBtn?.addEventListener("click", (e) => {
    e.stopPropagation();
    exportMenu?.classList.toggle("hidden");
  });
  document.addEventListener("click", () => exportMenu?.classList.add("hidden"));
  document.querySelectorAll(".pa2-export-item").forEach((item) => {
    item.addEventListener("click", () => {
      const fmt = (item.dataset.format || "csv").toUpperCase();
      exportMenu?.classList.add("hidden");
      showToast("Export Started", `Exporting data as ${fmt}...`);
    });
  });

  document.getElementById("pa2DateRange")?.addEventListener("change", (e) => {
    const label = e.target.options[e.target.selectedIndex].text;
    showToast("Range Updated", `Showing data for ${label}`);
  });

  /* ------------------------------------------------------------------ */
  /* 04. Drawers (report / filter / strategy / trade)                   */
  /* ------------------------------------------------------------------ */
  const overlay = document.getElementById("pa2DrawerOverlay");
  const reportDrawer = document.getElementById("pa2ReportDrawer");
  const filterDrawer = document.getElementById("pa2FilterDrawer");
  const strategyDrawer = document.getElementById("pa2StrategyDrawer");
  const tradeDrawer = document.getElementById("pa2TradeDrawer");
  const allDrawers = [reportDrawer, filterDrawer, strategyDrawer, tradeDrawer];

  function openDrawer(drawer) {
    allDrawers.forEach((d) => d?.classList.remove("active"));
    drawer?.classList.add("active");
    overlay?.classList.add("active");
    refreshIcons();
  }
  function closeDrawers() {
    allDrawers.forEach((d) => d?.classList.remove("active"));
    overlay?.classList.remove("active");
  }
  overlay?.addEventListener("click", closeDrawers);
  document.querySelectorAll(".pa2-close-drawer").forEach((btn) => btn.addEventListener("click", closeDrawers));

  document.getElementById("pa2GenerateBtn")?.addEventListener("click", () => openDrawer(reportDrawer));
  document.getElementById("pa2FilterTradesBtn")?.addEventListener("click", () => openDrawer(filterDrawer));

  document.querySelector(".pa2-generate-report")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Report Generated", "Your report is ready for download.");
  });
  document.querySelector(".pa2-apply-filters")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Filters Applied", "Trade list has been updated.");
  });
  document.querySelector(".pa2-strat-pause")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Strategy Paused", "The strategy has been paused.");
  });
  document.querySelector(".pa2-strat-edit")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Edit Strategy", "Strategy editor coming soon.");
  });

  // Pill helpers (single-select within a group)
  function activatePill(btn, group) {
    group.forEach((b) => {
      b.classList.remove("active", "border-accent", "bg-accent/15", "text-accent");
      b.classList.add("border-border", "bg-panel", "text-muted");
    });
    btn.classList.add("active", "border-accent", "bg-accent/15", "text-accent");
    btn.classList.remove("border-border", "bg-panel", "text-muted");
  }

  // Report format pills
  const reportFmts = Array.from(document.querySelectorAll(".pa2-report-fmt"));
  reportFmts.forEach((btn) => btn.addEventListener("click", () => activatePill(btn, reportFmts)));

  // Filter-drawer option groups
  document.querySelectorAll(".pa2-filter-group").forEach((group) => {
    const opts = Array.from(group.querySelectorAll(".pa2-filter-opt"));
    opts.forEach((btn) => btn.addEventListener("click", () => activatePill(btn, opts)));
  });

  /* ------------------------------------------------------------------ */
  /* 05. Main tabs (lazy chart init per tab)                            */
  /* ------------------------------------------------------------------ */
  document.querySelectorAll(".pa2-tab").forEach((tab) => {
    tab.addEventListener("click", () => {
      const id = tab.dataset.tab;
      document.querySelectorAll(".pa2-tab").forEach((t) => t.classList.remove("active"));
      tab.classList.add("active");
      document.querySelectorAll(".pa2-pane").forEach((p) => p.classList.remove("active"));
      document.getElementById(`pa2-tab-${id}`)?.classList.add("active");
      requestAnimationFrame(() => initTabCharts(id));
      refreshIcons();
    });
  });

  /* ------------------------------------------------------------------ */
  /* 06. Period / view / asset / year chips                            */
  /* ------------------------------------------------------------------ */
  const equityPeriods = Array.from(document.querySelectorAll(".pa2-period"));
  equityPeriods.forEach((btn) =>
    btn.addEventListener("click", () => {
      activatePill(btn, equityPeriods);
      updateEquity(btn.dataset.period);
    })
  );

  const returnViews = Array.from(document.querySelectorAll(".pa2-return-view"));
  returnViews.forEach((btn) =>
    btn.addEventListener("click", () => {
      activatePill(btn, returnViews);
      updateCumulative(btn.dataset.view);
    })
  );

  const assetFilters = Array.from(document.querySelectorAll(".pa2-asset-filter"));
  assetFilters.forEach((btn) =>
    btn.addEventListener("click", () => {
      activatePill(btn, assetFilters);
      const v = btn.dataset.asset;
      document.querySelectorAll(".pa2-asset-row").forEach((row) => {
        row.style.display = v === "all" || row.dataset.asset === v ? "" : "none";
      });
    })
  );

  document.getElementById("pa2YearFilter")?.addEventListener("change", (e) => {
    showToast("Year Changed", `Showing monthly returns for ${e.target.value}`);
  });

  /* ------------------------------------------------------------------ */
  /* 07. Recent-trades search                                           */
  /* ------------------------------------------------------------------ */
  document.getElementById("pa2TradeSearch")?.addEventListener("input", (e) => {
    const term = e.target.value.toLowerCase();
    document.querySelectorAll(".pa2-trade-row").forEach((row) => {
      row.style.display = row.textContent.toLowerCase().includes(term) ? "" : "none";
    });
  });

  /* ------------------------------------------------------------------ */
  /* 08. Strategy details + trade details populate                      */
  /* ------------------------------------------------------------------ */
  const STRATEGIES = {
    trend: { name: "Trend Following", desc: "Momentum-based strategy", icon: "trending-up", hero: ["from-emerald-500", "to-emerald-600"], win: "72.4%", pl: "+$12,450", tf: "4H, Daily", entry: "EMA Cross + RSI", sl: "2x ATR", tp: "3x ATR", risk: "1.5%", trades: "87", pf: "2.85", avgWin: "$143", avgLoss: "-$52" },
    mean: { name: "Mean Reversion", desc: "Counter-trend strategy", icon: "git-merge", hero: ["from-sky-500", "to-indigo-500"], win: "65.8%", pl: "+$8,234", tf: "1H, 4H", entry: "Bollinger + RSI", sl: "1.5x ATR", tp: "2x ATR", risk: "1.2%", trades: "62", pf: "2.12", avgWin: "$118", avgLoss: "-$58" },
    breakout: { name: "Breakout Scalping", desc: "High-frequency strategy", icon: "zap", hero: ["from-violet-500", "to-purple-600"], win: "58.2%", pl: "+$4,172", tf: "5M, 15M", entry: "Range Break + Volume", sl: "1x ATR", tp: "1.5x ATR", risk: "0.8%", trades: "79", pf: "1.68", avgWin: "$86", avgLoss: "-$54" },
  };
  const HERO_GRADS = ["from-emerald-500", "to-emerald-600", "from-sky-500", "to-indigo-500", "from-violet-500", "to-purple-600"];

  const stratHero = document.getElementById("pa2StratHero");
  const stratIcon = document.getElementById("pa2StratIcon");
  const stratName = document.getElementById("pa2StratName");
  const stratDesc = document.getElementById("pa2StratDesc");
  const stratWin = document.getElementById("pa2StratWin");
  const stratPl = document.getElementById("pa2StratPl");
  const stratTf = document.getElementById("pa2StratTf");
  const stratEntry = document.getElementById("pa2StratEntry");
  const stratSl = document.getElementById("pa2StratSl");
  const stratTp = document.getElementById("pa2StratTp");
  const stratRisk = document.getElementById("pa2StratRisk");
  const stratTrades = document.getElementById("pa2StratTrades");
  const stratPf = document.getElementById("pa2StratPf");
  const stratAvgWin = document.getElementById("pa2StratAvgWin");
  const stratAvgLoss = document.getElementById("pa2StratAvgLoss");

  function openStrategy(key) {
    const s = STRATEGIES[key];
    if (!s) return;
    stratHero?.classList.remove(...HERO_GRADS);
    stratHero?.classList.add(...s.hero);
    stratIcon?.setAttribute("data-lucide", s.icon);
    if (stratName) stratName.textContent = s.name;
    if (stratDesc) stratDesc.textContent = s.desc;
    if (stratWin) stratWin.textContent = s.win;
    if (stratPl) stratPl.textContent = s.pl;
    if (stratTf) stratTf.textContent = s.tf;
    if (stratEntry) stratEntry.textContent = s.entry;
    if (stratSl) stratSl.textContent = s.sl;
    if (stratTp) stratTp.textContent = s.tp;
    if (stratRisk) stratRisk.textContent = s.risk;
    if (stratTrades) stratTrades.textContent = s.trades;
    if (stratPf) stratPf.textContent = s.pf;
    if (stratAvgWin) stratAvgWin.textContent = s.avgWin;
    if (stratAvgLoss) stratAvgLoss.textContent = s.avgLoss;
    openDrawer(strategyDrawer);
  }
  document.querySelectorAll(".pa2-strategy-view").forEach((btn) =>
    btn.addEventListener("click", () => openStrategy(btn.dataset.strategy))
  );

  // Trade details — read static values straight off the clicked row's cells
  const tdPlBox = document.getElementById("pa2TradePlBox");
  const tdPlLabel = document.getElementById("pa2TradePlLabel");
  const tdStatusBadge = document.getElementById("pa2TradeStatusBadge");
  const tdPl = document.getElementById("pa2TradePl");
  const tdRoi = document.getElementById("pa2TradeRoi");
  const tdAsset = document.getElementById("pa2TradeAsset");
  const tdType = document.getElementById("pa2TradeType");
  const tdEntry = document.getElementById("pa2TradeEntry");
  const tdExit = document.getElementById("pa2TradeExit");
  const tdDuration = document.getElementById("pa2TradeDuration");
  const tdStatus = document.getElementById("pa2TradeStatus");

  function setColorPair(el, profit) {
    el?.classList.remove("text-emerald-500", "text-red-500");
    el?.classList.add(profit ? "text-emerald-500" : "text-red-500");
  }

  document.querySelectorAll(".pa2-trade-view").forEach((btn) =>
    btn.addEventListener("click", () => {
      const row = btn.closest(".pa2-trade-row");
      if (!row) return;
      const cells = row.querySelectorAll("td");
      const asset = cells[0].textContent.trim();
      const type = cells[1].textContent.trim();
      const entry = cells[2].textContent.trim();
      const exit = cells[3].textContent.trim();
      const pl = cells[4].textContent.trim();
      const roi = cells[5].textContent.trim();
      const duration = cells[6].textContent.trim();
      const status = cells[7].textContent.trim();
      const profit = status.toLowerCase() === "win";

      if (tdAsset) tdAsset.textContent = asset;
      if (tdType) tdType.textContent = type;
      if (tdEntry) tdEntry.textContent = entry;
      if (tdExit) tdExit.textContent = exit;
      if (tdPl) tdPl.textContent = pl;
      if (tdRoi) tdRoi.textContent = roi;
      if (tdDuration) tdDuration.textContent = duration;
      if (tdStatus) tdStatus.textContent = status;

      if (tdStatusBadge) {
        tdStatusBadge.textContent = status;
        tdStatusBadge.classList.remove("bg-emerald-500/15", "text-emerald-500", "bg-red-500/15", "text-red-500");
        tdStatusBadge.classList.add(profit ? "bg-emerald-500/15" : "bg-red-500/15", profit ? "text-emerald-500" : "text-red-500");
      }
      tdPlBox?.classList.remove("from-emerald-500/20", "to-teal-500/10", "from-red-500/20", "to-orange-500/10");
      tdPlBox?.classList.add(profit ? "from-emerald-500/20" : "from-red-500/20", profit ? "to-teal-500/10" : "to-orange-500/10");
      setColorPair(tdPlLabel, profit);
      setColorPair(tdPl, profit);
      setColorPair(tdRoi, profit);

      openDrawer(tradeDrawer);
    })
  );

  /* ------------------------------------------------------------------ */
  /* 09. Charts                                                         */
  /* ------------------------------------------------------------------ */
  const charts = {};

  function baseScales(extra) {
    return Object.assign(
      {
        y: { grid: { color: gridColor() }, ticks: { color: tickColor() } },
        x: { grid: { display: false }, ticks: { color: tickColor() } },
      },
      extra || {}
    );
  }

  function sparkline(id, data, color) {
    const el = document.getElementById(id);
    if (!el || charts[id] || typeof Chart === "undefined") return;
    charts[id] = new Chart(el.getContext("2d"), {
      type: "line",
      data: { labels: data.map((_, i) => i), datasets: [{ data, borderColor: color, backgroundColor: "transparent", tension: 0.4, pointRadius: 0, borderWidth: 2 }] },
      options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { display: false }, tooltip: { enabled: false } }, scales: { x: { display: false }, y: { display: false } } },
    });
  }

  function initOverviewCharts() {
    if (typeof Chart === "undefined") return;

    sparkline("pa2ReturnsSpark", [5, 8, 12, 10, 15, 18, 22, 20, 25, 28], "#10b981");
    sparkline("pa2ProfitFactorSpark", [2.1, 2.3, 2.0, 2.4, 2.2, 2.5, 2.3, 2.6, 2.4, 2.45], "#8b5cf6");
    sparkline("pa2DrawdownSpark", [-5, -3, -6, -4, -7, -5, -8, -6, -9, -8.2], "#f59e0b");

    const equity = document.getElementById("pa2EquityChart");
    if (equity && !charts.equity) {
      charts.equity = new Chart(equity.getContext("2d"), {
        type: "line",
        data: {
          labels: ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"],
          datasets: [{ label: "Portfolio Value", data: [100000, 105200, 103100, 111500, 115100, 113600, 120400, 124600, 123800, 130900, 136800, 139200], borderColor: "#10b981", backgroundColor: "rgba(16,185,129,0.1)", fill: true, tension: 0.4, pointRadius: 0, pointHoverRadius: 6, borderWidth: 3 }],
        },
        options: {
          responsive: true,
          maintainAspectRatio: false,
          interaction: { intersect: false, mode: "index" },
          plugins: { legend: { display: false }, tooltip: { callbacks: { label: (ctx) => `$${ctx.raw.toLocaleString()}` } } },
          scales: baseScales({ y: { grid: { color: gridColor() }, ticks: { color: tickColor(), callback: (v) => "$" + v / 1000 + "k" } } }),
        },
      });
    }

    const winLoss = document.getElementById("pa2WinLossChart");
    if (winLoss && !charts.winLoss) {
      charts.winLoss = new Chart(winLoss.getContext("2d"), {
        type: "doughnut",
        data: { labels: ["Winning Trades", "Losing Trades", "Breakeven"], datasets: [{ data: [156, 72, 5], backgroundColor: ["#10b981", "#ef4444", "#64748b"], borderWidth: 0, hoverOffset: 10 }] },
        options: { responsive: true, maintainAspectRatio: false, cutout: "70%", plugins: { legend: { position: "bottom", labels: { color: tickColor(), padding: 20, usePointStyle: true } } } },
      });
    }
  }

  function initTabCharts(tabId) {
    if (typeof Chart === "undefined") return;

    if (tabId === "returns") {
      const cum = document.getElementById("pa2CumulativeChart");
      if (cum && !charts.cum) {
        charts.cum = new Chart(cum.getContext("2d"), {
          type: "line",
          data: {
            labels: Array.from({ length: 30 }, (_, i) => `Day ${i + 1}`),
            datasets: [{ label: "Returns %", data: [0, 1.2, 2.5, 1.8, 3.2, 4.1, 3.5, 5.2, 6.8, 5.9, 7.4, 8.2, 7.5, 9.1, 10.2, 9.8, 11.5, 12.8, 11.9, 13.4, 14.2, 13.8, 15.6, 16.9, 15.8, 17.2, 18.5, 17.9, 19.4, 20.8], borderColor: "#10b981", backgroundColor: "rgba(16,185,129,0.1)", fill: true, tension: 0.3, pointRadius: 0, borderWidth: 2 }],
          },
          options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { display: false } }, scales: baseScales({ y: { grid: { color: gridColor() }, ticks: { color: tickColor(), callback: (v) => v + "%" } }, x: { grid: { display: false }, ticks: { color: tickColor(), maxTicksLimit: 10 } } }) },
        });
      }
    }

    if (tabId === "trades") {
      const size = document.getElementById("pa2TradeSizeChart");
      if (size && !charts.size) {
        charts.size = new Chart(size.getContext("2d"), {
          type: "bar",
          data: { labels: ["$0-100", "$100-250", "$250-500", "$500-1K", "$1K+"], datasets: [{ label: "Trade Count", data: [25, 68, 85, 42, 8], backgroundColor: ["rgba(16,185,129,0.8)", "rgba(20,184,166,0.8)", "rgba(99,102,241,0.8)", "rgba(139,92,246,0.8)", "rgba(245,158,11,0.8)"], borderRadius: 8 }] },
          options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { display: false } }, scales: baseScales() },
        });
      }
      const byDay = document.getElementById("pa2TradesByDayChart");
      if (byDay && !charts.byDay) {
        charts.byDay = new Chart(byDay.getContext("2d"), {
          type: "bar",
          data: { labels: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"], datasets: [{ label: "Wins", data: [28, 32, 35, 25, 24, 8, 4], backgroundColor: "#10b981", borderRadius: 6 }, { label: "Losses", data: [12, 14, 18, 10, 12, 4, 2], backgroundColor: "#ef4444", borderRadius: 6 }] },
          options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { position: "top", labels: { color: tickColor(), usePointStyle: true } } }, scales: baseScales() },
        });
      }
    }

    if (tabId === "risk") {
      const dd = document.getElementById("pa2DrawdownChart");
      if (dd && !charts.dd) {
        charts.dd = new Chart(dd.getContext("2d"), {
          type: "line",
          data: {
            labels: Array.from({ length: 30 }, (_, i) => `Day ${i + 1}`),
            datasets: [{ label: "Drawdown %", data: [0, -0.5, -1.2, -0.8, -2.1, -1.5, -3.2, -2.8, -4.5, -3.1, -5.8, -4.2, -6.2, -5.1, -7.4, -6.8, -8.2, -7.5, -6.9, -5.4, -4.8, -3.2, -2.5, -1.8, -1.2, -0.6, -0.3, 0, -0.4, -0.2], borderColor: "#ef4444", backgroundColor: "rgba(239,68,68,0.1)", fill: true, tension: 0.3, pointRadius: 0, borderWidth: 2 }],
          },
          options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { display: false } }, scales: baseScales({ y: { reverse: true, grid: { color: gridColor() }, ticks: { color: tickColor(), callback: (v) => v + "%" } }, x: { grid: { display: false }, ticks: { color: tickColor(), maxTicksLimit: 10 } } }) },
        });
      }
    }

    if (tabId === "strategies") {
      const sc = document.getElementById("pa2StrategyChart");
      if (sc && !charts.strategy) {
        charts.strategy = new Chart(sc.getContext("2d"), {
          type: "line",
          data: {
            labels: ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"],
            datasets: [
              { label: "Trend Following", data: [0, 2200, 4100, 5800, 7200, 8900, 9500, 10200, 11400, 11800, 12100, 12450], borderColor: "#10b981", backgroundColor: "transparent", tension: 0.4, borderWidth: 3, pointRadius: 0 },
              { label: "Mean Reversion", data: [0, 1500, 2800, 3500, 4200, 5100, 5800, 6400, 7100, 7600, 7900, 8234], borderColor: "#6366f1", backgroundColor: "transparent", tension: 0.4, borderWidth: 3, pointRadius: 0 },
              { label: "Breakout Scalping", data: [0, 800, 1400, 1800, 2100, 2600, 2900, 3200, 3500, 3800, 4000, 4172], borderColor: "#8b5cf6", backgroundColor: "transparent", tension: 0.4, borderWidth: 3, pointRadius: 0 },
            ],
          },
          options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { position: "top", labels: { color: tickColor(), usePointStyle: true, padding: 20 } } }, scales: baseScales({ y: { grid: { color: gridColor() }, ticks: { color: tickColor(), callback: (v) => "$" + v / 1000 + "k" } } }) },
        });
      }
    }

    if (tabId === "assets") {
      const alloc = document.getElementById("pa2AllocationChart");
      if (alloc && !charts.alloc) {
        charts.alloc = new Chart(alloc.getContext("2d"), {
          type: "doughnut",
          data: { labels: ["Forex", "Crypto", "Indices", "Commodities"], datasets: [{ data: [45, 35, 15, 5], backgroundColor: ["#0ea5e9", "#8b5cf6", "#10b981", "#f59e0b"], borderWidth: 0, hoverOffset: 10 }] },
          options: { responsive: true, maintainAspectRatio: false, cutout: "65%", plugins: { legend: { display: false } } },
        });
      }
    }
  }

  function updateEquity(period) {
    showToast("Equity Curve", `Showing the ${(period || "").toUpperCase()} view`);
  }
  function updateCumulative(view) {
    if (!charts.cum) return;
    const label = (view || "cumulative").charAt(0).toUpperCase() + (view || "cumulative").slice(1);
    showToast("Returns View", `Switched to ${label} returns`);
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

  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") closeDrawers();
  });

  // Charts depend on the CDN Chart.js (deferred) — init Overview after load.
  if (document.readyState === "complete") initOverviewCharts();
  else window.addEventListener("load", initOverviewCharts);
})();
