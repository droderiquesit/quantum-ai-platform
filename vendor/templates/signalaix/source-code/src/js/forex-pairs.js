/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: forex-pairs.html
 * description: SignalAIX - Forex Pairs Watchlist Page Controller
 *              Self-contained; mirrors the reference mockup's functionality
 *              (category tabs, trend/volatility filter panel, search, sort,
 *               session filter, grid/table view toggle, favorite/star toggle,
 *               table row selection + select-all, per-pair details drawer with a
 *               real Chart.js price chart, add-pair drawer, new-alert drawer,
 *               export menu, refresh, toast). Eight per-pair mini sparkline
 *               charts in grid view + four in the table view.
 *              All markup lives in forex-pairs.html; this file only modifies the
 *              DOM (text/values/classes/visibility, reorder existing nodes) and
 *              renders Chart.js — it never injects HTML strings.
 * author: SignalAIX

    -------------------------------------------------
     01. Init & DOM refs (guarded by #forex-pairs)
     -------------------------------------------------
     02. Toast
     -------------------------------------------------
     03. Drawers (details / add / alert)
     -------------------------------------------------
     04. Tabs + filter panel + search + session (grid/row filtering)
     -------------------------------------------------
     05. Sort + view toggle
     -------------------------------------------------
     06. Favorite/star toggle + selection (row checkboxes, select-all)
     -------------------------------------------------
     07. Details drawer populate + per-open chart
     -------------------------------------------------
     08. Add pair / Create alert / Export / Refresh
     -------------------------------------------------
     09. Mini charts + theme re-color + keyboard shortcuts
     -------------------------------------------------
    ================================================== */

document.addEventListener("DOMContentLoaded", () => {
  /* ======================================
   * 01. Init & DOM refs
   * ====================================== */
  const page = document.getElementById("forex-pairs");
  if (!page) return; // Guard: only run on the Forex Pairs page

  const refreshIcons = () => window.lucide?.createIcons?.();
  const hasChart = () => typeof Chart !== "undefined";

  // Static per-pair metadata (mirrors the mockup's pairData; used to populate the
  // details drawer + the per-open chart via DOM updates only).
  const PAIRS = {
    EURUSD: { name: "EUR/USD", fullName: "Euro / US Dollar", price: "1.0892", change: "+0.42%", up: true, signal: "Strong Buy", spread: "0.8", high: "1.0915", low: "1.0845", volume: "2.4M", series: [1.082, 1.0835, 1.0848, 1.086, 1.0872, 1.0883, 1.0892] },
    GBPUSD: { name: "GBP/USD", fullName: "British Pound / US Dollar", price: "1.2734", change: "+0.28%", up: true, signal: "Buy", spread: "1.2", high: "1.2768", low: "1.2695", volume: "1.8M", series: [1.2698, 1.2705, 1.2712, 1.272, 1.2726, 1.273, 1.2734] },
    USDJPY: { name: "USD/JPY", fullName: "US Dollar / Japanese Yen", price: "149.82", change: "-0.35%", up: false, signal: "Sell", spread: "0.9", high: "150.45", low: "149.52", volume: "2.1M", series: [150.35, 150.2, 150.05, 149.95, 149.9, 149.86, 149.82] },
    USDCHF: { name: "USD/CHF", fullName: "US Dollar / Swiss Franc", price: "0.8845", change: "+0.02%", up: true, signal: "Neutral", spread: "1.5", high: "0.8872", low: "0.8820", volume: "1.1M", series: [0.8842, 0.8847, 0.8843, 0.8846, 0.8844, 0.8846, 0.8845] },
    AUDUSD: { name: "AUD/USD", fullName: "Australian Dollar / US Dollar", price: "0.6542", change: "+0.58%", up: true, signal: "Strong Buy", spread: "1.0", high: "0.6568", low: "0.6498", volume: "1.4M", series: [0.6504, 0.6512, 0.6521, 0.6528, 0.6534, 0.6539, 0.6542] },
    NZDUSD: { name: "NZD/USD", fullName: "New Zealand Dollar / US Dollar", price: "0.6085", change: "+0.32%", up: true, signal: "Buy", spread: "1.8", high: "0.6102", low: "0.6058", volume: "0.9M", series: [0.6066, 0.607, 0.6075, 0.6079, 0.6082, 0.6084, 0.6085] },
    EURGBP: { name: "EUR/GBP", fullName: "Euro / British Pound", price: "0.8553", change: "-0.18%", up: false, signal: "Sell", spread: "1.4", high: "0.8578", low: "0.8542", volume: "0.8M", series: [0.8568, 0.8564, 0.856, 0.8557, 0.8555, 0.8554, 0.8553] },
    USDTRY: { name: "USD/TRY", fullName: "US Dollar / Turkish Lira", price: "32.458", change: "+0.85%", up: true, signal: "Exotic", spread: "85.0", high: "32.512", low: "32.145", volume: "0.5M", series: [32.18, 32.24, 32.31, 32.37, 32.41, 32.44, 32.458] },
  };

  const overlay = document.getElementById("fpDrawerOverlay");
  const detailsDrawer = document.getElementById("fpDetailsDrawer");
  const addDrawer = document.getElementById("fpAddDrawer");
  const alertDrawer = document.getElementById("fpAlertDrawer");
  const allDrawers = [detailsDrawer, addDrawer, alertDrawer];

  const toast = document.getElementById("fpToast");
  const toastTitle = document.getElementById("fpToastTitle");
  const toastMessage = document.getElementById("fpToastMessage");

  const gridView = document.getElementById("fpGridView");
  const tableView = document.getElementById("fpTableView");
  const tbody = document.getElementById("fpTableBody");
  const noResults = document.getElementById("fpNoResults");
  const searchInput = document.getElementById("fpSearch");

  const cards = Array.from(document.querySelectorAll(".fp-card"));
  const rows = Array.from(document.querySelectorAll(".fp-row"));

  // Filter state shared by tabs + filter panel + search
  let activeTab = "all"; // all | major | minor | exotic | favorites
  const advFilters = { trend: "all", vol: "all" };
  let searchTerm = "";
  let currentView = "grid";

  /* ======================================
   * 02. Toast
   * ====================================== */
  let toastTimer = null;
  function showToast(title, message) {
    if (toastTitle) toastTitle.textContent = title;
    if (toastMessage) toastMessage.textContent = message;
    toast?.classList.add("active");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(hideToast, 3500);
  }
  function hideToast() {
    toast?.classList.remove("active");
  }
  document.querySelector(".fp-toast-close")?.addEventListener("click", hideToast);

  /* ======================================
   * 03. Drawers (details / add / alert)
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
  document.querySelectorAll(".fp-close-drawer").forEach((btn) => btn.addEventListener("click", closeDrawers));
  document.getElementById("fpAddPairBtn")?.addEventListener("click", () => openDrawer(addDrawer));
  document.getElementById("fpSetAlertBtn")?.addEventListener("click", () => openDrawer(alertDrawer));

  /* ======================================
   * 04. Tabs + filter panel + search + session
   * ====================================== */
  function isFavorite(el) {
    // Grid card: the star button lives inside the card. Table row: same.
    return !!el.querySelector(".fp-star.active");
  }

  function matchesFilters(el) {
    const type = el.dataset.type;
    const trend = el.dataset.trend;
    const vol = el.dataset.vol;
    const pair = (el.dataset.pair || "").toLowerCase();
    const text = el.textContent.toLowerCase();

    const tabMatch =
      activeTab === "all" ||
      activeTab === type ||
      (activeTab === "favorites" && isFavorite(el));

    const trendMatch = advFilters.trend === "all" || trend === advFilters.trend;
    const volMatch = advFilters.vol === "all" || vol === advFilters.vol;
    const searchMatch = !searchTerm || pair.includes(searchTerm) || text.includes(searchTerm);

    return tabMatch && trendMatch && volMatch && searchMatch;
  }

  function applyFilters() {
    let visible = 0;
    cards.forEach((card) => {
      const show = matchesFilters(card);
      card.style.display = show ? "" : "none";
      if (show) visible += 1;
    });
    rows.forEach((row) => {
      row.style.display = matchesFilters(row) ? "" : "none";
    });
    if (noResults) noResults.classList.toggle("hidden", visible !== 0);
  }

  // Tabs
  document.querySelectorAll(".fp-tab").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll(".fp-tab").forEach((b) => {
        b.classList.remove("active");
        b.classList.add("text-muted");
      });
      btn.classList.add("active");
      btn.classList.remove("text-muted");
      activeTab = btn.dataset.tab;
      applyFilters();
    });
  });

  // Filter panel toggle
  document.getElementById("fpFilterToggle")?.addEventListener("click", () => {
    document.getElementById("fpFilterPanel")?.classList.toggle("hidden");
  });

  // Filter option groups (single-select per group)
  document.querySelectorAll(".fp-filter-group").forEach((group) => {
    const key = group.dataset.group;
    group.querySelectorAll(".fp-filter-opt").forEach((opt) => {
      opt.addEventListener("click", () => {
        group.querySelectorAll(".fp-filter-opt").forEach((o) => {
          o.classList.remove("active", "border-accent", "bg-accent/15", "text-accent");
          o.classList.add("border-border", "bg-panel", "text-muted");
        });
        opt.classList.add("active", "border-accent", "bg-accent/15", "text-accent");
        opt.classList.remove("border-border", "bg-panel", "text-muted");
        if (key) advFilters[key] = opt.dataset.value;
        applyFilters();
      });
    });
  });

  // Search
  searchInput?.addEventListener("input", (e) => {
    searchTerm = e.target.value.toLowerCase();
    applyFilters();
  });

  // Session filter (informational toast, mirrors mockup)
  document.getElementById("fpSession")?.addEventListener("change", (e) => {
    const v = e.target.value;
    showToast("Session Filter", `Showing pairs active during ${v === "all" ? "all sessions" : v + " session"}`);
  });

  /* ======================================
   * 05. Sort + view toggle
   * ====================================== */
  function reorder(parent, items, cmp) {
    items.slice().sort(cmp).forEach((node) => parent.appendChild(node));
  }
  function nameOf(el) {
    return el.dataset.pair || "";
  }
  const VOL_RANK = { high: 3, medium: 2, low: 1 };
  function sortBy(value) {
    const cmp = {
      name: (a, b) => nameOf(a).localeCompare(nameOf(b)),
      change: (a, b) => parseFloat(b.dataset.change) - parseFloat(a.dataset.change),
      volatility: (a, b) => (VOL_RANK[b.dataset.vol] || 0) - (VOL_RANK[a.dataset.vol] || 0),
    }[value];
    if (!cmp) return;
    if (gridView) reorder(gridView, cards, cmp);
    if (tbody) reorder(tbody, rows, cmp);
  }
  document.getElementById("fpSort")?.addEventListener("change", (e) => {
    sortBy(e.target.value);
    showToast("Sorting", `Pairs sorted by ${e.target.value}`);
  });

  document.querySelectorAll(".fp-view-toggle").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll(".fp-view-toggle").forEach((b) => {
        b.classList.remove("active", "text-accent", "bg-accent/10");
        b.classList.add("text-muted");
      });
      btn.classList.add("active", "text-accent", "bg-accent/10");
      btn.classList.remove("text-muted");
      currentView = btn.dataset.view;
      if (currentView === "grid") {
        gridView?.classList.remove("hidden");
        tableView?.classList.add("hidden");
      } else {
        gridView?.classList.add("hidden");
        tableView?.classList.remove("hidden");
        initTableCharts();
      }
    });
  });
  document.querySelector('.fp-view-toggle[data-view="grid"]')?.classList.add("text-accent", "bg-accent/10");

  /* ======================================
   * 06. Favorite/star toggle + selection
   * ====================================== */
  function setStarState(btn, on) {
    const icon = btn.querySelector("i");
    if (on) {
      btn.classList.add("active", "border-amber-500", "bg-amber-500/15", "text-amber-500");
      btn.classList.remove("border-border", "bg-panel", "text-muted", "hover:text-amber-500", "hover:border-amber-500/50");
      icon?.classList.add("fill-current");
    } else {
      btn.classList.remove("active", "border-amber-500", "bg-amber-500/15", "text-amber-500");
      btn.classList.add("border-border", "bg-panel", "text-muted", "hover:text-amber-500", "hover:border-amber-500/50");
      icon?.classList.remove("fill-current");
    }
  }

  document.querySelectorAll(".fp-star").forEach((btn) => {
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      const on = !btn.classList.contains("active");
      setStarState(btn, on);
      refreshIcons();
      showToast(on ? "Added to Watchlist" : "Removed from Watchlist", on ? "Pair added to your favorites" : "Pair removed from your favorites");
      if (activeTab === "favorites") applyFilters();
    });
  });

  // Table row selection
  const selectAll = document.getElementById("fpSelectAll");
  const rowChecks = Array.from(document.querySelectorAll(".fp-row-check"));

  function toggleCheck(cb, on) {
    if (on) {
      cb.classList.add("checked");
      cb.setAttribute("aria-checked", "true");
    } else {
      cb.classList.remove("checked");
      cb.setAttribute("aria-checked", "false");
    }
  }
  function updateSelectAll() {
    const checked = rowChecks.filter((cb) => cb.classList.contains("checked")).length;
    toggleCheck(selectAll, checked === rowChecks.length && rowChecks.length > 0);
  }
  rowChecks.forEach((cb) => {
    const onToggle = () => {
      toggleCheck(cb, !cb.classList.contains("checked"));
      updateSelectAll();
    };
    cb.addEventListener("click", onToggle);
    cb.addEventListener("keydown", (e) => {
      if (e.key === " " || e.key === "Enter") {
        e.preventDefault();
        onToggle();
      }
    });
  });
  selectAll?.addEventListener("click", () => {
    const checkAll = !selectAll.classList.contains("checked");
    rowChecks.forEach((cb) => toggleCheck(cb, checkAll));
    toggleCheck(selectAll, checkAll);
  });
  selectAll?.addEventListener("keydown", (e) => {
    if (e.key === " " || e.key === "Enter") {
      e.preventDefault();
      selectAll.click();
    }
  });

  /* ======================================
   * 07. Details drawer populate + per-open chart
   * ====================================== */
  const dName = document.getElementById("fpDetailName");
  const dFullName = document.getElementById("fpDetailFullName");
  const dPrice = document.getElementById("fpDetailPrice");
  const dChange = document.getElementById("fpDetailChange");
  const dPriceBox = document.getElementById("fpDetailPriceBox");
  const dTrendIconWrap = document.getElementById("fpDetailTrendIconWrap");
  const dTrendIcon = document.getElementById("fpDetailTrendIcon");
  const dSignalBadge = document.getElementById("fpDetailSignal");
  const dSpread = document.getElementById("fpDetailSpread");
  const dVolume = document.getElementById("fpDetailVolume");
  const dHigh = document.getElementById("fpDetailHigh");
  const dLow = document.getElementById("fpDetailLow");

  let detailChart = null;
  let currentPair = "EURUSD";

  function setClasses(el, remove, add) {
    if (!el) return;
    el.classList.remove(...remove);
    el.classList.add(...add);
  }

  function openDetails(key) {
    const p = PAIRS[key];
    if (!p) return;
    currentPair = key;
    const up = p.up;

    if (dName) dName.textContent = p.name;
    if (dFullName) dFullName.textContent = p.fullName;
    if (dPrice) dPrice.textContent = p.price;
    if (dChange) dChange.textContent = p.change;
    if (dSpread) dSpread.textContent = p.spread;
    if (dVolume) dVolume.textContent = p.volume;
    if (dHigh) dHigh.textContent = p.high;
    if (dLow) dLow.textContent = p.low;

    // Price box gradient + change color
    setClasses(
      dPriceBox,
      ["from-emerald-500/15", "to-teal-500/10", "border-emerald-500/20", "from-red-500/15", "to-orange-500/10", "border-red-500/20"],
      up ? ["from-emerald-500/15", "to-teal-500/10", "border-emerald-500/20"] : ["from-red-500/15", "to-orange-500/10", "border-red-500/20"]
    );
    setClasses(dChange, ["text-emerald-500", "text-red-500"], [up ? "text-emerald-500" : "text-red-500"]);
    setClasses(dTrendIconWrap, ["bg-emerald-500/15", "text-emerald-500", "bg-red-500/15", "text-red-500"], up ? ["bg-emerald-500/15", "text-emerald-500"] : ["bg-red-500/15", "text-red-500"]);
    dTrendIcon?.setAttribute("data-lucide", up ? "trending-up" : "trending-down");

    // Signal badge
    setClasses(
      dSignalBadge,
      ["bg-emerald-500/15", "text-emerald-500", "bg-red-500/15", "text-red-500"],
      up ? ["bg-emerald-500/15", "text-emerald-500"] : ["bg-red-500/15", "text-red-500"]
    );
    if (dSignalBadge) dSignalBadge.textContent = up ? "Bullish" : "Bearish";

    openDrawer(detailsDrawer);
    renderDetailChart(p);
  }

  function renderDetailChart(p) {
    const canvas = document.getElementById("fpDetailChart");
    if (!canvas || !hasChart()) return;
    if (detailChart) {
      detailChart.destroy();
      detailChart = null;
    }
    requestAnimationFrame(() => {
      const isLight = !document.documentElement.classList.contains("dark");
      const tick = isLight ? "#64748B" : "#94A3B8";
      const color = p.up ? "#10b981" : "#ef4444";
      detailChart = new Chart(canvas.getContext("2d"), {
        type: "line",
        data: {
          labels: p.series.map((_, i) => i + 1),
          datasets: [
            {
              data: p.series,
              borderColor: color,
              backgroundColor: color + "1f",
              borderWidth: 2,
              fill: true,
              tension: 0.4,
              pointRadius: 0,
              pointHoverRadius: 5,
            },
          ],
        },
        options: {
          responsive: true,
          maintainAspectRatio: false,
          plugins: { legend: { display: false } },
          scales: {
            x: { display: false, grid: { display: false } },
            y: {
              display: true,
              grid: { color: "rgba(148,163,184,0.1)" },
              ticks: { color: tick, font: { size: 10 } },
            },
          },
          interaction: { intersect: false, mode: "index" },
        },
      });
    });
  }

  document.querySelectorAll(".fp-details-btn").forEach((btn) =>
    btn.addEventListener("click", () => openDetails(btn.dataset.pair))
  );

  // Details drawer actions
  document.querySelector(".fp-detail-trade")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Trade", `Opening trade position for ${PAIRS[currentPair]?.name || ""}...`);
  });
  document.querySelector(".fp-detail-alert")?.addEventListener("click", () => {
    if (document.getElementById("fpAlertPair")) {
      const sel = document.getElementById("fpAlertPair");
      const want = PAIRS[currentPair]?.name;
      Array.from(sel.options).forEach((o) => { if (o.textContent === want) o.selected = true; });
    }
    openDrawer(alertDrawer);
  });

  // Table row alert buttons
  document.querySelectorAll(".fp-row-alert").forEach((btn) =>
    btn.addEventListener("click", () => {
      const key = btn.dataset.pair;
      if (document.getElementById("fpAlertPair")) {
        const sel = document.getElementById("fpAlertPair");
        const want = PAIRS[key]?.name;
        Array.from(sel.options).forEach((o) => { if (o.textContent === want) o.selected = true; });
      }
      openDrawer(alertDrawer);
    })
  );

  /* ======================================
   * 08. Add pair / Create alert / Export / Refresh
   * ====================================== */
  document.querySelectorAll(".fp-add-avail").forEach((btn) =>
    btn.addEventListener("click", () => {
      closeDrawers();
      showToast("Pair Added", `${btn.dataset.pair} added to your watchlist`);
    })
  );
  document.querySelector(".fp-create-alert")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Alert Created", "Your price alert has been set");
  });

  // Export menu
  const exportBtn = document.getElementById("fpExportBtn");
  const exportMenu = document.getElementById("fpExportMenu");
  exportBtn?.addEventListener("click", (e) => {
    e.stopPropagation();
    exportMenu?.classList.toggle("hidden");
  });
  document.addEventListener("click", () => exportMenu?.classList.add("hidden"));
  document.querySelectorAll(".fp-export-item").forEach((item) =>
    item.addEventListener("click", () => {
      const fmt = (item.dataset.format || "csv").toUpperCase();
      exportMenu?.classList.add("hidden");
      showToast("Export Started", `Exporting pairs as ${fmt}...`);
    })
  );

  // Refresh
  document.getElementById("fpRefreshBtn")?.addEventListener("click", () => {
    showToast("Refreshing", "Updating market data...");
    setTimeout(() => showToast("Updated", "Market data refreshed successfully"), 1500);
  });

  /* ======================================
   * 09. Mini charts + theme re-color + keyboard shortcuts
   * ====================================== */
  const miniCharts = [];
  function makeMini(canvas, series, up) {
    const color = up ? "#10b981" : "#ef4444";
    return new Chart(canvas.getContext("2d"), {
      type: "line",
      data: {
        labels: series.map((_, i) => i),
        datasets: [{ data: series, borderColor: color, backgroundColor: color + "1f", borderWidth: 2, fill: true, tension: 0.4, pointRadius: 0 }],
      },
      options: {
        responsive: true,
        maintainAspectRatio: false,
        plugins: { legend: { display: false }, tooltip: { enabled: false } },
        scales: { x: { display: false }, y: { display: false } },
      },
    });
  }
  function initGridCharts() {
    if (!hasChart()) return;
    Object.keys(PAIRS).forEach((key) => {
      const canvas = document.getElementById(`fpChart-${key}`);
      if (canvas && !canvas.dataset.init) {
        canvas.dataset.init = "1";
        miniCharts.push(makeMini(canvas, PAIRS[key].series, PAIRS[key].up));
      }
    });
  }
  function initTableCharts() {
    if (!hasChart()) return;
    Object.keys(PAIRS).forEach((key) => {
      const canvas = document.getElementById(`fpTableChart-${key}`);
      if (canvas && !canvas.dataset.init) {
        canvas.dataset.init = "1";
        miniCharts.push(makeMini(canvas, PAIRS[key].series, PAIRS[key].up));
      }
    });
  }

  document.getElementById("themeToggle")?.addEventListener("click", () => {
    setTimeout(() => {
      if (detailChart && detailsDrawer?.classList.contains("active")) {
        const isLight = !document.documentElement.classList.contains("dark");
        const tick = isLight ? "#64748B" : "#94A3B8";
        if (detailChart.options?.scales?.y?.ticks) detailChart.options.scales.y.ticks.color = tick;
        detailChart.update();
      }
    }, 50);
  });

  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") closeDrawers();
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
      e.preventDefault();
      searchInput?.focus();
    }
  });

  // Initial render
  initGridCharts();
  applyFilters();
  refreshIcons();
});
