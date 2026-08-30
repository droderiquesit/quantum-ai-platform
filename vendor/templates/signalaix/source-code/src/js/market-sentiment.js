/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: market-sentiment.js
 * description: Self-contained controller for the Market Sentiment page
 *              (#market-sentiment). DOM-only — all markup is static; the
 *              asset-detail / all-movers drawer panels are static templates
 *              patched via textContent/className. No HTML generated in JS.
 * author: SignalAIX

    -------------------------------------------------
    Table of Contents
    -------------------------------------------------
    01. Init & guard
    02. Toast
    03. Main tabs (Overview / Forex / Crypto / Social / News / History)
    04. Fear & Greed mini tabs (Forex / Crypto / Combined)
    05. Range pill groups (trend / history)
    06. Export menu
    07. Drawers (asset detail, all movers, filter) + filter pills
    08. Movers search + type filter
    09. Charts + theme re-color
    ================================================== */

(function () {
  if (!document.getElementById("market-sentiment")) return;

  const htmlEl = document.documentElement;
  const isDark = () => htmlEl.classList.contains("dark");
  const tickColor = () => (isDark() ? "#94A3B8" : "#64748B");
  const gridColor = () => (isDark() ? "rgba(255,255,255,0.05)" : "rgba(0,0,0,0.05)");
  const refreshIcons = () => window.lucide && window.lucide.createIcons();

  /* 02. Toast */
  const toast = document.getElementById("msToast");
  let toastTimer;
  function showToast(title, message) {
    if (!toast) return;
    if (title) document.getElementById("msToastTitle").textContent = title;
    if (message) document.getElementById("msToastMessage").textContent = message;
    toast.classList.add("active");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => toast.classList.remove("active"), 3000);
  }
  document.getElementById("msToastClose")?.addEventListener("click", () => toast.classList.remove("active"));
  document.getElementById("msRefresh")?.addEventListener("click", () => showToast("Refreshing", "Updating sentiment data..."));
  document.querySelectorAll(".ms-toast-btn").forEach((b) =>
    b.addEventListener("click", () => showToast(b.dataset.toastTitle, b.dataset.toastMsg))
  );

  /* 03. Main tabs */
  document.querySelectorAll(".ms-tab").forEach((tab) => {
    tab.addEventListener("click", () => {
      document.querySelectorAll(".ms-tab").forEach((t) => t.classList.remove("active"));
      document.querySelectorAll(".tab-content").forEach((c) => c.classList.remove("active"));
      tab.classList.add("active");
      document.getElementById(`${tab.dataset.tab}-content`)?.classList.add("active");
      refreshIcons();
    });
  });

  /* 04. Fear & Greed mini tabs */
  const GREED = {
    forex: { value: 68, label: "Greed", color: "emerald" },
    crypto: { value: 78, label: "Greed", color: "violet" },
    combined: { value: 72, label: "Greed", color: "emerald" },
  };
  const greedNeedle = document.getElementById("msGreedNeedle");
  const greedFill = document.getElementById("msGreedFill");
  const greedValue = document.getElementById("msGreedValue");
  const greedLabel = document.getElementById("msGreedLabel");
  document.querySelectorAll(".ms-greed-tab").forEach((tab) => {
    tab.addEventListener("click", () => {
      document.querySelectorAll(".ms-greed-tab").forEach((t) => t.classList.remove("active"));
      tab.classList.add("active");
      const data = GREED[tab.dataset.greed];
      if (!data) return;
      if (greedNeedle) greedNeedle.style.left = data.value + "%";
      if (greedFill) greedFill.style.width = data.value + "%";
      if (greedValue) greedValue.textContent = data.value;
      if (greedLabel) {
        greedLabel.textContent = data.label;
        greedLabel.className =
          "inline-block px-4 py-2 rounded-lg text-lg font-semibold " +
          (data.color === "violet" ? "bg-violet-500/15 text-violet-500" : "bg-emerald-500/15 text-emerald-500");
      }
    });
  });

  /* 05. Range pill groups */
  function wirePillGroup(selector) {
    const pills = document.querySelectorAll(selector);
    pills.forEach((pill) => {
      pill.addEventListener("click", () => {
        pills.forEach((p) => {
          p.classList.remove("active", "bg-accent", "text-white", "border-accent");
          p.classList.add("text-muted");
        });
        pill.classList.add("active", "bg-accent", "text-white", "border-accent");
        pill.classList.remove("text-muted");
      });
    });
  }
  wirePillGroup(".ms-range");
  wirePillGroup(".ms-history-range");
  // seed initial active styling
  document.querySelectorAll(".ms-range.active, .ms-history-range.active").forEach((p) => {
    p.classList.add("bg-accent", "text-white", "border-accent");
    p.classList.remove("text-muted");
  });

  /* 06. Export menu */
  const exportBtn = document.getElementById("msExportBtn");
  const exportMenu = document.getElementById("msExportMenu");
  exportBtn?.addEventListener("click", (e) => {
    e.stopPropagation();
    exportMenu?.classList.toggle("hidden");
  });
  document.addEventListener("click", () => exportMenu?.classList.add("hidden"));
  document.querySelectorAll(".ms-export").forEach((item) => {
    item.addEventListener("click", () => {
      showToast("Export Started", `Exporting data as ${item.dataset.export.toUpperCase()}...`);
      exportMenu?.classList.add("hidden");
    });
  });

  /* 07. Drawers */
  const overlay = document.getElementById("msOverlay");
  const detailDrawer = document.getElementById("msDrawer");
  const filterDrawer = document.getElementById("msFilterDrawer");
  const drawerTitle = document.getElementById("msDrawerTitle");

  function openDrawerEl(el) {
    overlay?.classList.add("active");
    el?.classList.add("active");
  }
  function closeDrawers() {
    overlay?.classList.remove("active");
    detailDrawer?.classList.remove("active");
    filterDrawer?.classList.remove("active");
  }
  overlay?.addEventListener("click", closeDrawers);
  document.querySelectorAll(".ms-drawer-close").forEach((b) => b.addEventListener("click", closeDrawers));
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") closeDrawers();
  });

  function showPanel(name) {
    document.querySelectorAll(".ms-panel").forEach((p) => {
      p.classList.toggle("hidden", p.dataset.panel !== name);
    });
  }

  // Asset detail data
  const ASSETS = {
    btc: { name: "Bitcoin", pair: "BTC/USD", glyph: "BTC", iconWrap: "bg-orange-500/20", glyphColor: "text-orange-500", badge: "Extreme Greed", badgeCls: "bg-violet-500/15 text-violet-500", score: 85, change: "+12.5", changeCls: "text-emerald-500", long: "78%", volume: "2.4M" },
    eurusd: { name: "EUR/USD", pair: "Euro vs Dollar", glyph: "€$", iconWrap: "bg-blue-500/20", glyphColor: "text-blue-500", badge: "Bullish", badgeCls: "bg-emerald-500/15 text-emerald-500", score: 72, change: "+5.2", changeCls: "text-emerald-500", long: "68%", volume: "890K" },
    eth: { name: "Ethereum", pair: "ETH/USD", glyph: "Ξ", iconWrap: "bg-purple-500/20", glyphColor: "text-purple-500", badge: "Greed", badgeCls: "bg-emerald-500/15 text-emerald-500", score: 79, change: "+8.7", changeCls: "text-emerald-500", long: "72%", volume: "1.8M" },
    xrp: { name: "Ripple", pair: "XRP/USD", glyph: "XRP", iconWrap: "bg-red-500/20", glyphColor: "text-red-500", badge: "Bearish", badgeCls: "bg-red-500/15 text-red-500", score: 32, change: "-15.8", changeCls: "text-red-500", long: "35%", volume: "1.2M" },
  };

  function populateAsset(key) {
    const a = ASSETS[key];
    if (!a) return;
    const iconWrap = document.getElementById("msAssetIconWrap");
    const glyph = document.getElementById("msAssetGlyph");
    if (iconWrap) iconWrap.className = "w-16 h-16 rounded-2xl flex items-center justify-center shrink-0 " + a.iconWrap;
    if (glyph) {
      glyph.textContent = a.glyph;
      glyph.className = "font-bold text-2xl " + a.glyphColor;
    }
    document.getElementById("msAssetName").textContent = a.name;
    document.getElementById("msAssetPair").textContent = a.pair;
    const badge = document.getElementById("msAssetBadge");
    badge.textContent = a.badge;
    badge.className = "px-2.5 py-1 rounded-lg text-xs font-semibold ml-auto shrink-0 " + a.badgeCls;
    document.getElementById("msAssetScore").textContent = a.score;
    const change = document.getElementById("msAssetChange");
    change.textContent = a.change;
    change.className = "text-2xl font-bold " + a.changeCls;
    document.getElementById("msAssetLong").textContent = a.long;
    document.getElementById("msAssetVolume").textContent = a.volume;
  }

  document.querySelectorAll(".ms-open-drawer").forEach((btn) => {
    btn.addEventListener("click", () => {
      const type = btn.dataset.drawer;
      if (type === "asset") {
        showPanel("asset");
        populateAsset(btn.dataset.asset);
        if (drawerTitle) drawerTitle.textContent = (ASSETS[btn.dataset.asset]?.name || "Asset") + " Sentiment Analysis";
      } else {
        showPanel("movers");
        if (drawerTitle) drawerTitle.textContent = "All Top Movers";
      }
      openDrawerEl(detailDrawer);
      refreshIcons();
    });
  });

  // Filter drawer
  document.getElementById("msFilterBtn")?.addEventListener("click", () => openDrawerEl(filterDrawer));
  function wireSingleSelect(selector) {
    const pills = document.querySelectorAll(selector);
    pills.forEach((pill) =>
      pill.addEventListener("click", () => {
        pills.forEach((p) => {
          p.classList.remove("active", "bg-accent/15", "text-accent", "border-accent");
          p.classList.add("text-muted");
        });
        pill.classList.add("active", "bg-accent/15", "text-accent", "border-accent");
        pill.classList.remove("text-muted");
      })
    );
  }
  wireSingleSelect(".ms-tf-pill");
  // sentiment pills are independent toggles
  document.querySelectorAll(".ms-sent-pill").forEach((pill) =>
    pill.addEventListener("click", () => {
      pill.classList.toggle("active");
      pill.classList.toggle("bg-accent/15");
      pill.classList.toggle("text-accent");
      pill.classList.toggle("border-accent");
      pill.classList.toggle("text-muted");
    })
  );
  document.getElementById("msApplyFilters")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Filters Applied", "Sentiment data has been filtered");
  });

  /* 08. Movers search + type filter */
  const movers = Array.from(document.querySelectorAll(".ms-mover"));
  const moversSearch = document.getElementById("msMoversSearch");
  let activeMoverFilter = "all";
  function applyMoverFilters() {
    const q = (moversSearch?.value || "").trim().toLowerCase();
    movers.forEach((m) => {
      const matchType = activeMoverFilter === "all" || m.dataset.type === activeMoverFilter;
      const matchQ = !q || (m.dataset.name || "").includes(q);
      m.classList.toggle("hidden", !(matchType && matchQ));
    });
  }
  document.querySelectorAll(".ms-movers-filter").forEach((pill) =>
    pill.addEventListener("click", () => {
      document.querySelectorAll(".ms-movers-filter").forEach((p) => {
        p.classList.remove("active", "bg-accent/15", "text-accent", "border-accent");
        p.classList.add("text-muted");
      });
      pill.classList.add("active", "bg-accent/15", "text-accent", "border-accent");
      pill.classList.remove("text-muted");
      activeMoverFilter = pill.dataset.filter;
      applyMoverFilters();
    })
  );
  moversSearch?.addEventListener("input", applyMoverFilters);

  /* 09. Charts */
  if (typeof Chart === "undefined") return;
  const charts = [];

  const trendCtx = document.getElementById("msTrendChart");
  if (trendCtx) {
    charts.push(
      new Chart(trendCtx, {
        type: "line",
        data: {
          labels: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
          datasets: [
            { label: "Overall", data: [65, 68, 72, 70, 75, 73, 72], borderColor: "#6366F1", backgroundColor: "rgba(99,102,241,0.1)", fill: true, tension: 0.4, borderWidth: 2 },
            { label: "Forex", data: [60, 62, 68, 65, 70, 68, 68], borderColor: "#0ea5e9", backgroundColor: "rgba(14,165,233,0.1)", fill: true, tension: 0.4, borderWidth: 2 },
            { label: "Crypto", data: [70, 74, 76, 75, 80, 78, 78], borderColor: "#10b981", backgroundColor: "rgba(16,185,129,0.1)", fill: true, tension: 0.4, borderWidth: 2 },
          ],
        },
        options: {
          responsive: true,
          maintainAspectRatio: false,
          interaction: { intersect: false, mode: "index" },
          plugins: {
            legend: { position: "top", align: "end", labels: { color: tickColor(), usePointStyle: true, pointStyle: "circle", padding: 16, font: { size: 12 } } },
            tooltip: { backgroundColor: "rgba(0,0,0,0.8)", padding: 12, titleColor: "#fff", bodyColor: "#fff" },
          },
          scales: {
            y: { min: 0, max: 100, grid: { color: gridColor() }, ticks: { color: tickColor() } },
            x: { grid: { display: false }, ticks: { color: tickColor() } },
          },
        },
      })
    );
  }

  const distCtx = document.getElementById("msDistChart");
  if (distCtx) {
    charts.push(
      new Chart(distCtx, {
        type: "doughnut",
        data: {
          labels: ["Bullish", "Bearish", "Neutral"],
          datasets: [{ data: [45, 28, 27], backgroundColor: ["#10b981", "#ef4444", "#6b7280"], borderWidth: 0, hoverOffset: 10 }],
        },
        options: {
          responsive: true,
          maintainAspectRatio: false,
          plugins: {
            legend: { display: false },
            tooltip: { backgroundColor: "rgba(0,0,0,0.8)", padding: 12, callbacks: { label: (c) => `${c.label}: ${c.parsed}%` } },
          },
          cutout: "70%",
        },
      })
    );
  }

  const socialCtx = document.getElementById("msSocialChart");
  if (socialCtx) {
    charts.push(
      new Chart(socialCtx, {
        type: "bar",
        data: {
          labels: ["Twitter", "Reddit", "Telegram", "Discord", "YouTube"],
          datasets: [
            {
              label: "Mentions (K)",
              data: [4200, 1800, 986, 654, 432],
              backgroundColor: ["rgba(99,102,241,0.8)", "rgba(249,115,22,0.8)", "rgba(139,92,246,0.8)", "rgba(16,185,129,0.7)", "rgba(239,68,68,0.8)"],
              borderRadius: 8,
            },
          ],
        },
        options: {
          responsive: true,
          maintainAspectRatio: false,
          plugins: { legend: { display: false }, tooltip: { backgroundColor: "rgba(0,0,0,0.8)", padding: 12 } },
          scales: {
            y: { beginAtZero: true, grid: { color: gridColor() }, ticks: { color: tickColor(), callback: (v) => v / 1000 + "K" } },
            x: { grid: { display: false }, ticks: { color: tickColor() } },
          },
        },
      })
    );
  }

  const newsCtx = document.getElementById("msNewsChart");
  if (newsCtx) {
    charts.push(
      new Chart(newsCtx, {
        type: "pie",
        data: {
          labels: ["Positive", "Negative", "Neutral"],
          datasets: [{ data: [55, 20, 25], backgroundColor: ["#10b981", "#ef4444", "#6b7280"], borderWidth: 0 }],
        },
        options: {
          responsive: true,
          maintainAspectRatio: false,
          plugins: { legend: { position: "bottom", labels: { color: tickColor(), usePointStyle: true, padding: 15 } } },
        },
      })
    );
  }

  const historyCtx = document.getElementById("msHistoryChart");
  if (historyCtx) {
    charts.push(
      new Chart(historyCtx, {
        type: "line",
        data: {
          labels: ["Oct 1", "Oct 15", "Nov 1", "Nov 15", "Dec 1", "Dec 15", "Dec 28"],
          datasets: [
            { label: "Overall Sentiment", data: [35, 42, 48, 55, 62, 75, 72], borderColor: "#6366F1", backgroundColor: "rgba(99,102,241,0.1)", fill: true, tension: 0.4, borderWidth: 3 },
            { label: "Fear & Greed", data: [25, 38, 45, 52, 58, 78, 72], borderColor: "#f59e0b", backgroundColor: "rgba(245,158,11,0.05)", fill: true, tension: 0.4, borderWidth: 2, borderDash: [5, 5] },
          ],
        },
        options: {
          responsive: true,
          maintainAspectRatio: false,
          interaction: { intersect: false, mode: "index" },
          plugins: {
            legend: { position: "top", align: "end", labels: { color: tickColor(), usePointStyle: true, padding: 16 } },
            tooltip: { backgroundColor: "rgba(0,0,0,0.8)", padding: 12 },
          },
          scales: {
            y: { min: 0, max: 100, grid: { color: gridColor() }, ticks: { color: tickColor() } },
            x: { grid: { display: false }, ticks: { color: tickColor() } },
          },
        },
      })
    );
  }

  function recolorCharts() {
    charts.forEach((chart) => {
      if (chart.options.scales?.y) {
        chart.options.scales.y.grid.color = gridColor();
        chart.options.scales.y.ticks.color = tickColor();
      }
      if (chart.options.scales?.x) chart.options.scales.x.ticks.color = tickColor();
      if (chart.options.plugins?.legend?.labels) chart.options.plugins.legend.labels.color = tickColor();
      chart.update();
    });
  }
  document.getElementById("themeToggle")?.addEventListener("click", () => setTimeout(recolorCharts, 50));
})();
