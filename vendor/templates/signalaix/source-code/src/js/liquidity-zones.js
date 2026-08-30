/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: liquidity-zones.js
 * description: Self-contained controller for the Liquidity Zones page
 *              (#liquidity-zones). DOM-only — the zone-details and filters
 *              drawers are static templates the JS shows/hides and patches via
 *              textContent / className / setAttribute; no HTML is generated in JS.
 * author: SignalAIX

    -------------------------------------------------
    Table of Contents
    -------------------------------------------------
    01. Init & guard
    02. Toast
    03. Tabs (market filter on the table)
    04. Filter chips (type/impact/near/untested filter)
    05. Table search
    06. Combined table filtering (tab + chip + search)
    07. Drawers (zone details + filters) open/close
    08. Zone details — populate from row / top-zone data attrs
    09. Filters drawer (range value, reset, apply)
    10. Row actions (watch / alert) + header actions
    11. Charts (distribution bar + accuracy line) + theme re-color
    ================================================== */

(function () {
  /* 01. Init & guard */
  if (!document.getElementById("liquidity-zones")) return;

  const htmlEl = document.documentElement;
  const isDark = () => htmlEl.classList.contains("dark");
  const tickColor = () => (isDark() ? "#94A3B8" : "#64748B");
  const gridColor = () => (isDark() ? "rgba(255,255,255,0.05)" : "rgba(0,0,0,0.06)");
  const refreshIcons = () => window.lucide && window.lucide.createIcons();

  /* 02. Toast */
  const toast = document.getElementById("lzToast");
  let toastTimer;
  function showToast(title, message) {
    if (!toast) return;
    if (title) document.getElementById("lzToastTitle").textContent = title;
    if (message) document.getElementById("lzToastMessage").textContent = message;
    toast.classList.add("active");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => toast.classList.remove("active"), 3000);
  }
  document.getElementById("lzToastClose")?.addEventListener("click", () => toast.classList.remove("active"));

  /* 03. Tabs (market filter) */
  let currentTab = "all";
  const tabs = document.querySelectorAll(".lz-tab");
  tabs.forEach((tab) => {
    tab.addEventListener("click", () => {
      tabs.forEach((t) => {
        t.classList.remove("active", "bg-accent", "text-white");
        t.classList.add("text-muted");
      });
      tab.classList.add("active", "bg-accent", "text-white");
      tab.classList.remove("text-muted");
      currentTab = tab.dataset.tab;
      applyTableFilters();
    });
  });
  // set initial active styling
  document.querySelector(".lz-tab.active")?.classList.add("bg-accent", "text-white");
  document.querySelector(".lz-tab.active")?.classList.remove("text-muted");

  /* 04. Filter chips */
  let currentFilter = "all";
  const chips = document.querySelectorAll(".lz-chip");
  chips.forEach((chip) => {
    chip.addEventListener("click", () => {
      chips.forEach((c) => {
        c.classList.remove("active", "bg-accent/10", "border-accent", "text-accent");
        c.classList.add("text-muted");
      });
      chip.classList.add("active", "bg-accent/10", "border-accent", "text-accent");
      chip.classList.remove("text-muted");
      currentFilter = chip.dataset.filter;
      applyTableFilters();
    });
  });
  document.querySelector(".lz-chip.active")?.classList.add("bg-accent/10", "border-accent", "text-accent");
  document.querySelector(".lz-chip.active")?.classList.remove("text-muted");

  /* 05. Table search */
  let searchTerm = "";
  document.getElementById("lzTableSearch")?.addEventListener("input", (e) => {
    searchTerm = e.target.value.toLowerCase();
    applyTableFilters();
  });

  /* 06. Combined table filtering */
  const rows = Array.from(document.querySelectorAll(".lz-row"));
  const noResults = document.getElementById("lzNoResults");

  function rowPassesFilter(d) {
    switch (currentFilter) {
      case "support":
        return d.type === "support";
      case "resistance":
        return d.type === "resistance";
      case "high-impact":
        return parseInt(d.strength, 10) >= 85;
      case "near-price":
        return parseFloat(d.distance) <= 1;
      case "untested":
        return parseInt(d.touches, 10) <= 1;
      default:
        return true;
    }
  }

  function applyTableFilters() {
    let visible = 0;
    rows.forEach((r) => {
      const d = r.dataset;
      const tabOk = currentTab === "all" || d.market === currentTab;
      const filterOk = rowPassesFilter(d);
      const searchOk = !searchTerm || (d.pair + " " + d.market).toLowerCase().includes(searchTerm);
      const show = tabOk && filterOk && searchOk;
      r.style.display = show ? "" : "none";
      if (show) visible++;
    });
    if (noResults) noResults.classList.toggle("hidden", visible !== 0);
  }

  /* 07. Drawers open/close */
  const overlay = document.getElementById("lzDrawerOverlay");
  const zoneDrawer = document.getElementById("lzZoneDrawer");
  const filterDrawer = document.getElementById("lzFilterDrawer");

  function openDrawer(d) {
    overlay?.classList.add("active");
    d?.classList.add("active");
    refreshIcons();
  }
  function closeDrawers() {
    overlay?.classList.remove("active");
    zoneDrawer?.classList.remove("active");
    filterDrawer?.classList.remove("active");
  }
  document.querySelectorAll(".lz-drawer-close").forEach((b) => b.addEventListener("click", closeDrawers));
  overlay?.addEventListener("click", closeDrawers);
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") closeDrawers();
  });
  document.getElementById("lzFilters")?.addEventListener("click", () => openDrawer(filterDrawer));

  /* 08. Zone details — populate from data attrs */
  let activeZonePair = "";
  function openZoneDetails(d) {
    activeZonePair = d.pair;
    const isSupport = d.type === "support";
    const cap = d.type.charAt(0).toUpperCase() + d.type.slice(1);

    document.getElementById("lzZoneSubtitle").textContent = `${d.pair} - ${cap} Zone`;
    document.getElementById("lzZonePair").textContent = d.pair;

    const typeLabel = document.getElementById("lzZoneTypeLabel");
    typeLabel.textContent = `${cap} Zone`;
    typeLabel.className = `text-sm ${isSupport ? "text-emerald-500" : "text-red-500"}`;

    document.getElementById("lzZoneOverview").className = `p-4 rounded-xl ${isSupport ? "bg-emerald-500/10 border border-emerald-500/20" : "bg-red-500/10 border border-red-500/20"}`;
    document.getElementById("lzZoneIconWrap").className = `w-12 h-12 rounded-xl ${isSupport ? "bg-emerald-500/20" : "bg-red-500/20"} flex items-center justify-center shrink-0`;
    const icon = document.getElementById("lzZoneIcon");
    icon.setAttribute("data-lucide", isSupport ? "arrow-down-to-line" : "arrow-up-from-line");
    icon.className = `w-6 h-6 ${isSupport ? "text-emerald-500" : "text-red-500"} shrink-0`;

    document.getElementById("lzZoneRange").textContent = d.range;

    const dist = parseFloat(d.distance);
    const distEl = document.getElementById("lzZoneDistance");
    distEl.textContent = `${d.distance}% from price`;
    distEl.className = `font-semibold ${dist <= 0.5 ? "text-red-500" : "text-text"}`;

    const strength = parseInt(d.strength, 10);
    const strColor = strength >= 80 ? "bg-emerald-500" : strength >= 60 ? "bg-yellow-500" : "bg-red-500";
    const strTextColor = strength >= 80 ? "text-emerald-500" : "text-yellow-500";
    const strLabel = document.getElementById("lzZoneStrengthLabel");
    strLabel.textContent = `${strength}%`;
    strLabel.className = `text-sm font-bold ${strTextColor}`;
    const strBar = document.getElementById("lzZoneStrengthBar");
    strBar.className = `h-full rounded-full ${strColor}`;
    strBar.style.width = `${strength}%`;

    document.getElementById("lzZoneTouches").textContent = d.touches;
    document.getElementById("lzZoneAiText").textContent = `This ${d.type} zone has shown consistent price reaction with ${strength}% strength rating. Historical data suggests high probability of bounce at this level.`;

    openDrawer(zoneDrawer);
  }

  rows.forEach((row) => {
    row.addEventListener("click", (e) => {
      if (e.target.closest(".lz-watch") || e.target.closest(".lz-alert")) return;
      openZoneDetails(row.dataset);
    });
  });
  document.querySelectorAll(".lz-top").forEach((item) => {
    item.addEventListener("click", () => openZoneDetails(item.dataset));
  });

  /* 09. Filters drawer interactions */
  const range = document.getElementById("lzStrengthRange");
  const rangeVal = document.getElementById("lzStrengthValue");
  range?.addEventListener("input", () => {
    if (rangeVal) rangeVal.textContent = `${range.value}%`;
  });
  document.querySelector(".lz-reset")?.addEventListener("click", () => {
    if (range) {
      range.value = 60;
      if (rangeVal) rangeVal.textContent = "60%";
    }
    showToast("Filters Reset", "All filters have been reset to default");
  });
  document.querySelector(".lz-apply")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Filters Applied", "Your filter settings have been applied");
  });

  /* 10. Row actions + header actions */
  document.querySelectorAll(".lz-watch").forEach((b) =>
    b.addEventListener("click", (e) => {
      e.stopPropagation();
      showToast("Added to Watchlist", `${b.dataset.pair} has been added to your watchlist`);
    })
  );
  document.querySelectorAll(".lz-alert").forEach((b) =>
    b.addEventListener("click", (e) => {
      e.stopPropagation();
      showToast("Alert Created", `Price alert set for ${b.dataset.pair} zone`);
    })
  );
  document.querySelector(".lz-drawer-alert")?.addEventListener("click", () => showToast("Alert Created", `Price alert set for ${activeZonePair} zone`));
  document.querySelector(".lz-drawer-watch")?.addEventListener("click", () => showToast("Added to Watchlist", `${activeZonePair} has been added to your watchlist`));

  document.querySelectorAll(".lz-export").forEach((b) =>
    b.addEventListener("click", () => showToast("Export Started", `Exporting data as ${b.dataset.format.toUpperCase()}...`))
  );
  document.getElementById("lzRefresh")?.addEventListener("click", () => showToast("Data Refreshed", "Liquidity zones updated successfully"));
  document.getElementById("lzConfigAlerts")?.addEventListener("click", () => showToast("Notifications", "Opening alert configuration..."));

  /* 11. Charts */
  const charts = {};

  function initDistribution() {
    if (typeof Chart === "undefined") return;
    const el = document.getElementById("lzDistributionChart");
    if (!el || charts.dist) return;
    charts.dist = new Chart(el.getContext("2d"), {
      type: "bar",
      data: {
        labels: ["Forex", "Crypto", "Commodities", "Indices"],
        datasets: [
          { label: "Support Zones", data: [245, 128, 32, 18], backgroundColor: "rgba(16, 185, 129, 0.8)", borderRadius: 8 },
          { label: "Resistance Zones", data: [238, 132, 35, 19], backgroundColor: "rgba(239, 68, 68, 0.8)", borderRadius: 8 },
        ],
      },
      options: {
        responsive: true,
        maintainAspectRatio: false,
        plugins: { legend: { position: "top", labels: { color: tickColor(), usePointStyle: true } } },
        scales: {
          x: { grid: { color: gridColor() }, ticks: { color: tickColor() } },
          y: { grid: { color: gridColor() }, ticks: { color: tickColor() } },
        },
      },
    });
  }

  function initAccuracy() {
    if (typeof Chart === "undefined") return;
    const el = document.getElementById("lzAccuracyChart");
    if (!el || charts.acc) return;
    charts.acc = new Chart(el.getContext("2d"), {
      type: "line",
      data: {
        labels: ["Week 1", "Week 2", "Week 3", "Week 4"],
        datasets: [
          { label: "Support Hit Rate", data: [82, 85, 88, 91], borderColor: "#10B981", backgroundColor: "rgba(16, 185, 129, 0.1)", fill: true, tension: 0.4, pointBackgroundColor: "#10B981" },
          { label: "Resistance Hit Rate", data: [78, 81, 84, 87], borderColor: "#EF4444", backgroundColor: "rgba(239, 68, 68, 0.1)", fill: true, tension: 0.4, pointBackgroundColor: "#EF4444" },
        ],
      },
      options: {
        responsive: true,
        maintainAspectRatio: false,
        plugins: { legend: { position: "top", labels: { color: tickColor(), usePointStyle: true } } },
        scales: {
          x: { grid: { color: gridColor() }, ticks: { color: tickColor() } },
          y: { min: 70, max: 100, grid: { color: gridColor() }, ticks: { color: tickColor(), callback: (v) => v + "%" } },
        },
      },
    });
  }

  // Distribution toggle (Support / Resistance) — toggle dataset visibility
  document.querySelectorAll(".lz-dist-toggle").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll(".lz-dist-toggle").forEach((b) => {
        b.classList.remove("active", "bg-accent/10", "border-accent", "text-accent");
        b.classList.add("text-muted");
      });
      btn.classList.add("active", "bg-accent/10", "border-accent", "text-accent");
      btn.classList.remove("text-muted");
      if (!charts.dist) return;
      const want = btn.dataset.chart; // 'support' | 'resistance'
      charts.dist.setDatasetVisibility(0, want === "support");
      charts.dist.setDatasetVisibility(1, want === "resistance");
      charts.dist.update();
    });
  });

  function recolor() {
    [charts.dist, charts.acc].forEach((c) => {
      if (!c) return;
      c.options.plugins.legend.labels.color = tickColor();
      c.options.scales.x.grid.color = gridColor();
      c.options.scales.x.ticks.color = tickColor();
      c.options.scales.y.grid.color = gridColor();
      c.options.scales.y.ticks.color = tickColor();
      c.update();
    });
  }
  document.getElementById("themeToggle")?.addEventListener("click", () => setTimeout(recolor, 0));

  function initCharts() {
    initDistribution();
    initAccuracy();
    // default view = Support only
    if (charts.dist) {
      charts.dist.setDatasetVisibility(0, true);
      charts.dist.setDatasetVisibility(1, false);
      charts.dist.update();
    }
  }
  if (document.readyState === "complete") initCharts();
  else window.addEventListener("load", initCharts);
})();
