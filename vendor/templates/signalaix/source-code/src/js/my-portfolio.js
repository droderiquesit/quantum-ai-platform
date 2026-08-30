/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: my-portfolio.html
 * description: SignalAIX - My Portfolio Page Controller
 *              Self-contained; mirrors the reference mockup's functionality
 *              (asset-type tabs, profit/loss filter chips, holdings search,
 *               add-asset / trade / details / filter / watchlist drawers,
 *               export menu, toast, allocation doughnut, performance line +
 *               period switcher, risk gauge, 6 mini sparkline charts, plus a
 *               real per-open price chart in the details drawer).
 *              All markup lives in my-portfolio.html; this file only modifies
 *              the DOM (text/values/classes) and renders charts — it never
 *              injects HTML strings.
 * author: SignalAIX

    -------------------------------------------------
     01. Init & DOM refs (guarded by #my-portfolio)
     -------------------------------------------------
     02. Toast
     -------------------------------------------------
     03. Drawers (add / trade / details / filter / watchlist)
     -------------------------------------------------
     04. Asset tabs + filter chips + search (row filtering)
     -------------------------------------------------
     05. Trade drawer (side select / % buttons / estimate)
     -------------------------------------------------
     06. Details drawer populate + price chart
     -------------------------------------------------
     07. Add-asset type select + confirms
     -------------------------------------------------
     08. Export menu, global search
     -------------------------------------------------
     09. Charts (allocation / performance / risk gauge / mini sparklines)
     -------------------------------------------------
     10. Theme re-color + keyboard shortcuts
     -------------------------------------------------
    ================================================== */

document.addEventListener("DOMContentLoaded", () => {
  /* ======================================
   * 01. Init & DOM refs
   * ====================================== */
  const page = document.getElementById("my-portfolio");
  if (!page) return; // Guard: only run on the My Portfolio page

  const html = document.documentElement;
  const refreshIcons = () => window.lucide?.createIcons?.();

  const overlay = document.getElementById("mpDrawerOverlay");
  const addDrawer = document.getElementById("mpAddDrawer");
  const tradeDrawer = document.getElementById("mpTradeDrawer");
  const detailsDrawer = document.getElementById("mpDetailsDrawer");
  const filterDrawer = document.getElementById("mpFilterDrawer");
  const watchlistDrawer = document.getElementById("mpWatchlistDrawer");
  const allDrawers = [addDrawer, tradeDrawer, detailsDrawer, filterDrawer, watchlistDrawer];

  const toast = document.getElementById("mpToast");
  const toastTitle = document.getElementById("mpToastTitle");
  const toastMessage = document.getElementById("mpToastMessage");

  const rows = Array.from(document.querySelectorAll(".mp-row"));
  const noResults = document.getElementById("mpNoResults");

  // Per-asset metadata mirroring the mockup's table values.
  const ASSETS = {
    BTC: {
      name: "Bitcoin", badge: "BTC", value: "$53,421.50", holdings: "1.245 BTC",
      avg: "$38,500.00", current: "$42,923.15", pl: "+$5,526.73", plPct: "+11.49%",
      up: true, series: [41000, 41500, 42000, 41800, 42500, 42200, 42923],
    },
    ETH: {
      name: "Ethereum", badge: "ETH", value: "$28,125.00", holdings: "12.5 ETH",
      avg: "$2,100.00", current: "$2,250.00", pl: "+$1,875.00", plPct: "+7.14%",
      up: true, series: [2180, 2200, 2220, 2190, 2230, 2240, 2250],
    },
    EURUSD: {
      name: "EUR/USD", badge: "€/$", value: "$12,500.00", holdings: "50,000 Units",
      avg: "1.0920", current: "1.0875", pl: "-$225.00", plPct: "-0.41%",
      up: false, series: [1.092, 1.091, 1.0895, 1.0905, 1.088, 1.087, 1.0875],
    },
    GBPJPY: {
      name: "GBP/JPY", badge: "£/¥", value: "$8,750.00", holdings: "30,000 Units",
      avg: "186.50", current: "188.25", pl: "+$412.50", plPct: "+0.94%",
      up: true, series: [186.5, 187.0, 187.5, 187.2, 188.0, 188.1, 188.25],
    },
    SOL: {
      name: "Solana", badge: "SOL", value: "$8,925.00", holdings: "85 SOL",
      avg: "$95.00", current: "$105.00", pl: "+$850.00", plPct: "+10.53%",
      up: true, series: [98, 100, 102, 101, 103, 104, 105],
    },
    GOLD: {
      name: "Gold", badge: "XAU", value: "$10,125.00", holdings: "5 oz",
      avg: "$1,985.00", current: "$2,025.00", pl: "+$200.00", plPct: "+2.01%",
      up: true, series: [2010, 2015, 2020, 2018, 2022, 2023, 2025],
    },
  };

  // Current filter state shared by tabs + chips + search
  let activeTab = "all-assets"; // all-assets | forex | crypto | watchlist
  let activeFilter = "all"; // all | profit | loss
  let searchTerm = "";

  /* ======================================
   * 02. Toast
   * ====================================== */
  let toastTimer = null;
  function showToast(title, message) {
    if (toastTitle) toastTitle.textContent = title;
    if (toastMessage) toastMessage.textContent = message;
    toast?.classList.add("active");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(hideToast, 4000);
  }
  function hideToast() {
    toast?.classList.remove("active");
  }
  document.querySelector(".mp-toast-close")?.addEventListener("click", hideToast);

  /* ======================================
   * 03. Drawers
   * ====================================== */
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
  document.querySelectorAll(".mp-close-drawer").forEach((btn) =>
    btn.addEventListener("click", closeDrawers)
  );
  document.querySelectorAll(".mp-open-add").forEach((btn) =>
    btn.addEventListener("click", () => openDrawer(addDrawer))
  );
  document.querySelectorAll(".mp-open-filter").forEach((btn) =>
    btn.addEventListener("click", () => openDrawer(filterDrawer))
  );
  document.querySelectorAll(".mp-open-watchlist").forEach((btn) =>
    btn.addEventListener("click", () => openDrawer(watchlistDrawer))
  );

  /* ======================================
   * 04. Asset tabs + filter chips + search
   * ====================================== */
  const panes = Array.from(document.querySelectorAll(".mp-pane"));

  function showPane(name) {
    panes.forEach((p) => p.classList.toggle("hidden", p.dataset.pane !== name));
  }

  function applyRowFilters() {
    let visible = 0;
    rows.forEach((row) => {
      const type = row.dataset.type;
      const isProfit = row.dataset.profit === "true";
      const text = row.textContent.toLowerCase();

      const filterMatch =
        activeFilter === "all" ||
        (activeFilter === "profit" && isProfit) ||
        (activeFilter === "loss" && !isProfit);
      const searchMatch = !searchTerm || text.includes(searchTerm);

      const show = filterMatch && searchMatch;
      row.style.display = show ? "" : "none";
      if (show) visible += 1;
      void type;
    });
    if (noResults) noResults.classList.toggle("hidden", visible !== 0);
  }

  // Asset tabs (swap visible pane)
  document.querySelectorAll(".mp-tab").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll(".mp-tab").forEach((b) => {
        b.classList.remove("active");
        b.classList.add("text-muted");
      });
      btn.classList.add("active");
      btn.classList.remove("text-muted");
      activeTab = btn.dataset.tab;
      showPane(activeTab);
    });
  });

  // Filter chips
  document.querySelectorAll(".mp-chip").forEach((chip) => {
    chip.addEventListener("click", () => {
      document.querySelectorAll(".mp-chip").forEach((c) => {
        c.classList.remove("active", "border-accent", "bg-accent/15", "text-accent");
        c.classList.add("border-border", "bg-bg", "text-muted");
      });
      chip.classList.add("active", "border-accent", "bg-accent/15", "text-accent");
      chip.classList.remove("border-border", "bg-bg", "text-muted");
      activeFilter = chip.dataset.filter;
      applyRowFilters();
    });
  });

  // Holdings search
  document.getElementById("mpAssetSearch")?.addEventListener("input", (e) => {
    searchTerm = e.target.value.toLowerCase();
    applyRowFilters();
  });

  // Filter drawer apply
  document.querySelector(".mp-apply-filters")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Filters Applied", "Portfolio view has been updated");
  });

  /* ======================================
   * 05. Trade drawer (side / % / estimate)
   * ====================================== */
  const tradeTitle = document.getElementById("mpTradeTitle");
  const tradePair = document.getElementById("mpTradePair");
  const tradeBadge = document.getElementById("mpTradeBadge");
  const tradePrice = document.getElementById("mpTradePrice");
  const tradeAmount = document.getElementById("mpTradeAmount");
  const tradeTotal = document.getElementById("mpTradeTotal");
  const tradeFee = document.getElementById("mpTradeFee");
  let currentTradeAsset = "BTC";

  function openTrade(key) {
    const a = ASSETS[key] || ASSETS.BTC;
    currentTradeAsset = key;
    if (tradeTitle) tradeTitle.textContent = a.name;
    if (tradePair) tradePair.textContent = a.name;
    if (tradeBadge) tradeBadge.textContent = a.badge;
    if (tradePrice) tradePrice.textContent = a.current;
    if (tradeAmount) tradeAmount.value = "";
    if (tradeTotal) tradeTotal.textContent = "$0.00";
    if (tradeFee) tradeFee.textContent = "$0.00";
    openDrawer(tradeDrawer);
  }

  document.querySelectorAll(".mp-open-trade").forEach((btn) => {
    btn.addEventListener("click", () => openTrade(btn.dataset.asset));
  });

  // Buy / Sell toggle
  document.querySelectorAll(".mp-trade-side").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll(".mp-trade-side").forEach((b) => {
        b.classList.remove(
          "active", "border-emerald-500", "bg-emerald-500/10", "text-emerald-500",
          "border-red-500", "bg-red-500/10", "text-red-500"
        );
        b.classList.add("border-border", "bg-bg", "text-muted");
      });
      btn.classList.remove("border-border", "bg-bg", "text-muted");
      btn.classList.add("active");
      if (btn.dataset.side === "buy") {
        btn.classList.add("border-emerald-500", "bg-emerald-500/10", "text-emerald-500");
      } else {
        btn.classList.add("border-red-500", "bg-red-500/10", "text-red-500");
      }
    });
  });

  function recalcTrade() {
    const a = ASSETS[currentTradeAsset] || ASSETS.BTC;
    const unitPrice = parseFloat(String(a.current).replace(/[^0-9.]/g, "")) || 0;
    const amt = parseFloat(tradeAmount?.value) || 0;
    const total = unitPrice * amt;
    const fee = total * 0.001;
    if (tradeTotal) tradeTotal.textContent = "$" + total.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 });
    if (tradeFee) tradeFee.textContent = "$" + fee.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 });
  }
  tradeAmount?.addEventListener("input", recalcTrade);

  // % quick-amount buttons (relative to held units)
  document.querySelectorAll(".mp-trade-pct").forEach((btn) => {
    btn.addEventListener("click", () => {
      const a = ASSETS[currentTradeAsset] || ASSETS.BTC;
      const held = parseFloat(String(a.holdings).replace(/[^0-9.]/g, "")) || 0;
      const pct = parseFloat(btn.dataset.pct) || 0;
      if (tradeAmount) tradeAmount.value = ((held * pct) / 100).toString();
      recalcTrade();
    });
  });

  document.querySelector(".mp-trade-confirm")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Trade Submitted", `Your ${currentTradeAsset} order has been placed`);
  });

  /* ======================================
   * 06. Details drawer populate + price chart
   * ====================================== */
  const dTitle = document.getElementById("mpDetailsTitle");
  const dBadge = document.getElementById("mpDetailsBadge");
  const dName = document.getElementById("mpDetailsName");
  const dValue = document.getElementById("mpDetailsValue");
  const dHoldings = document.getElementById("mpDetailsHoldings");
  const dAvg = document.getElementById("mpDetailsAvg");
  const dCurrent = document.getElementById("mpDetailsCurrent");
  const dPl = document.getElementById("mpDetailsPl");
  const dPlBadge = document.getElementById("mpDetailsPlBadge");
  const dPlBox = document.getElementById("mpDetailsPlBox");
  const txAssets = document.querySelectorAll(".mp-tx-asset");

  let detailsChart = null;
  let currentDetailsAsset = "BTC";

  function openDetails(key) {
    const a = ASSETS[key] || ASSETS.BTC;
    currentDetailsAsset = key;
    if (dTitle) dTitle.textContent = a.name;
    if (dBadge) dBadge.textContent = a.badge;
    if (dName) dName.textContent = a.name;
    if (dValue) dValue.textContent = a.value;
    if (dHoldings) dHoldings.textContent = a.holdings;
    if (dAvg) dAvg.textContent = a.avg;
    if (dCurrent) dCurrent.textContent = a.current;
    if (dPl) dPl.textContent = a.pl;
    if (dPlBadge) dPlBadge.textContent = a.plPct;
    txAssets.forEach((el) => (el.textContent = a.badge));

    // P/L box + badge color by direction
    if (dPlBox) {
      dPlBox.classList.remove(
        "bg-emerald-500/10", "border-emerald-500/20",
        "bg-red-500/10", "border-red-500/20"
      );
      dPlBox.classList.add(
        a.up ? "bg-emerald-500/10" : "bg-red-500/10",
        a.up ? "border-emerald-500/20" : "border-red-500/20"
      );
    }
    if (dPl) {
      dPl.classList.remove("text-emerald-500", "text-red-500");
      dPl.classList.add(a.up ? "text-emerald-500" : "text-red-500");
    }
    if (dPlBadge) {
      dPlBadge.classList.remove(
        "bg-emerald-500/15", "text-emerald-500",
        "bg-red-500/15", "text-red-500"
      );
      dPlBadge.classList.add(
        a.up ? "bg-emerald-500/15" : "bg-red-500/15",
        a.up ? "text-emerald-500" : "text-red-500"
      );
    }

    openDrawer(detailsDrawer);
    renderDetailsChart(a);
  }

  function renderDetailsChart(a) {
    const canvas = document.getElementById("mpDetailsChart");
    if (!canvas || typeof Chart === "undefined") return;
    if (detailsChart) {
      detailsChart.destroy();
      detailsChart = null;
    }
    requestAnimationFrame(() => {
      const color = a.up ? "#10b981" : "#ef4444";
      const ctx = canvas.getContext("2d");
      detailsChart = new Chart(ctx, {
        type: "line",
        data: {
          labels: a.series.map((_, i) => i + 1),
          datasets: [
            {
              data: a.series,
              borderColor: color,
              backgroundColor: color + "22",
              borderWidth: 2,
              fill: true,
              tension: 0.4,
              pointRadius: 0,
            },
          ],
        },
        options: {
          responsive: true,
          maintainAspectRatio: false,
          plugins: { legend: { display: false } },
          scales: {
            y: { display: false, grid: { display: false } },
            x: { display: false, grid: { display: false } },
          },
        },
      });
    });
  }

  document.querySelectorAll(".mp-open-details").forEach((btn) => {
    btn.addEventListener("click", () => openDetails(btn.dataset.asset));
  });
  document.querySelector(".mp-details-trade")?.addEventListener("click", () =>
    openTrade(currentDetailsAsset)
  );
  document.querySelector(".mp-details-alert")?.addEventListener("click", () =>
    showToast("Alert", `Price alert set for ${currentDetailsAsset}`)
  );

  /* ======================================
   * 07. Add-asset type select + confirms
   * ====================================== */
  document.querySelectorAll(".mp-add-type").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll(".mp-add-type").forEach((b) => {
        b.classList.remove("active", "border-accent", "bg-accent/10", "text-accent");
        b.classList.add("border-border", "bg-bg", "text-muted");
      });
      btn.classList.add("active", "border-accent", "bg-accent/10", "text-accent");
      btn.classList.remove("border-border", "bg-bg", "text-muted");
    });
  });
  document.querySelector(".mp-add-confirm")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Asset Added", "New asset has been added to your portfolio");
  });
  document.querySelector(".mp-watchlist-confirm")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Watchlist Updated", "Selected assets added to your watchlist");
  });

  /* ======================================
   * 08. Export menu, global search
   * ====================================== */
  const exportBtn = document.getElementById("mpExportBtn");
  const exportMenu = document.getElementById("mpExportMenu");
  exportBtn?.addEventListener("click", (e) => {
    e.stopPropagation();
    exportMenu?.classList.toggle("hidden");
  });
  document.addEventListener("click", () => exportMenu?.classList.add("hidden"));
  document.querySelectorAll(".mp-export-item").forEach((item) => {
    item.addEventListener("click", () => {
      const fmt = (item.dataset.format || "csv").toUpperCase();
      exportMenu?.classList.add("hidden");
      showToast("Export Started", `Exporting portfolio as ${fmt}...`);
      setTimeout(
        () => showToast("Export Complete", `Portfolio exported as ${fmt} successfully!`),
        2000
      );
    });
  });

  // Global search filters the holdings table too (and keeps the All Assets pane visible)
  document.getElementById("mpGlobalSearch")?.addEventListener("input", (e) => {
    searchTerm = e.target.value.toLowerCase();
    applyRowFilters();
  });

  /* ======================================
   * 09. Charts
   * ====================================== */
  let allocationChart = null;
  let performanceChart = null;
  let riskGauge = null;
  const miniCharts = {};

  function chartColors() {
    const isDark = html.classList.contains("dark");
    return {
      grid: isDark ? "rgba(255,255,255,0.05)" : "rgba(0,0,0,0.05)",
      text: isDark ? "#94A3B8" : "#64748B",
      gaugeTrack: isDark ? "#1E293B" : "#E2E8F0",
      point: isDark ? "#0C0F16" : "#FFFFFF",
    };
  }

  const PERF_DATA = {
    "1W": { labels: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"], data: [120000, 122500, 121000, 125000, 124000, 126500, 127845] },
    "1M": { labels: ["W1", "W2", "W3", "W4"], data: [113500, 118200, 122900, 127845] },
    "3M": { labels: ["Apr", "May", "Jun"], data: [101200, 114600, 127845] },
    "1Y": { labels: ["Q1", "Q2", "Q3", "Q4"], data: [78400, 95300, 110200, 127845] },
  };

  function initCharts() {
    if (typeof Chart === "undefined") return;
    const c = chartColors();

    // Allocation doughnut
    const allocEl = document.getElementById("mpAllocationChart");
    if (allocEl) {
      allocationChart = new Chart(allocEl.getContext("2d"), {
        type: "doughnut",
        data: {
          labels: ["Bitcoin", "Ethereum", "Forex", "Gold", "Others"],
          datasets: [
            {
              data: [42, 22, 17, 8, 11],
              backgroundColor: ["#f59e0b", "#6366f1", "#0ea5e9", "#eab308", "#8b5cf6"],
              borderWidth: 0,
              hoverOffset: 10,
            },
          ],
        },
        options: {
          responsive: true,
          maintainAspectRatio: false,
          plugins: {
            legend: { display: false },
            tooltip: {
              callbacks: { label: (ctx) => ctx.label + ": " + ctx.parsed + "%" },
            },
          },
          cutout: "70%",
        },
      });
    }

    // Performance line
    const perfEl = document.getElementById("mpPerformanceChart");
    if (perfEl) {
      const pctx = perfEl.getContext("2d");
      const gradient = pctx.createLinearGradient(0, 0, 0, 200);
      gradient.addColorStop(0, "rgba(16,185,129,0.3)");
      gradient.addColorStop(1, "rgba(16,185,129,0)");
      const d = PERF_DATA["1W"];
      performanceChart = new Chart(pctx, {
        type: "line",
        data: {
          labels: d.labels,
          datasets: [
            {
              label: "Portfolio Value",
              data: d.data,
              borderColor: "#10b981",
              backgroundColor: gradient,
              borderWidth: 3,
              fill: true,
              tension: 0.4,
              pointRadius: 0,
              pointHoverRadius: 6,
              pointBackgroundColor: "#10b981",
              pointBorderColor: c.point,
              pointBorderWidth: 3,
            },
          ],
        },
        options: {
          responsive: true,
          maintainAspectRatio: false,
          plugins: {
            legend: { display: false },
            tooltip: { callbacks: { label: (ctx) => "$" + ctx.parsed.y.toLocaleString() } },
          },
          scales: {
            y: { display: false },
            x: { grid: { display: false }, ticks: { color: c.text } },
          },
          interaction: { intersect: false, mode: "index" },
        },
      });
    }

    // Risk gauge (semicircle)
    const riskEl = document.getElementById("mpRiskGauge");
    if (riskEl) {
      riskGauge = new Chart(riskEl.getContext("2d"), {
        type: "doughnut",
        data: {
          datasets: [
            { data: [55, 45], backgroundColor: ["#8b5cf6", c.gaugeTrack], borderWidth: 0 },
          ],
        },
        options: {
          responsive: true,
          maintainAspectRatio: false,
          plugins: { legend: { display: false }, tooltip: { enabled: false } },
          cutout: "75%",
          rotation: -90,
          circumference: 180,
        },
      });
    }

    // Mini sparklines
    const miniConfigs = {
      mpChartBtc: { key: "BTC" },
      mpChartEth: { key: "ETH" },
      mpChartEurusd: { key: "EURUSD" },
      mpChartGbpjpy: { key: "GBPJPY" },
      mpChartSol: { key: "SOL" },
      mpChartGold: { key: "GOLD" },
    };
    Object.keys(miniConfigs).forEach((id) => {
      const el = document.getElementById(id);
      if (!el) return;
      const a = ASSETS[miniConfigs[id].key];
      const color = a.up ? "#10b981" : "#ef4444";
      miniCharts[id] = new Chart(el.getContext("2d"), {
        type: "line",
        data: {
          labels: a.series.map((_, i) => i),
          datasets: [
            { data: a.series, borderColor: color, borderWidth: 2, fill: false, tension: 0.4, pointRadius: 0 },
          ],
        },
        options: {
          responsive: true,
          maintainAspectRatio: false,
          plugins: { legend: { display: false }, tooltip: { enabled: false } },
          scales: { x: { display: false }, y: { display: false } },
        },
      });
    });
  }

  initCharts();

  // Performance period switcher
  document.querySelectorAll(".mp-period").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll(".mp-period").forEach((b) => {
        b.classList.remove("active", "bg-panel", "text-text");
        b.classList.add("text-muted");
      });
      btn.classList.add("active", "bg-panel", "text-text");
      btn.classList.remove("text-muted");
      const d = PERF_DATA[btn.dataset.period] || PERF_DATA["1W"];
      if (performanceChart) {
        performanceChart.data.labels = d.labels;
        performanceChart.data.datasets[0].data = d.data;
        performanceChart.update();
      }
    });
  });

  /* ======================================
   * 10. Theme re-color + keyboard shortcuts
   * ====================================== */
  function recolorCharts() {
    const c = chartColors();
    if (performanceChart) {
      performanceChart.options.scales.x.ticks.color = c.text;
      performanceChart.data.datasets[0].pointBorderColor = c.point;
      performanceChart.update();
    }
    if (riskGauge) {
      riskGauge.data.datasets[0].backgroundColor[1] = c.gaugeTrack;
      riskGauge.update();
    }
  }
  document.getElementById("themeToggle")?.addEventListener("click", () => {
    setTimeout(recolorCharts, 50);
  });

  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      closeDrawers();
      exportMenu?.classList.add("hidden");
    }
    if ((e.ctrlKey || e.metaKey) && e.key === "k") {
      e.preventDefault();
      document.getElementById("mpGlobalSearch")?.focus();
    }
  });
});
