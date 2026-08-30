/**
 * top-gainers-losers.js
 * theme name: SignalAIX
 * page: Top Gainers & Losers (<body id="top-gainers-losers">)
 * ported from: SignalAIX/MARKET ANALYTICS/Top Gainers & Losers/Top gainers losers_01.html
 *
 * Self-contained page controller (DOM-only — no HTML generated in JS).
 * Page-scoped hooks: tg-*, #tg*.
 *
 * Table of contents
 *   01. Guards & element refs
 *   02. Toast
 *   03. Drawers (asset detail + filter) over one overlay
 *   04. Mini sparkline charts (per row)
 *   05. Market overview chart (theme re-color)
 *   06. Detail drawer chart (per-open destroy/recreate)
 *   07. Market tabs (filter rows by type)
 *   08. Timeframe / view-toggle / chart-toggle / strength chips
 *   09. Export dropdown
 *   10. Table search + sort (reorder existing nodes)
 *   11. Asset detail open (DOM updates from row data attrs)
 *   12. Filters / watchlist / refresh / view-all helpers
 *   13. Keyboard shortcuts
 *   14. Init
 */
document.addEventListener("DOMContentLoaded", () => {
  /* 01. Guards & element refs ------------------------------------------------ */
  if (!document.getElementById("top-gainers-losers")) return;

  const htmlEl = document.documentElement;
  const refreshIcons = () => window.lucide?.createIcons?.();

  const overlay = document.getElementById("tgDrawerOverlay");
  const assetDrawer = document.getElementById("tgAssetDrawer");
  const filterDrawer = document.getElementById("tgFilterDrawer");

  const gainersBody = document.getElementById("tgGainersBody");
  const losersBody = document.getElementById("tgLosersBody");
  const gainersCard = document.getElementById("tgGainersCard");
  const losersCard = document.getElementById("tgLosersCard");
  const tablesGrid = document.getElementById("tgTablesGrid");

  /* 02. Toast ---------------------------------------------------------------- */
  const toast = document.getElementById("tgToast");
  const toastTitle = document.getElementById("tgToastTitle");
  const toastMessage = document.getElementById("tgToastMessage");
  let toastTimer;
  function showToast(title, message) {
    toastTitle.textContent = title;
    toastMessage.textContent = message;
    toast.classList.add("active");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => toast.classList.remove("active"), 3000);
  }

  /* 03. Drawers -------------------------------------------------------------- */
  function openDrawer(drawer) {
    overlay.classList.add("active");
    drawer.classList.add("active");
  }
  function closeDrawers() {
    overlay.classList.remove("active");
    assetDrawer.classList.remove("active");
    filterDrawer.classList.remove("active");
  }
  overlay.addEventListener("click", closeDrawers);
  document.querySelectorAll(".tg-close-drawer").forEach((b) => b.addEventListener("click", closeDrawers));

  /* 04. Mini sparkline charts ----------------------------------------------- */
  const sparkInstances = [];
  function sparkData(isGainer) {
    const data = [];
    let value = isGainer ? 50 : 80;
    for (let i = 0; i < 20; i++) {
      value += isGainer ? Math.random() * 5 - 1 : Math.random() * 5 - 4;
      data.push(Math.max(0, value));
    }
    return data;
  }
  function renderSparklines() {
    if (typeof Chart === "undefined") return;
    sparkInstances.forEach((c) => c.destroy());
    sparkInstances.length = 0;
    document.querySelectorAll("#tgGainersBody canvas, #tgLosersBody canvas").forEach((canvas) => {
      const isGainer = canvas.id.indexOf("Gain") !== -1;
      const color = isGainer ? "#10b981" : "#ef4444";
      const inst = new Chart(canvas.getContext("2d"), {
        type: "line",
        data: {
          labels: Array.from({ length: 20 }, (_, i) => i),
          datasets: [
            {
              data: sparkData(isGainer),
              borderColor: color,
              backgroundColor: color + "20",
              fill: true,
              tension: 0.4,
              borderWidth: 2,
              pointRadius: 0,
            },
          ],
        },
        options: {
          responsive: true,
          maintainAspectRatio: false,
          plugins: { legend: { display: false } },
          scales: { x: { display: false }, y: { display: false } },
        },
      });
      sparkInstances.push(inst);
    });
  }

  /* 05. Market overview chart ----------------------------------------------- */
  let marketChart;
  function chartTickColors() {
    const isDark = htmlEl.classList.contains("dark");
    return {
      tick: isDark ? "#94A3B8" : "#64748B",
      grid: isDark ? "rgba(255,255,255,0.05)" : "rgba(0,0,0,0.05)",
    };
  }
  // distribution = grouped gainers/losers counts; performance = avg % change.
  const chartViews = {
    distribution: {
      datasets: [
        { label: "Gainers", data: [28, 52, 18, 11], backgroundColor: "#10b981", borderRadius: 8, barPercentage: 0.6 },
        { label: "Losers", data: [14, 33, 6, 7], backgroundColor: "#ef4444", borderRadius: 8, barPercentage: 0.6 },
      ],
    },
    performance: {
      datasets: [
        { label: "Avg Gain %", data: [4.2, 7.8, 3.1, 2.4], backgroundColor: "#10b981", borderRadius: 8, barPercentage: 0.6 },
        { label: "Avg Loss %", data: [-1.6, -2.1, -1.2, -0.9], backgroundColor: "#ef4444", borderRadius: 8, barPercentage: 0.6 },
      ],
    },
  };
  function initMarketChart(view) {
    const canvas = document.getElementById("tgMarketChart");
    if (!canvas || typeof Chart === "undefined") return;
    const c = chartTickColors();
    if (marketChart) marketChart.destroy();
    marketChart = new Chart(canvas.getContext("2d"), {
      type: "bar",
      data: { labels: ["Forex", "Crypto", "Commodities", "Indices"], datasets: chartViews[view].datasets },
      options: {
        responsive: true,
        maintainAspectRatio: false,
        plugins: {
          legend: {
            position: "top",
            align: "end",
            labels: { color: c.tick, usePointStyle: true, pointStyle: "circle", padding: 20 },
          },
        },
        scales: {
          y: { beginAtZero: true, grid: { color: c.grid }, ticks: { color: c.tick } },
          x: { grid: { display: false }, ticks: { color: c.tick } },
        },
      },
    });
  }
  let currentChartView = "distribution";

  /* 06. Detail drawer chart -------------------------------------------------- */
  let detailChart;
  function renderDetailChart(isGainer) {
    const canvas = document.getElementById("tgDetailChart");
    if (!canvas || typeof Chart === "undefined") return;
    if (detailChart) detailChart.destroy();
    const c = chartTickColors();
    const color = isGainer ? "#10b981" : "#ef4444";
    detailChart = new Chart(canvas.getContext("2d"), {
      type: "line",
      data: {
        labels: Array.from({ length: 24 }, (_, i) => `${i}:00`),
        datasets: [
          {
            data: sparkData(isGainer),
            borderColor: color,
            backgroundColor: color + "20",
            fill: true,
            tension: 0.4,
            borderWidth: 2,
            pointRadius: 0,
          },
        ],
      },
      options: {
        responsive: true,
        maintainAspectRatio: false,
        plugins: { legend: { display: false } },
        scales: {
          x: { grid: { display: false }, ticks: { color: c.tick, maxTicksLimit: 6 } },
          y: { grid: { color: c.grid }, ticks: { color: c.tick } },
        },
      },
    });
  }

  /* 07. Market tabs (filter rows by type) ----------------------------------- */
  const tabs = document.querySelectorAll(".tg-tab");
  let currentMarket = "all";
  function setTabActive(btn) {
    tabs.forEach((b) => {
      b.classList.remove("text-white", "bg-accent");
      b.classList.add("text-muted", "hover:text-text");
    });
    btn.classList.add("text-white", "bg-accent");
    btn.classList.remove("text-muted", "hover:text-text");
  }
  function applyMarketFilter() {
    const q = (document.getElementById("tgSearch").value || "").toLowerCase();
    document.querySelectorAll(".tg-row").forEach((row) => {
      const matchMarket = currentMarket === "all" || row.dataset.type === currentMarket;
      const matchSearch =
        !q ||
        row.dataset.symbol.toLowerCase().includes(q) ||
        row.dataset.name.toLowerCase().includes(q);
      row.classList.toggle("hidden", !(matchMarket && matchSearch));
    });
  }
  tabs.forEach((btn) => {
    btn.addEventListener("click", function () {
      setTabActive(this);
      currentMarket = this.dataset.tab;
      applyMarketFilter();
      showToast("Filter Applied", `Showing ${currentMarket === "all" ? "all markets" : currentMarket} data`);
    });
  });

  /* 08. Chip groups (timeframe / view / chart / strength) -------------------- */
  function chipActivate(group, btn, activeCls, idleCls) {
    document.querySelectorAll(group).forEach((b) => {
      b.classList.remove(...activeCls);
      b.classList.add(...idleCls);
    });
    btn.classList.add(...activeCls);
    btn.classList.remove(...idleCls);
  }
  const pillActive = ["border-accent/30", "bg-accent/10", "text-accent"];
  const pillIdle = ["border-border", "bg-panel", "text-muted", "hover:text-text"];

  document.querySelectorAll(".tg-timeframe").forEach((chip) => {
    chip.addEventListener("click", function () {
      chipActivate(".tg-timeframe", this, pillActive, pillIdle);
      showToast("Timeframe Changed", `Showing ${this.dataset.timeframe} data`);
    });
  });

  document.querySelectorAll(".tg-view").forEach((btn) => {
    btn.addEventListener("click", function () {
      document.querySelectorAll(".tg-view").forEach((b) => {
        b.classList.remove("text-accent", "bg-accent/10");
        b.classList.add("text-muted", "hover:text-text");
      });
      this.classList.add("text-accent", "bg-accent/10");
      this.classList.remove("text-muted", "hover:text-text");
      const view = this.dataset.view;
      if (view === "gainers") {
        gainersCard.classList.remove("hidden");
        losersCard.classList.add("hidden");
        tablesGrid.classList.remove("xl:grid-cols-2");
      } else if (view === "losers") {
        gainersCard.classList.add("hidden");
        losersCard.classList.remove("hidden");
        tablesGrid.classList.remove("xl:grid-cols-2");
      } else {
        gainersCard.classList.remove("hidden");
        losersCard.classList.remove("hidden");
        tablesGrid.classList.add("xl:grid-cols-2");
      }
    });
  });

  document.querySelectorAll(".tg-chart").forEach((chip) => {
    chip.addEventListener("click", function () {
      chipActivate(".tg-chart", this, pillActive, pillIdle);
      currentChartView = this.dataset.chart;
      initMarketChart(currentChartView);
    });
  });

  document.querySelectorAll(".tg-strength").forEach((chip) => {
    chip.addEventListener("click", function () {
      chipActivate(".tg-strength", this, pillActive, pillIdle);
    });
  });

  /* 09. Export dropdown ------------------------------------------------------ */
  const exportBtn = document.getElementById("tgExportBtn");
  const exportMenu = document.getElementById("tgExportMenu");
  exportBtn.addEventListener("click", (e) => {
    e.stopPropagation();
    exportMenu.classList.toggle("hidden");
  });
  document.addEventListener("click", () => exportMenu.classList.add("hidden"));
  document.querySelectorAll(".tg-export-item").forEach((item) => {
    item.addEventListener("click", function () {
      const fmt = this.dataset.format.toUpperCase();
      exportMenu.classList.add("hidden");
      showToast("Export Started", `Exporting data as ${fmt}...`);
      setTimeout(() => showToast("Export Complete", `Data exported successfully as ${fmt}`), 1500);
    });
  });

  /* 10. Search + sort -------------------------------------------------------- */
  document.getElementById("tgSearch").addEventListener("input", applyMarketFilter);

  function parseVol(v) {
    const num = parseFloat(v);
    if (/B/i.test(v)) return num * 1e9;
    if (/M/i.test(v)) return num * 1e6;
    if (/K/i.test(v)) return num * 1e3;
    return num;
  }
  function sortBody(body, sortBy, isGainer) {
    const rows = Array.from(body.querySelectorAll(".tg-row"));
    rows.sort((a, b) => {
      const ca = parseFloat(a.dataset.change), cb = parseFloat(b.dataset.change);
      const pa = parseFloat(a.dataset.price), pb = parseFloat(b.dataset.price);
      const va = parseVol(a.dataset.vol), vb = parseVol(b.dataset.vol);
      switch (sortBy) {
        case "change_desc": return isGainer ? cb - ca : ca - cb;
        case "change_asc": return isGainer ? ca - cb : cb - ca;
        case "volume_desc": return vb - va;
        case "volume_asc": return va - vb;
        case "price_desc": return pb - pa;
        case "price_asc": return pa - pb;
        default: return 0;
      }
    });
    rows.forEach((r) => body.appendChild(r));
  }
  document.getElementById("tgSort").addEventListener("change", function () {
    sortBody(gainersBody, this.value, true);
    sortBody(losersBody, this.value, false);
  });

  /* 11. Asset detail open ---------------------------------------------------- */
  const typeBadgeCls = {
    crypto: "bg-violet-500/15 text-violet-500",
    forex: "bg-sky-500/15 text-sky-500",
    commodity: "bg-amber-500/15 text-amber-500",
    index: "bg-teal-500/15 text-teal-500",
  };
  const typeGrad = {
    crypto: ["from-violet-500", "to-purple-500"],
    forex: ["from-sky-500", "to-blue-500"],
    commodity: ["from-amber-500", "to-orange-500"],
    index: ["from-teal-500", "to-cyan-500"],
  };
  function fmtPrice(p) {
    if (p < 0.01) return "$" + p.toFixed(8);
    if (p < 1) return "$" + p.toFixed(4);
    if (p < 10) return "$" + p.toFixed(3);
    return "$" + p.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 });
  }
  function openAssetDetail(row) {
    const d = row.dataset;
    const isGainer = d.kind === "gainer";
    const price = parseFloat(d.price);
    const change = parseFloat(d.change);

    document.getElementById("tgAssetTitle").textContent = `${d.symbol} Details`;

    const icon = document.getElementById("tgDetailIcon");
    icon.textContent = d.symbol.substring(0, 2);
    icon.className = "w-16 h-16 rounded-xl flex items-center justify-center text-white font-bold text-xl shrink-0 bg-gradient-to-br " + (typeGrad[d.type] || typeGrad.crypto).join(" ");

    document.getElementById("tgDetailSymbol").textContent = d.symbol;
    document.getElementById("tgDetailName").textContent = d.name;

    const typeEl = document.getElementById("tgDetailType");
    typeEl.textContent = d.type;
    typeEl.className = "inline-block mt-1 px-2 py-1 rounded-md text-[10px] font-bold uppercase tracking-wide " + (typeBadgeCls[d.type] || typeBadgeCls.crypto);

    document.getElementById("tgDetailPrice").textContent = fmtPrice(price);
    const chEl = document.getElementById("tgDetailChange");
    chEl.textContent = `${isGainer ? "+" : ""}${change.toFixed(2)}%`;
    chEl.className = "text-lg font-bold " + (isGainer ? "text-emerald-500" : "text-red-500");

    document.getElementById("tgDetailHigh").textContent = fmtPrice(price * 1.05);
    document.getElementById("tgDetailLow").textContent = fmtPrice(price * 0.95);
    document.getElementById("tgDetailVolume").textContent = "$" + d.vol;
    document.getElementById("tgDetailMcap").textContent = "$" + (Math.random() * 100).toFixed(2) + "B";

    document.getElementById("tgDetailAnalysis").textContent = isGainer
      ? "Strong bullish momentum detected. RSI indicates overbought conditions but trend remains strong. Consider trailing stop-loss."
      : "Bearish pressure continuing. Support level at current price may hold. Watch for reversal signals.";

    const sig = document.getElementById("tgDetailSignal");
    sig.textContent = isGainer ? "Bullish Signal" : "Bearish Signal";
    sig.className = "px-2.5 py-1 rounded-lg text-xs font-semibold " + (isGainer ? "bg-emerald-500/15 text-emerald-500" : "bg-red-500/15 text-red-500");
    document.getElementById("tgDetailConf").textContent = `Confidence: ${(75 + Math.random() * 20).toFixed(0)}%`;

    document.getElementById("tgDetailAlert").dataset.symbol = d.symbol;
    document.getElementById("tgDetailWatch").dataset.symbol = d.symbol;

    openDrawer(assetDrawer);
    refreshIcons();
    requestAnimationFrame(() => renderDetailChart(isGainer));
  }

  document.querySelectorAll(".tg-row").forEach((row) => {
    row.addEventListener("click", () => openAssetDetail(row));
  });

  /* 12. Watchlist / filters / refresh / view-all ---------------------------- */
  function addToWatchlist(symbol) {
    showToast("Added to Watchlist", `${symbol} has been added to your watchlist`);
  }
  document.querySelectorAll(".tg-watch").forEach((btn) => {
    btn.addEventListener("click", function (e) {
      e.stopPropagation();
      addToWatchlist(this.dataset.symbol);
    });
  });

  document.getElementById("tgFilterBtn").addEventListener("click", () => openDrawer(filterDrawer));

  document.getElementById("tgApplyFilters").addEventListener("click", () => {
    closeDrawers();
    showToast("Filters Applied", "Data filtered successfully");
  });

  document.getElementById("tgResetFilters").addEventListener("click", () => {
    ["tgFMinChange", "tgFMaxChange", "tgFVolume", "tgFMinPrice", "tgFMaxPrice"].forEach((id) => {
      document.getElementById(id).value = "";
    });
    ["tgFForex", "tgFCrypto", "tgFCommodities", "tgFIndices"].forEach((id) => {
      document.getElementById(id).checked = true;
    });
    showToast("Filters Reset", "All filters have been cleared");
  });

  document.getElementById("tgDetailAlert").addEventListener("click", function () {
    showToast("Alert Set", `Price alert configured for ${this.dataset.symbol}`);
    closeDrawers();
  });
  document.getElementById("tgDetailWatch").addEventListener("click", function () {
    addToWatchlist(this.dataset.symbol);
  });

  document.getElementById("tgRefresh").addEventListener("click", () => {
    showToast("Refreshing", "Updating market data...");
    document.getElementById("tgLastUpdated").textContent = "Just now";
    setTimeout(() => {
      renderSparklines();
      showToast("Data Updated", "Market data refreshed successfully");
    }, 1000);
  });

  document.querySelectorAll(".tg-view-all").forEach((btn) => {
    btn.addEventListener("click", () => {
      const which = btn.dataset.viewAll;
      showToast("View All", `Opening full ${which} list...`);
    });
  });

  /* 13. Keyboard shortcuts --------------------------------------------------- */
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      closeDrawers();
      exportMenu.classList.add("hidden");
    }
    if ((e.ctrlKey || e.metaKey) && e.key === "k") {
      e.preventDefault();
      document.getElementById("globalSearch")?.focus();
    }
  });

  /* Theme re-color */
  document.getElementById("themeToggle")?.addEventListener("click", () => {
    setTimeout(() => {
      if (marketChart) {
        const c = chartTickColors();
        marketChart.options.scales.y.grid.color = c.grid;
        marketChart.options.scales.y.ticks.color = c.tick;
        marketChart.options.scales.x.ticks.color = c.tick;
        marketChart.options.plugins.legend.labels.color = c.tick;
        marketChart.update();
      }
    }, 50);
  });

  /* 14. Init ----------------------------------------------------------------- */
  renderSparklines();
  initMarketChart(currentChartView);
});
