/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: ai-signal-generator.html
 * description: SignalAIX - AI Signal Generator controller, ported from
 *              SignalAIX/MAIN/Ai signal generator/Ai signal generator_01.html
 *              (content + functionality). DOM-only — no HTML generated in JS.
 * author: SignalAIX

    -------------------------------------------------
     01. Init & guard (#ai-signal-generator)
     -------------------------------------------------
     02. Toast
     -------------------------------------------------
     03. Tabs
     -------------------------------------------------
     04. Generator form (markets, assets, timeframe, indicators)
     -------------------------------------------------
     05. Drawers (signal/generate panels + filter) & export menu
     -------------------------------------------------
     06. Generate button, table search
     -------------------------------------------------
     07. Charts (performance / win-rate / distribution) + period + theme
     -------------------------------------------------
    ================================================== */

document.addEventListener("DOMContentLoaded", () => {
  /**
   * ======================================
   * 01. Init & guard
   * ======================================
   */
  if (!document.getElementById("ai-signal-generator")) return;

  const drawerOverlay = document.getElementById("asgDrawerOverlay");
  const signalDrawer = document.getElementById("asgSignalDrawer");
  const filterDrawer = document.getElementById("asgFilterDrawer");
  const toast = document.getElementById("asgToast");
  const refreshIcons = () => window.lucide?.createIcons?.();

  /**
   * ======================================
   * 02. Toast
   * ======================================
   */
  let toastTimer;
  function showToast(title, message) {
    document.getElementById("asgToastTitle").textContent = title;
    document.getElementById("asgToastMessage").textContent = message;
    toast.classList.add("active");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => toast.classList.remove("active"), 4000);
  }
  document.getElementById("asgToastClose")?.addEventListener("click", () => toast.classList.remove("active"));

  /**
   * ======================================
   * 03. Tabs — toggle .asg-pane visibility
   * ======================================
   */
  document.querySelectorAll(".asg-tab").forEach((tab) => {
    tab.addEventListener("click", () => {
      const id = tab.dataset.tab;
      document.querySelectorAll(".asg-tab").forEach((t) => t.classList.remove("active"));
      tab.classList.add("active");
      document.querySelectorAll(".asg-pane").forEach((p) => p.classList.add("hidden"));
      document.getElementById(`asg-tab-${id}`)?.classList.remove("hidden");
      refreshIcons();
    });
  });

  /**
   * ======================================
   * 04. Generator form
   * ======================================
   */
  // Market pills
  document.querySelectorAll(".asg-market").forEach((pill) => {
    pill.addEventListener("click", () => {
      document.querySelectorAll(".asg-market").forEach((p) => {
        p.classList.remove("active", "bg-accent", "text-white", "border-accent");
      });
      pill.classList.add("active");
    });
  });

  // Asset selection — click the card toggles its custom-checkbox + highlight + count.
  const selectedCount = document.getElementById("asgSelectedCount");
  function updateSelectedCount() {
    selectedCount.textContent = document.querySelectorAll(".asg-asset-cb:checked").length;
  }
  function syncAssetCard(item) {
    const on = item.querySelector(".asg-asset-cb")?.checked;
    item.classList.toggle("border-accent", !!on);
    item.classList.toggle("bg-accent/5", !!on);
    item.classList.toggle("border-border", !on);
  }
  document.querySelectorAll(".asg-asset").forEach((item) => {
    syncAssetCard(item); // reflect initial checked state
    item.addEventListener("click", (e) => {
      const cb = item.querySelector(".asg-asset-cb");
      if (!cb) return;
      // Avoid double-toggle when the checkbox itself is clicked.
      if (e.target !== cb) cb.checked = !cb.checked;
      syncAssetCard(item);
      updateSelectedCount();
    });
  });

  // Indicators count + select-all
  const indicatorCount = document.getElementById("asgIndicatorCount");
  const indicators = () => document.querySelectorAll(".asg-indicator");
  function updateIndicatorCount() {
    indicatorCount.textContent = document.querySelectorAll(".asg-indicator:checked").length;
  }
  indicators().forEach((cb) => cb.addEventListener("change", updateIndicatorCount));
  document.getElementById("asgSelectAll")?.addEventListener("click", () => {
    const all = Array.from(indicators()).every((c) => c.checked);
    indicators().forEach((c) => (c.checked = !all));
    updateIndicatorCount();
  });

  // Timeframe buttons
  document.querySelectorAll(".asg-tf").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll(".asg-tf").forEach((b) => b.classList.remove("active"));
      btn.classList.add("active");
    });
  });

  /**
   * ======================================
   * 05. Drawers & export menu
   * ======================================
   */
  function openSignalDrawer(panel) {
    signalDrawer.querySelectorAll(".asg-panel").forEach((p) => p.classList.toggle("hidden", p.dataset.panel !== panel));
    if (panel === "generate") {
      document.getElementById("asgDrawerTitle").textContent = "Generate AI Signal";
      document.getElementById("asgDrawerSub").textContent = "Quick signal generation";
    } else {
      document.getElementById("asgDrawerTitle").textContent = "EUR/USD Signal";
      document.getElementById("asgDrawerSub").textContent = "AI Generated • 2 minutes ago";
    }
    drawerOverlay.classList.add("active");
    signalDrawer.classList.add("active");
    refreshIcons();
  }
  function closeDrawers() {
    drawerOverlay.classList.remove("active");
    signalDrawer.classList.remove("active");
    filterDrawer.classList.remove("active");
  }
  document.querySelectorAll(".asg-open-drawer").forEach((el) => el.addEventListener("click", () => openSignalDrawer(el.dataset.drawer)));
  document.querySelectorAll(".asg-close-drawer").forEach((b) => b.addEventListener("click", closeDrawers));
  drawerOverlay.addEventListener("click", closeDrawers);
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") closeDrawers();
    if ((e.ctrlKey || e.metaKey) && e.key === "k") {
      e.preventDefault();
      document.getElementById("globalSearch")?.focus();
    }
  });

  // Filter drawer
  document.getElementById("asgFilterBtn")?.addEventListener("click", () => {
    drawerOverlay.classList.add("active");
    filterDrawer.classList.add("active");
  });
  const slider = document.getElementById("asgConfidenceSlider");
  const sliderVal = document.getElementById("asgConfidenceValue");
  slider?.addEventListener("input", (e) => (sliderVal.textContent = e.target.value + "%"));
  document.getElementById("asgResetFilters")?.addEventListener("click", () => showToast("Filters Reset", "All filters cleared"));
  document.getElementById("asgApplyFilters")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Filters Applied", "Filter settings saved");
  });

  // Export menu
  const exportBtn = document.getElementById("asgExportBtn");
  const exportMenu = exportBtn?.nextElementSibling;
  exportBtn?.addEventListener("click", (e) => {
    e.stopPropagation();
    exportMenu?.classList.toggle("active");
  });
  document.querySelectorAll(".asg-export").forEach((item) => {
    item.addEventListener("click", () => {
      exportMenu?.classList.remove("active");
      showToast(`Exporting as ${(item.dataset.format || "").toUpperCase()}`, "Your data is being prepared");
    });
  });
  document.addEventListener("click", () => exportMenu?.classList.remove("active"));

  /**
   * ======================================
   * 06. Generate button, table search
   * ======================================
   */
  const genBtn = document.getElementById("asgGenerateBtn");
  const genLabel = document.getElementById("asgGenerateLabel");
  genBtn?.addEventListener("click", () => {
    if (genBtn.dataset.loading === "true") return;
    genBtn.dataset.loading = "true";
    genBtn.disabled = true;
    genLabel.textContent = "Generating...";
    setTimeout(() => {
      genLabel.textContent = "Generate AI Signals";
      genBtn.disabled = false;
      genBtn.dataset.loading = "false";
      showToast("Signals Generated!", "3 new AI signals created");
    }, 2200);
  });

  const tableSearch = document.getElementById("asgTableSearch");
  const typeFilter = document.getElementById("asgTypeFilter");
  function applyTableFilters() {
    const term = (tableSearch?.value || "").toLowerCase();
    const type = typeFilter?.value || "all";
    document.querySelectorAll("#asgSignalsBody tr").forEach((row) => {
      const matchesTerm = row.textContent.toLowerCase().includes(term);
      const matchesType = type === "all" || row.dataset.type === type;
      row.style.display = matchesTerm && matchesType ? "" : "none";
    });
  }
  tableSearch?.addEventListener("input", applyTableFilters);
  typeFilter?.addEventListener("change", applyTableFilters);

  /**
   * ======================================
   * 07. Charts
   * ======================================
   */
  if (typeof Chart === "undefined") return;
  const isDark = () => document.documentElement.classList.contains("dark");
  const tick = () => (isDark() ? "#94A3B8" : "#64748B");
  const grid = () => (isDark() ? "rgba(255,255,255,0.05)" : "rgba(0,0,0,0.05)");

  // Performance (history tab)
  let perf, winRate, dist;
  const perfCanvas = document.getElementById("asgPerformanceChart");
  if (perfCanvas) {
    perf = new Chart(perfCanvas.getContext("2d"), {
      type: "line",
      data: {
        labels: ["Week 1", "Week 2", "Week 3", "Week 4"],
        datasets: [
          { label: "Winning", data: [45, 52, 48, 61], borderColor: "#10B981", backgroundColor: "rgba(16,185,129,0.1)", fill: true, tension: 0.4, borderWidth: 2, pointRadius: 0 },
          { label: "Losing", data: [8, 6, 9, 5], borderColor: "#EF4444", backgroundColor: "rgba(239,68,68,0.1)", fill: true, tension: 0.4, borderWidth: 2, pointRadius: 0 },
        ],
      },
      options: {
        responsive: true, maintainAspectRatio: false,
        plugins: { legend: { position: "top", align: "end", labels: { usePointStyle: true, pointStyle: "circle", padding: 16, color: tick() } } },
        scales: { y: { beginAtZero: true, grid: { color: grid() }, ticks: { color: tick() } }, x: { grid: { display: false }, ticks: { color: tick() } } },
        interaction: { intersect: false, mode: "index" },
      },
    });
  }
  // Win rate (analytics)
  const winCanvas = document.getElementById("asgWinRateChart");
  if (winCanvas) {
    winRate = new Chart(winCanvas.getContext("2d"), {
      type: "bar",
      data: { labels: ["EUR/USD", "GBP/USD", "BTC/USD", "ETH/USD", "XAU/USD"], datasets: [{ label: "Win Rate %", data: [94, 87, 91, 85, 89], backgroundColor: ["rgba(16,185,129,0.8)", "rgba(99,102,241,0.8)", "rgba(245,158,11,0.8)", "rgba(139,92,246,0.8)", "rgba(6,182,212,0.8)"], borderRadius: 8 }] },
      options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { display: false } }, scales: { y: { beginAtZero: true, max: 100, grid: { color: grid() }, ticks: { color: tick(), callback: (v) => v + "%" } }, x: { grid: { display: false }, ticks: { color: tick() } } } },
    });
  }
  // Distribution (analytics)
  const distCanvas = document.getElementById("asgDistributionChart");
  if (distCanvas) {
    dist = new Chart(distCanvas.getContext("2d"), {
      type: "doughnut",
      data: { labels: ["Buy", "Sell", "Hold"], datasets: [{ data: [58, 32, 10], backgroundColor: ["rgba(16,185,129,0.85)", "rgba(239,68,68,0.85)", "rgba(245,158,11,0.85)"], borderWidth: 0 }] },
      options: { responsive: true, maintainAspectRatio: false, cutout: "65%", plugins: { legend: { position: "bottom", labels: { color: tick(), usePointStyle: true, padding: 16 } } } },
    });
  }

  // Period switch (history)
  const periodData = {
    "7d": { l: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"], w: [8, 12, 10, 15, 11, 9, 14], lo: [1, 2, 1, 2, 1, 2, 1] },
    "30d": { l: ["Week 1", "Week 2", "Week 3", "Week 4"], w: [45, 52, 48, 61], lo: [8, 6, 9, 5] },
    "90d": { l: ["Jan", "Feb", "Mar"], w: [180, 195, 210], lo: [25, 22, 28] },
    "1y": { l: ["Q1", "Q2", "Q3", "Q4"], w: [520, 580, 620, 680], lo: [75, 68, 82, 72] },
  };
  document.querySelectorAll(".asg-period").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll(".asg-period").forEach((b) => b.classList.remove("active"));
      btn.classList.add("active");
      const d = periodData[btn.dataset.period];
      if (perf && d) {
        perf.data.labels = d.l;
        perf.data.datasets[0].data = d.w;
        perf.data.datasets[1].data = d.lo;
        perf.update();
      }
    });
  });

  // Re-color charts on theme toggle
  document.getElementById("themeToggle")?.addEventListener("click", () => {
    [perf, winRate].forEach((c) => {
      if (!c) return;
      c.options.scales.y.grid.color = grid();
      c.options.scales.y.ticks.color = tick();
      c.options.scales.x.ticks.color = tick();
      if (c.options.plugins.legend.labels) c.options.plugins.legend.labels.color = tick();
      c.update();
    });
    if (dist) {
      dist.options.plugins.legend.labels.color = tick();
      dist.update();
    }
  });
});
