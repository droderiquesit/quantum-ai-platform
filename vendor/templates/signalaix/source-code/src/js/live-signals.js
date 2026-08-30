/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: live-signals.html
 * description: SignalAIX - Live Signals Page Controller
 *              Self-contained: mirrors the reference mockup's functionality
 *              (category tabs, quick filters, signal-details/filter/export
 *               drawers, table filter, toast, save/share/copy, search).
 *              All markup lives in live-signals.html; this file only modifies
 *              the DOM (text/values/classes) — it never injects HTML strings.
 * author: SignalAIX

    -------------------------------------------------
     01. Init & DOM refs (guarded by #live-signals)
     -------------------------------------------------
     02. Toast
     -------------------------------------------------
     03. Drawers (signal / filter / export)
     -------------------------------------------------
     04. Category Tabs & Quick Filters
     -------------------------------------------------
     05. Signal Details content
     -------------------------------------------------
     06. Filter drawer controls
     -------------------------------------------------
     07. Export, Refresh, Save/Share/Copy
     -------------------------------------------------
     08. Table filter, Search, Shortcuts, Live ticks
     -------------------------------------------------
    ================================================== */

document.addEventListener("DOMContentLoaded", () => {
  /**
   * ======================================
   * 01. Init & DOM refs
   * ======================================
   */
  const page = document.getElementById("live-signals");
  if (!page) return; // Guard: only run on the Live Signals page

  const drawerOverlay = document.getElementById("drawerOverlay");
  const signalDrawer = document.getElementById("signalDrawer");
  const filterDrawer = document.getElementById("filterDrawer");
  const exportDrawer = document.getElementById("exportDrawer");
  const toast = document.getElementById("toast");
  const grid = document.getElementById("signalsGrid");
  const noResults = document.getElementById("lsNoResults");

  // Current filter state shared by tabs + quick filters
  let activeTab = "all"; // all | forex | crypto | commodities | indices
  let activeType = "all"; // all | buy | sell | high-confidence

  const refreshIcons = () => window.lucide?.createIcons?.();

  /**
   * ======================================
   * 02. Toast
   * ======================================
   */
  let toastTimer;
  function showToast(title, message) {
    document.getElementById("toastTitle").textContent = title;
    document.getElementById("toastMessage").textContent = message;
    toast.classList.add("active");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(hideToast, 4000);
  }
  function hideToast() {
    toast.classList.remove("active");
  }
  document.getElementById("lsToastClose")?.addEventListener("click", hideToast);

  /**
   * ======================================
   * 03. Drawers
   * ======================================
   */
  function openDrawer(drawer) {
    drawerOverlay.classList.add("active");
    drawer.classList.add("active");
  }
  function closeAllDrawers() {
    drawerOverlay.classList.remove("active");
    signalDrawer.classList.remove("active");
    filterDrawer.classList.remove("active");
    exportDrawer.classList.remove("active");
  }
  drawerOverlay.addEventListener("click", closeAllDrawers);
  document.querySelectorAll(".ls-close-drawer").forEach((btn) => btn.addEventListener("click", closeAllDrawers));
  document.getElementById("lsFilterBtn")?.addEventListener("click", () => openDrawer(filterDrawer));
  document.getElementById("lsExportBtn")?.addEventListener("click", () => openDrawer(exportDrawer));

  /**
   * ======================================
   * 04. Category Tabs & Quick Filters
   * ======================================
   */
  function applyFilters() {
    const cards = grid.querySelectorAll(".signal-card");
    let visible = 0;
    cards.forEach((card) => {
      const cat = card.dataset.category;
      const type = card.dataset.type;
      const conf = parseInt(card.dataset.confidence, 10) || 0;

      let show = true;
      if (activeTab !== "all" && cat !== activeTab) show = false;
      if (activeType === "buy" && type !== "buy") show = false;
      if (activeType === "sell" && type !== "sell") show = false;
      if (activeType === "high-confidence" && conf < 85) show = false;

      card.style.display = show ? "" : "none";
      if (show) visible++;
    });
    noResults.classList.toggle("hidden", visible > 0);
  }

  // Tabs (category)
  document.querySelectorAll(".ls-tab").forEach((tab) => {
    tab.addEventListener("click", () => {
      document.querySelectorAll(".ls-tab").forEach((t) => t.classList.remove("active"));
      tab.classList.add("active");
      activeTab = tab.dataset.tab;
      applyFilters();
      showToast("Filter Applied", `Showing ${activeTab === "all" ? "all" : activeTab} signals`);
    });
  });

  // Quick filter chips (type)
  document.querySelectorAll("#lsQuickFilters .ls-chip").forEach((chip) => {
    chip.addEventListener("click", () => {
      document.querySelectorAll("#lsQuickFilters .ls-chip").forEach((c) => c.classList.remove("active"));
      chip.classList.add("active");
      const f = chip.dataset.filter;
      // scalping/swing are illustrative (no data) -> treat as 'all' for now
      activeType = ["buy", "sell", "high-confidence"].includes(f) ? f : "all";
      applyFilters();
    });
  });

  /**
   * ======================================
   * 05. Signal Details content
   * ======================================
   */
  // Cached references to the static signal-details template in the HTML.
  const sd = {
    title: document.getElementById("sdTitle"),
    header: document.getElementById("sdHeader"),
    iconWrap: document.getElementById("sdHeaderIconWrap"),
    icon: document.getElementById("sdHeaderIcon"),
    pair: document.getElementById("sdPair"),
    typeLabel: document.getElementById("sdTypeLabel"),
    entry: document.getElementById("sdEntry"),
    tp: document.getElementById("sdTp"),
    sl: document.getElementById("sdSl"),
    confBar: document.getElementById("sdConfBar"),
    confValue: document.getElementById("sdConfValue"),
    chart: document.getElementById("sdChart"),
  };

  // Chart.js instance for the signal-details price chart (recreated per open).
  let sdChartInstance = null;

  // Render a small price line/area chart in the drawer, colored by signal type.
  function renderSignalChart(isBuy) {
    if (!sd.chart || typeof Chart === "undefined") return;
    const color = isBuy ? "#10b981" : "#ef4444"; // emerald / red
    const ctx = sd.chart.getContext("2d");

    // Deterministic-ish sample series so it looks like a price path.
    const points = [];
    let v = 50;
    for (let i = 0; i < 24; i++) {
      v += Math.sin(i / 2) * 6 + (i % 3 === 0 ? 4 : -3);
      points.push(Math.max(10, Math.min(90, v)));
    }

    const gradient = ctx.createLinearGradient(0, 0, 0, sd.chart.height || 192);
    gradient.addColorStop(0, isBuy ? "rgba(16,185,129,0.30)" : "rgba(239,68,68,0.30)");
    gradient.addColorStop(1, "rgba(0,0,0,0)");

    if (sdChartInstance) sdChartInstance.destroy();
    sdChartInstance = new Chart(ctx, {
      type: "line",
      data: {
        labels: points.map((_, i) => i),
        datasets: [
          {
            data: points,
            borderColor: color,
            backgroundColor: gradient,
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
        plugins: { legend: { display: false }, tooltip: { enabled: false } },
        scales: { x: { display: false }, y: { display: false } },
        animation: { duration: 400 },
      },
    });
  }

  // Open the signal-details drawer by MODIFYING the static template's DOM
  // (text/values/classes) — no HTML is generated in JS.
  function openSignalDetails(pair, type, levels) {
    const lv = levels || { entry: "1.0842", tp: "1.0895", sl: "1.0815", conf: 92 };
    const isBuy = type === "buy";

    sd.title.textContent = `${pair} Signal`;
    sd.pair.textContent = pair;
    sd.typeLabel.textContent = `${type} Signal`;
    sd.entry.textContent = lv.entry;
    sd.tp.textContent = lv.tp;
    sd.sl.textContent = lv.sl;
    sd.confBar.style.width = `${lv.conf}%`;
    sd.confValue.textContent = `${lv.conf}%`;

    // Type-dependent styling — toggle classes rather than rebuild markup.
    sd.header.classList.toggle("bg-emerald-500/10", isBuy);
    sd.header.classList.toggle("bg-red-500/10", !isBuy);

    sd.iconWrap.classList.toggle("from-emerald-500", isBuy);
    sd.iconWrap.classList.toggle("to-teal-500", isBuy);
    sd.iconWrap.classList.toggle("from-red-500", !isBuy);
    sd.iconWrap.classList.toggle("to-rose-600", !isBuy);

    sd.typeLabel.classList.toggle("text-emerald-500", isBuy);
    sd.typeLabel.classList.toggle("text-red-500", !isBuy);

    sd.icon.setAttribute("data-lucide", isBuy ? "arrow-up-right" : "arrow-down-right");

    openDrawer(signalDrawer);
    refreshIcons();
    // Render after the drawer is visible so the canvas has measurable size.
    requestAnimationFrame(() => renderSignalChart(isBuy));
  }

  // Save / Copy in the details drawer are static elements — bind once.
  document.getElementById("lsDrawerSave")?.addEventListener("click", () => showToast("Signal Saved", "Added to your watchlist"));
  document.getElementById("lsDrawerCopy")?.addEventListener("click", () => showToast("Signal Copied", "Trade details copied to clipboard"));

  // Details buttons on each card
  grid.querySelectorAll(".ls-details").forEach((btn) => {
    btn.addEventListener("click", () => {
      const card = btn.closest(".signal-card");
      openSignalDetails(card.dataset.pair, card.dataset.type, {
        entry: card.querySelector(".grid .font-mono")?.textContent || "",
        tp: card.querySelectorAll(".grid .font-mono")[1]?.textContent || "",
        sl: card.querySelectorAll(".grid .font-mono")[2]?.textContent || "",
        conf: parseInt(card.dataset.confidence, 10) || 90,
      });
    });
  });

  /**
   * ======================================
   * 06. Filter drawer controls
   * ======================================
   */
  const confidenceRange = document.getElementById("confidenceRange");
  const confidenceValue = document.getElementById("confidenceValue");
  confidenceRange?.addEventListener("input", function () {
    confidenceValue.textContent = this.value + "%";
  });

  // Signal-type chips inside filter drawer
  document.querySelectorAll("#lsDrawerTypeChips .ls-dtype").forEach((chip) => {
    chip.addEventListener("click", () => {
      document.querySelectorAll("#lsDrawerTypeChips .ls-dtype").forEach((c) => c.classList.remove("active"));
      chip.classList.add("active");
    });
  });
  // Timeframe chips (toggle single)
  document.querySelectorAll("#lsTimeframeChips .ls-tf").forEach((chip) => {
    chip.addEventListener("click", () => {
      document.querySelectorAll("#lsTimeframeChips .ls-tf").forEach((c) => c.classList.remove("active"));
      chip.classList.add("active");
    });
  });

  document.getElementById("lsResetFilters")?.addEventListener("click", () => {
    filterDrawer.querySelectorAll('input[type="checkbox"]').forEach((cb) => (cb.checked = true));
    if (confidenceRange) {
      confidenceRange.value = 70;
      confidenceValue.textContent = "70%";
    }
    showToast("Filters Reset", "All filters have been reset");
  });
  document.getElementById("lsApplyFilters")?.addEventListener("click", () => {
    closeAllDrawers();
    showToast("Filters Applied", "Signals filtered successfully");
  });

  /**
   * ======================================
   * 07. Export, Refresh, Save/Share/Copy
   * ======================================
   */
  document.getElementById("lsExportConfirm")?.addEventListener("click", () => {
    closeAllDrawers();
    showToast("Export Started", "Your file is being prepared...");
    setTimeout(() => showToast("Export Complete", "Download started successfully"), 2000);
  });

  document.getElementById("lsRefreshBtn")?.addEventListener("click", function () {
    const icon = this.querySelector("i");
    if (icon) {
      icon.style.transition = "transform 1s linear";
      icon.style.transform = "rotate(360deg)";
      setTimeout(() => {
        icon.style.transform = "";
      }, 1000);
    }
    setTimeout(() => showToast("Signals Refreshed", "3 new signals available"), 1000);
  });

  // Save / Share on each card
  grid.querySelectorAll(".ls-save").forEach((btn) => {
    btn.addEventListener("click", () => {
      const icon = btn.querySelector("i");
      icon?.setAttribute("data-lucide", "bookmark-check");
      refreshIcons();
      showToast("Signal Saved", "Added to your watchlist");
    });
  });
  grid.querySelectorAll(".ls-share").forEach((btn) => {
    btn.addEventListener("click", () => showToast("Share Link Copied", "Signal link copied to clipboard"));
  });

  /**
   * ======================================
   * 08. Table filter, Search, Shortcuts, Live ticks
   * ======================================
   */
  document.querySelectorAll(".ls-history-view").forEach((btn) => {
    btn.addEventListener("click", () => {
      const row = btn.closest("tr");
      const pair = row.querySelector(".font-medium").textContent;
      openSignalDetails(pair, row.dataset.type || "buy");
    });
  });

  document.getElementById("tableFilter")?.addEventListener("change", function () {
    const filter = this.value;
    document.querySelectorAll("#signalTableBody tr").forEach((row) => {
      row.style.display = filter === "all" || row.dataset.type === filter ? "" : "none";
    });
  });

  // Global search (top header) filters cards on Enter
  const globalSearch = document.getElementById("globalSearch");
  globalSearch?.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && globalSearch.value.trim()) {
      const term = globalSearch.value.toLowerCase();
      let found = 0;
      grid.querySelectorAll(".signal-card").forEach((card) => {
        const match = card.textContent.toLowerCase().includes(term);
        card.style.display = match ? "" : "none";
        if (match) found++;
      });
      noResults.classList.toggle("hidden", found > 0);
      showToast("Search Results", `Found ${found} matching signals`);
    }
  });

  // Keyboard: Esc closes drawers
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") closeAllDrawers();
  });

  // Simulated real-time price flashes
  setInterval(() => {
    grid.querySelectorAll(".font-mono").forEach((price) => {
      if (Math.random() > 0.85) {
        const up = Math.random() > 0.5;
        price.style.transition = "color 0.2s ease";
        const original = price.style.color;
        price.style.color = up ? "var(--color-success)" : "var(--color-error)";
        setTimeout(() => {
          price.style.color = original;
        }, 500);
      }
    });
  }, 3000);
});
