/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: index.html (Dashboard)
 * description: SignalAIX - Dashboard Controller, ported from
 *              SignalAIX/MAIN/dashboard/Dashboard_01.html (content + functionality).
 *              All markup lives in index.html; this file only modifies the DOM
 *              (text/values/classes, show/hide) — it never injects HTML strings.
 * author: SignalAIX

    -------------------------------------------------
     01. Init & guard (#dashboard)
     -------------------------------------------------
     02. Toast
     -------------------------------------------------
     03. Drawer (static panels, shown/hidden by data-panel)
     -------------------------------------------------
     04. Tabs, Filter pills, Export menu
     -------------------------------------------------
     05. Top Movers (gainers/losers)
     -------------------------------------------------
     06. Recent Trades table (search + status filter)
     -------------------------------------------------
     07. Quick-trade amount, Generate signal, Summary refresh
     -------------------------------------------------
     08. Performance chart (Chart.js) + period + theme
     -------------------------------------------------
    ================================================== */

document.addEventListener("DOMContentLoaded", () => {
  /**
   * ======================================
   * 01. Init & guard
   * ======================================
   */
  if (!document.getElementById("dashboard")) return;

  const drawer = document.getElementById("dashDrawer");
  const drawerOverlay = document.getElementById("dashDrawerOverlay");
  const drawerTitle = document.getElementById("dashDrawerTitle");
  const toast = document.getElementById("dashToast");
  const refreshIcons = () => window.lucide?.createIcons?.();

  /**
   * ======================================
   * 02. Toast
   * ======================================
   */
  let toastTimer;
  function showToast(title, message) {
    document.getElementById("dashToastTitle").textContent = title;
    document.getElementById("dashToastMessage").textContent = message;
    toast.classList.add("active");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => toast.classList.remove("active"), 3000);
  }
  document.getElementById("dashToastClose")?.addEventListener("click", () => toast.classList.remove("active"));

  /**
   * ======================================
   * 03. Drawer — show one static panel, hide the rest (no HTML in JS)
   * ======================================
   */
  const titles = {
    "generate-signal": "AI Signal Generator",
    "quick-trade": "Quick Trade",
    "signal-detail": "Signal Details",
    "trade-detail": "Trade Details",
  };
  function openDrawer(type) {
    drawerTitle.textContent = titles[type] || "Details";
    drawer.querySelectorAll(".dash-panel").forEach((p) => {
      p.classList.toggle("hidden", p.dataset.panel !== type);
    });
    drawer.classList.add("active");
    drawerOverlay.classList.add("active");
    refreshIcons();
  }
  function closeDrawer() {
    drawer.classList.remove("active");
    drawerOverlay.classList.remove("active");
  }
  document.querySelectorAll(".dash-open-drawer").forEach((el) => {
    el.addEventListener("click", () => openDrawer(el.dataset.drawer));
  });
  document.querySelectorAll(".dash-close-drawer").forEach((b) => b.addEventListener("click", closeDrawer));
  drawerOverlay.addEventListener("click", closeDrawer);

  /**
   * ======================================
   * 04. Tabs, Filter pills, Export menu
   * ======================================
   */
  // Tabs (market) and Buy/Sell pills filter the signal cards together (intersected).
  let currentTab = "signals"; // "signals"=all markets, else "forex"/"crypto"
  let currentSide = "all"; // "all" | "buy" | "sell"

  function applySignalFilters() {
    document.querySelectorAll(".dash-signal").forEach((sig) => {
      const marketOk =
        currentTab === "signals" ||
        currentTab === "all" ||
        sig.dataset.market === currentTab;
      const sideOk = currentSide === "all" || sig.classList.contains(currentSide);
      sig.style.display = marketOk && sideOk ? "" : "none";
    });
  }

  document.querySelectorAll(".dash-tab").forEach((tab) => {
    tab.addEventListener("click", () => {
      document.querySelectorAll(".dash-tab").forEach((t) => t.classList.remove("active"));
      tab.classList.add("active");
      currentTab = tab.dataset.tab;
      applySignalFilters();
    });
  });

  document.querySelectorAll(".dash-filter").forEach((pill) => {
    pill.addEventListener("click", () => {
      document.querySelectorAll(".dash-filter").forEach((p) => p.classList.remove("active"));
      pill.classList.add("active");
      currentSide = pill.dataset.filter;
      applySignalFilters();
    });
  });

  const exportBtn = document.getElementById("dashExportBtn");
  const exportMenu = exportBtn?.nextElementSibling;
  exportBtn?.addEventListener("click", (e) => {
    e.stopPropagation();
    exportMenu?.classList.toggle("active");
  });
  document.querySelectorAll(".dash-export").forEach((item) => {
    item.addEventListener("click", () => {
      exportMenu?.classList.remove("active");
      showToast("Export Started", `Exporting data as ${(item.dataset.format || "").toUpperCase()}...`);
    });
  });
  document.addEventListener("click", () => exportMenu?.classList.remove("active"));

  /**
   * ======================================
   * 05. Top Movers (gainers / losers) — toggle visibility of two static lists
   * ======================================
   */
  const gainersBtn = document.getElementById("dashMoversGainers");
  const losersBtn = document.getElementById("dashMoversLosers");
  const gainers = document.getElementById("dashGainers");
  const losers = document.getElementById("dashLosers");
  function setMovers(showGainers) {
    gainersBtn.classList.toggle("active", showGainers);
    losersBtn.classList.toggle("active", !showGainers);
    gainers.classList.toggle("hidden", !showGainers);
    losers.classList.toggle("hidden", showGainers);
  }
  gainersBtn?.addEventListener("click", () => setMovers(true));
  losersBtn?.addEventListener("click", () => setMovers(false));

  /**
   * ======================================
   * 06. Recent Trades table (search + status filter)
   * ======================================
   */
  const tableSearch = document.getElementById("dashTableSearch");
  const statusFilter = document.getElementById("dashStatusFilter");
  const rows = () => document.querySelectorAll("#dashTradesTable tbody tr");
  function applyTableFilters() {
    const term = (tableSearch?.value || "").toLowerCase();
    const status = statusFilter?.value || "";
    rows().forEach((row) => {
      const matchesTerm = row.textContent.toLowerCase().includes(term);
      const matchesStatus = !status || row.dataset.status === status;
      row.style.display = matchesTerm && matchesStatus ? "" : "none";
    });
  }
  tableSearch?.addEventListener("input", applyTableFilters);
  statusFilter?.addEventListener("change", applyTableFilters);

  /**
   * ======================================
   * 07. Quick-trade amount, Generate signal, Summary refresh
   * ======================================
   */
  document.querySelectorAll(".dash-amount").forEach((btn) => {
    btn.addEventListener("click", () => {
      const input = document.getElementById("dashAmount");
      if (input) input.value = btn.dataset.amount;
    });
  });
  document.querySelectorAll(".dash-pill").forEach((pill) => {
    pill.addEventListener("click", () => {
      pill.parentElement.querySelectorAll(".dash-pill").forEach((p) => p.classList.remove("active"));
      pill.classList.add("active");
    });
  });
  document.getElementById("dashGenerateSignal")?.addEventListener("click", () => {
    closeDrawer();
    showToast("Generating Signal", "AI is analyzing the market...");
    setTimeout(() => showToast("Signal Ready", "New EUR/USD BUY signal generated"), 2000);
  });
  document.getElementById("dashSummaryRefresh")?.addEventListener("click", () => {
    showToast("AI Summary", "Market summary refreshed");
  });

  // Esc closes drawer
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") closeDrawer();
  });

  /**
   * ======================================
   * 08. Performance chart (Chart.js) + period + theme
   * ======================================
   */
  const canvas = document.getElementById("performanceChart");
  if (!canvas || typeof Chart === "undefined") return;

  const isDark = () => document.documentElement.classList.contains("dark");
  const tickColor = () => (isDark() ? "#94A3B8" : "#64748B");
  const gridColor = () => (isDark() ? "rgba(255,255,255,0.05)" : "rgba(0,0,0,0.05)");

  const periods = {
    "7d": { labels: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"], profit: [1200, 2800, 3100, 4500, 5200, 6800, 7500], loss: [400, 600, 900, 1100, 1300, 1500, 1800] },
    "30d": { labels: ["Week 1", "Week 2", "Week 3", "Week 4"], profit: [4200, 8900, 15600, 24892], loss: [1200, 2100, 3400, 4580] },
    "90d": { labels: ["Month 1", "Month 2", "Month 3"], profit: [18500, 42300, 68900], loss: [4200, 8900, 14200] },
    "1y": { labels: ["Q1", "Q2", "Q3", "Q4"], profit: [45000, 98000, 156000, 234000], loss: [12000, 24000, 38000, 52000] },
  };

  const mkDataset = (label, data, color, rgba) => ({
    label,
    data,
    borderColor: color,
    backgroundColor: rgba,
    fill: true,
    tension: 0.4,
    borderWidth: 3,
    pointBackgroundColor: color,
    pointBorderColor: "#fff",
    pointBorderWidth: 2,
    pointRadius: 4,
    pointHoverRadius: 7,
  });

  const perf = new Chart(canvas.getContext("2d"), {
    type: "line",
    data: {
      labels: periods["30d"].labels,
      datasets: [
        mkDataset("Profit", periods["30d"].profit, "#10B981", "rgba(16,185,129,0.1)"),
        mkDataset("Loss", periods["30d"].loss, "#EF4444", "rgba(239,68,68,0.1)"),
      ],
    },
    options: {
      responsive: true,
      maintainAspectRatio: false,
      plugins: {
        legend: { display: true, position: "top", align: "end", labels: { usePointStyle: true, pointStyle: "circle", padding: 20, color: tickColor(), font: { size: 12, weight: "500" } } },
        tooltip: {
          backgroundColor: "rgba(0,0,0,0.8)", padding: 12, titleColor: "#fff", bodyColor: "#fff",
          borderColor: "rgba(16,185,129,0.5)", borderWidth: 1, displayColors: true,
          callbacks: { label: (c) => `${c.dataset.label}: $${c.parsed.y.toLocaleString()}` },
        },
      },
      scales: {
        y: { beginAtZero: true, grid: { color: gridColor() }, ticks: { color: tickColor(), font: { size: 11 }, callback: (v) => "$" + v.toLocaleString() } },
        x: { grid: { display: false }, ticks: { color: tickColor(), font: { size: 11 } } },
      },
      interaction: { intersect: false, mode: "index" },
    },
  });

  document.getElementById("dashChartPeriod")?.addEventListener("change", (e) => {
    const d = periods[e.target.value];
    perf.data.labels = d.labels;
    perf.data.datasets[0].data = d.profit;
    perf.data.datasets[1].data = d.loss;
    perf.update();
  });

  // Re-theme chart when the theme toggles (top-header sets/removes .dark).
  document.getElementById("themeToggle")?.addEventListener("click", () => {
    perf.options.scales.y.grid.color = gridColor();
    perf.options.scales.y.ticks.color = tickColor();
    perf.options.scales.x.ticks.color = tickColor();
    perf.options.plugins.legend.labels.color = tickColor();
    perf.update();
  });
});
