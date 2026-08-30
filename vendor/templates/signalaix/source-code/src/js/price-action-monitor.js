/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: price-action-monitor.html
 * description: SignalAIX - Price Action Monitor Page Controller
 *              Self-contained; mirrors the reference mockup's functionality
 *              (main tabs, filter chips, table search, auto-refresh toggle,
 *               filter / alert / details drawers, export menu, toast, refresh,
 *               quick actions, Pattern Distribution + Signal Performance charts,
 *               plus a real per-open price chart in the details drawer).
 *              All markup lives in price-action-monitor.html; this file only
 *              modifies the DOM (text/values/classes) and renders charts —
 *              it never injects HTML strings.
 * author: SignalAIX

    -------------------------------------------------
     01. Init & DOM refs (guarded by #price-action-monitor)
     -------------------------------------------------
     02. Toast
     -------------------------------------------------
     03. Drawers (filter / alert / details)
     -------------------------------------------------
     04. Main tabs + filter chips + search (table filtering)
     -------------------------------------------------
     05. Details drawer populate + chart
     -------------------------------------------------
     06. Export menu, refresh, auto-refresh, quick actions
     -------------------------------------------------
     07. Charts (pattern distribution + signal performance)
     -------------------------------------------------
     08. Theme re-color + keyboard shortcuts
     -------------------------------------------------
    ================================================== */

document.addEventListener("DOMContentLoaded", () => {
  /* ======================================
   * 01. Init & DOM refs
   * ====================================== */
  const page = document.getElementById("price-action-monitor");
  if (!page) return; // Guard: only run on the Price Action Monitor page

  const html = document.documentElement;
  const refreshIcons = () => window.lucide?.createIcons?.();

  const overlay = document.getElementById("paDrawerOverlay");
  const filterDrawer = document.getElementById("paFilterDrawer");
  const alertDrawer = document.getElementById("paAlertDrawer");
  const detailsDrawer = document.getElementById("paDetailsDrawer");
  const allDrawers = [filterDrawer, alertDrawer, detailsDrawer];

  const toast = document.getElementById("paToast");
  const toastTitle = document.getElementById("paToastTitle");
  const toastMessage = document.getElementById("paToastMessage");

  const rows = Array.from(document.querySelectorAll(".pa-row"));
  const noResults = document.getElementById("paNoResults");
  const tableSearch = document.getElementById("paTableSearch");

  // Current filter state shared by tabs + chips + search
  let activeTab = "all"; // all | forex | crypto | patterns | signals
  let activeFilter = "all"; // all | bullish | bearish | breakout | reversal | high-volume
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
  document.querySelector(".pa-toast-close")?.addEventListener("click", hideToast);

  /* ======================================
   * 03. Drawers (filter / alert / details)
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
  document.querySelectorAll(".pa-close-drawer").forEach((btn) =>
    btn.addEventListener("click", closeDrawers)
  );
  document.querySelectorAll(".pa-open-filter").forEach((btn) =>
    btn.addEventListener("click", () => openDrawer(filterDrawer))
  );
  document.querySelectorAll(".pa-open-alert").forEach((btn) =>
    btn.addEventListener("click", () => openDrawer(alertDrawer))
  );

  // Apply filters (filter drawer)
  document.querySelector(".pa-apply-filters")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Filters Applied", "Your filter preferences have been saved");
  });
  // Create alert (alert drawer)
  document.querySelector(".pa-create-alert")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Alert Created", "You will be notified when conditions are met");
  });
  // Pattern type chips inside the filter drawer (toggle active)
  document.querySelectorAll(".pa-pattern-chip").forEach((chip) => {
    chip.addEventListener("click", () => {
      const on = chip.classList.toggle("active");
      if (on) {
        chip.classList.add("border-accent", "bg-accent/15", "text-accent");
        chip.classList.remove("border-border", "bg-panel", "text-muted");
      } else {
        chip.classList.remove("border-accent", "bg-accent/15", "text-accent");
        chip.classList.add("border-border", "bg-panel", "text-muted");
      }
    });
  });

  /* ======================================
   * 04. Main tabs + filter chips + search
   * ====================================== */
  function applyRowFilters() {
    let visible = 0;
    rows.forEach((row) => {
      const type = row.dataset.type;
      const signal = row.dataset.signal;
      const text = row.textContent.toLowerCase();

      // Tab match (all / patterns / signals show every type)
      const tabMatch =
        activeTab === "all" ||
        activeTab === "patterns" ||
        activeTab === "signals" ||
        type === activeTab;

      // Chip match (signal-based; pattern-specific chips show all, like the mockup)
      const chipMatch =
        activeFilter === "all" ||
        signal === activeFilter ||
        activeFilter === "breakout" ||
        activeFilter === "reversal" ||
        activeFilter === "high-volume";

      // Search match
      const searchMatch = !searchTerm || text.includes(searchTerm);

      const show = tabMatch && chipMatch && searchMatch;
      row.style.display = show ? "" : "none";
      if (show) visible += 1;
    });
    if (noResults) noResults.classList.toggle("hidden", visible !== 0);
  }

  // Main tabs
  document.querySelectorAll(".pa-tab").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll(".pa-tab").forEach((b) => {
        b.classList.remove("active");
        b.classList.add("text-muted");
      });
      btn.classList.add("active");
      btn.classList.remove("text-muted");
      activeTab = btn.dataset.tab;
      applyRowFilters();
    });
  });

  // Filter chips
  document.querySelectorAll(".pa-chip").forEach((chip) => {
    chip.addEventListener("click", () => {
      document.querySelectorAll(".pa-chip").forEach((c) => {
        c.classList.remove("active", "border-accent", "bg-accent/15", "text-accent");
        c.classList.add("border-border", "bg-panel", "text-muted");
      });
      chip.classList.add("active", "border-accent", "bg-accent/15", "text-accent");
      chip.classList.remove("border-border", "bg-panel", "text-muted");
      activeFilter = chip.dataset.filter;
      applyRowFilters();
    });
  });

  // Table search
  tableSearch?.addEventListener("input", (e) => {
    searchTerm = e.target.value.toLowerCase();
    applyRowFilters();
  });

  /* ======================================
   * 05. Details drawer populate + chart
   * ====================================== */
  // Per-pair metadata mirroring the mockup's analysis copy / levels.
  const PAIR_META = {
    "EUR/USD": {
      price: "1.0856", change: "+0.42%", up: true, high: "1.0892", low: "1.0812",
      pattern: "Ascending Triangle", resistance: "1.0880", confidence: "87%",
      r2: "1.0920", r1: "1.0880", cur: "1.0856", s1: "1.0820", s2: "1.0780",
      series: [1.082, 1.0835, 1.0828, 1.0844, 1.0851, 1.0848, 1.0856],
    },
    "BTC/USD": {
      price: "67,842.50", change: "+3.24%", up: true, high: "68,420.00", low: "65,900.00",
      pattern: "Bull Flag", resistance: "68,000", confidence: "82%",
      r2: "69,500", r1: "68,000", cur: "67,842", s1: "66,500", s2: "65,000",
      series: [65900, 66400, 66200, 67100, 67500, 67400, 67842],
    },
    "GBP/USD": {
      price: "1.2734", change: "-0.28%", up: false, high: "1.2790", low: "1.2710",
      pattern: "Descending Triangle", resistance: "1.2760", confidence: "74%",
      r2: "1.2800", r1: "1.2760", cur: "1.2734", s1: "1.2700", s2: "1.2660",
      series: [1.279, 1.2775, 1.276, 1.2752, 1.2741, 1.2738, 1.2734],
    },
    "ETH/USD": {
      price: "3,542.80", change: "+2.18%", up: true, high: "3,590.00", low: "3,460.00",
      pattern: "Cup & Handle", resistance: "3,600", confidence: "80%",
      r2: "3,680", r1: "3,600", cur: "3,542", s1: "3,480", s2: "3,400",
      series: [3460, 3490, 3475, 3510, 3528, 3535, 3542],
    },
    "USD/JPY": {
      price: "154.82", change: "+0.05%", up: null, high: "155.10", low: "154.50",
      pattern: "Range Bound", resistance: "155.20", confidence: "61%",
      r2: "155.60", r1: "155.20", cur: "154.82", s1: "154.40", s2: "154.00",
      series: [154.6, 154.7, 154.65, 154.78, 154.75, 154.8, 154.82],
    },
    "SOL/USD": {
      price: "142.65", change: "-1.82%", up: false, high: "147.20", low: "141.30",
      pattern: "Bear Flag", resistance: "145.00", confidence: "71%",
      r2: "148.00", r1: "145.00", cur: "142.65", s1: "140.00", s2: "136.50",
      series: [147.2, 146.1, 145.4, 144.2, 143.5, 143.0, 142.65],
    },
    "AUD/USD": {
      price: "0.6542", change: "+0.68%", up: true, high: "0.6570", low: "0.6500",
      pattern: "Double Bottom", resistance: "0.6580", confidence: "79%",
      r2: "0.6610", r1: "0.6580", cur: "0.6542", s1: "0.6510", s2: "0.6470",
      series: [0.65, 0.6515, 0.6508, 0.6525, 0.6536, 0.6539, 0.6542],
    },
    "AVAX/USD": {
      price: "38.45", change: "+5.12%", up: true, high: "39.20", low: "36.40",
      pattern: "Breakout", resistance: "38.00", confidence: "84%",
      r2: "40.50", r1: "39.00", cur: "38.45", s1: "37.00", s2: "35.50",
      series: [36.4, 36.9, 37.3, 37.9, 38.1, 38.3, 38.45],
    },
  };

  const dPair = document.getElementById("paDetailsPair");
  const dPrice = document.getElementById("paDetailsPrice");
  const dChange = document.getElementById("paDetailsChange");
  const dChangeBadge = document.getElementById("paDetailsChangeBadge");
  const dHigh = document.getElementById("paDetailsHigh");
  const dLow = document.getElementById("paDetailsLow");
  const dAnalysis = document.getElementById("paDetailsAnalysis");
  const dR2 = document.getElementById("paDetailsR2");
  const dR1 = document.getElementById("paDetailsR1");
  const dCur = document.getElementById("paDetailsCurrent");
  const dS1 = document.getElementById("paDetailsS1");
  const dS2 = document.getElementById("paDetailsS2");

  let detailsChart = null;
  let currentDetailsPair = "EUR/USD";

  function openDetails(pair) {
    const m = PAIR_META[pair] || PAIR_META["EUR/USD"];
    currentDetailsPair = pair;
    if (dPair) dPair.textContent = pair;
    if (dPrice) dPrice.textContent = m.price;
    if (dChange) dChange.textContent = m.change;
    if (dHigh) dHigh.textContent = m.high;
    if (dLow) dLow.textContent = m.low;
    if (dR2) dR2.textContent = m.r2;
    if (dR1) dR1.textContent = m.r1;
    if (dCur) dCur.textContent = m.cur;
    if (dS1) dS1.textContent = m.s1;
    if (dS2) dS2.textContent = m.s2;

    // Change badge color + icon by direction
    if (dChangeBadge) {
      dChangeBadge.classList.remove(
        "bg-emerald-500/15", "text-emerald-500",
        "bg-red-500/15", "text-red-500",
        "bg-amber-500/15", "text-amber-500"
      );
      const icon = dChangeBadge.querySelector("i");
      if (m.up === true) {
        dChangeBadge.classList.add("bg-emerald-500/15", "text-emerald-500");
        icon?.setAttribute("data-lucide", "trending-up");
      } else if (m.up === false) {
        dChangeBadge.classList.add("bg-red-500/15", "text-red-500");
        icon?.setAttribute("data-lucide", "trending-down");
      } else {
        dChangeBadge.classList.add("bg-amber-500/15", "text-amber-500");
        icon?.setAttribute("data-lucide", "minus");
      }
    }

    if (dAnalysis) {
      dAnalysis.textContent =
        `${pair} is forming a ${m.pattern} pattern on the H1 timeframe. ` +
        `This pattern suggests a potential move around the level at ${m.resistance}. ` +
        `The AI confidence level is ${m.confidence}.`;
    }

    openDrawer(detailsDrawer);
    renderDetailsChart(m);
  }

  function renderDetailsChart(m) {
    const canvas = document.getElementById("paDetailsChart");
    if (!canvas || typeof Chart === "undefined") return;
    if (detailsChart) {
      detailsChart.destroy();
      detailsChart = null;
    }
    requestAnimationFrame(() => {
      const isDark = html.classList.contains("dark");
      const color = m.up === false ? "#ef4444" : "#10b981";
      const ctx = canvas.getContext("2d");
      detailsChart = new Chart(ctx, {
        type: "line",
        data: {
          labels: m.series.map((_, i) => i + 1),
          datasets: [
            {
              data: m.series,
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
      void isDark;
    });
  }

  document.querySelectorAll(".pa-open-details").forEach((btn) => {
    btn.addEventListener("click", () => openDetails(btn.dataset.pair));
  });
  // "Set Alert" inside details drawer → switch to alert drawer
  document.querySelector(".pa-details-alert")?.addEventListener("click", () =>
    openDrawer(alertDrawer)
  );
  document.querySelector(".pa-view-chart")?.addEventListener("click", () =>
    showToast("Chart", `Opening full chart for ${currentDetailsPair}`)
  );

  /* ======================================
   * 06. Export menu, refresh, auto-refresh, quick actions
   * ====================================== */
  const exportBtn = document.getElementById("paExportBtn");
  const exportMenu = document.getElementById("paExportMenu");
  exportBtn?.addEventListener("click", (e) => {
    e.stopPropagation();
    exportMenu?.classList.toggle("hidden");
  });
  document.addEventListener("click", () => exportMenu?.classList.add("hidden"));
  document.querySelectorAll(".pa-export-item").forEach((item) => {
    item.addEventListener("click", () => {
      const fmt = (item.dataset.format || "csv").toUpperCase();
      exportMenu?.classList.add("hidden");
      showToast("Export Started", `Exporting data as ${fmt}...`);
      setTimeout(
        () => showToast("Export Complete", `Your ${fmt} file is ready for download`),
        2000
      );
    });
  });

  // Refresh
  const refreshBtn = document.getElementById("paRefreshBtn");
  refreshBtn?.addEventListener("click", () => {
    const icon = refreshBtn.querySelector("i");
    icon?.classList.add("animate-spin");
    setTimeout(() => {
      icon?.classList.remove("animate-spin");
      showToast("Data Refreshed", "Price action data has been updated");
    }, 1500);
  });

  // Auto-refresh (simulate price update flash on a random visible row)
  const autoToggle = document.getElementById("paAutoRefresh");
  let autoInterval = null;
  function simulatePriceUpdate() {
    const visible = rows.filter((r) => r.style.display !== "none");
    if (!visible.length) return;
    const row = visible[Math.floor(Math.random() * visible.length)];
    const up = Math.random() > 0.5;
    row.style.transition = "background-color 0.5s ease";
    row.style.backgroundColor = up ? "rgba(16,185,129,0.18)" : "rgba(239,68,68,0.18)";
    setTimeout(() => {
      row.style.backgroundColor = "";
    }, 500);
  }
  function startAuto() {
    clearInterval(autoInterval);
    autoInterval = setInterval(simulatePriceUpdate, 5000);
  }
  function stopAuto() {
    clearInterval(autoInterval);
  }
  autoToggle?.addEventListener("change", (e) => {
    if (e.target.checked) startAuto();
    else stopAuto();
  });
  if (autoToggle?.checked) startAuto();

  // Quick actions
  document.querySelectorAll(".pa-quick-action").forEach((btn) => {
    btn.addEventListener("click", () => {
      const action = btn.dataset.action;
      if (action === "save") showToast("View Saved", "Your current view has been saved");
      else if (action === "share") showToast("Share", "Share link copied to clipboard");
      else if (action === "settings") showToast("Settings", "Opening monitor settings...");
    });
  });

  /* ======================================
   * 07. Charts (pattern distribution + signal performance)
   * ====================================== */
  let patternChart = null;
  let performanceChart = null;

  function chartColors() {
    const isDark = html.classList.contains("dark");
    return {
      grid: isDark ? "rgba(255,255,255,0.05)" : "rgba(0,0,0,0.05)",
      text: isDark ? "#94A3B8" : "#64748B",
    };
  }

  function initCharts() {
    if (typeof Chart === "undefined") return;
    const { grid, text } = chartColors();

    const patternEl = document.getElementById("paPatternChart");
    if (patternEl) {
      patternChart = new Chart(patternEl.getContext("2d"), {
        type: "doughnut",
        data: {
          labels: ["Ascending Triangle", "Bull Flag", "Head & Shoulders", "Double Bottom", "Cup & Handle", "Others"],
          datasets: [
            {
              data: [25, 18, 15, 12, 10, 20],
              backgroundColor: ["#10b981", "#14b8a6", "#ef4444", "#f59e0b", "#8b5cf6", "#64748b"],
              borderWidth: 0,
              hoverOffset: 10,
            },
          ],
        },
        options: {
          responsive: true,
          maintainAspectRatio: false,
          plugins: {
            legend: {
              position: "right",
              labels: { color: text, usePointStyle: true, padding: 15, font: { size: 12 } },
            },
          },
          cutout: "65%",
        },
      });
    }

    const perfEl = document.getElementById("paPerformanceChart");
    if (perfEl) {
      performanceChart = new Chart(perfEl.getContext("2d"), {
        type: "bar",
        data: {
          labels: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
          datasets: [
            { label: "Winning Signals", data: [28, 32, 25, 35, 30, 22, 18], backgroundColor: "#10b981", borderRadius: 6 },
            { label: "Losing Signals", data: [8, 12, 10, 8, 15, 6, 5], backgroundColor: "#ef4444", borderRadius: 6 },
          ],
        },
        options: {
          responsive: true,
          maintainAspectRatio: false,
          plugins: {
            legend: {
              position: "top",
              align: "end",
              labels: { color: text, usePointStyle: true, padding: 20, font: { size: 12 } },
            },
          },
          scales: {
            y: { beginAtZero: true, grid: { color: grid }, ticks: { color: text } },
            x: { grid: { display: false }, ticks: { color: text } },
          },
        },
      });
    }
  }

  initCharts();

  /* ======================================
   * 08. Theme re-color + keyboard shortcuts
   * ====================================== */
  function recolorCharts() {
    const { grid, text } = chartColors();
    if (patternChart) {
      patternChart.options.plugins.legend.labels.color = text;
      patternChart.update();
    }
    if (performanceChart) {
      performanceChart.options.scales.y.grid.color = grid;
      performanceChart.options.scales.y.ticks.color = text;
      performanceChart.options.scales.x.ticks.color = text;
      performanceChart.options.plugins.legend.labels.color = text;
      performanceChart.update();
    }
  }
  document.getElementById("themeToggle")?.addEventListener("click", () => {
    setTimeout(recolorCharts, 50);
  });

  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") closeDrawers();
  });
});
