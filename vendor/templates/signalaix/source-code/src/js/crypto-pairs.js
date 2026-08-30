/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: crypto-pairs.html
 * description: SignalAIX - Crypto Pairs Watchlist Page Controller
 *              Self-contained; mirrors the reference mockup's functionality
 *              (category tabs, signal/market-cap filter panel, search, sort,
 *               exchange filter, grid/table view toggle, favorite/star toggle,
 *               table row selection + select-all, per-pair details drawer with a
 *               real Chart.js price chart + AI signal analysis, add-pair drawer,
 *               price-alert drawer, export menu, refresh, toast). Nine per-pair
 *               mini sparkline charts in grid view + nine in the table view.
 *              All markup lives in crypto-pairs.html; this file only modifies the
 *              DOM (text/values/classes/visibility, reorder existing nodes) and
 *              renders Chart.js — it never injects HTML strings.
 * author: SignalAIX

    -------------------------------------------------
     01. Init & DOM refs (guarded by #crypto-pairs)
     -------------------------------------------------
     02. Toast
     -------------------------------------------------
     03. Drawers (details / add / alert)
     -------------------------------------------------
     04. Tabs + filter panel + search + exchange (grid/row filtering)
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
  const page = document.getElementById("crypto-pairs");
  if (!page) return; // Guard: only run on the Crypto Pairs page

  const refreshIcons = () => window.lucide?.createIcons?.();
  const hasChart = () => typeof Chart !== "undefined";

  // Static per-pair metadata (mirrors the mockup's cryptoData; used to populate the
  // details drawer + the per-open chart via DOM updates only).
  const PAIRS = {
    BTCUSDT: { name: "BTC/USDT", fullName: "Bitcoin", icon: "BT", color: "#F7931A", price: "$67,432.50", change: "+2.45%", up: true, signal: "Buy", high: "$68,500", low: "$65,800", volume: "$28.5B", mcap: "$1.32T", strength: 85, series: [65800, 66100, 66400, 66200, 66900, 67200, 67432] },
    ETHUSDT: { name: "ETH/USDT", fullName: "Ethereum", icon: "ET", color: "#627EEA", price: "$3,542.80", change: "-1.23%", up: false, signal: "Hold", high: "$3,620", low: "$3,480", volume: "$15.2B", mcap: "$425B", strength: 62, series: [3590, 3580, 3565, 3558, 3550, 3548, 3542] },
    SOLUSDT: { name: "SOL/USDT", fullName: "Solana", icon: "SO", color: "#9945FF", price: "$148.35", change: "+5.82%", up: true, signal: "Strong Buy", high: "$152", low: "$138", volume: "$4.8B", mcap: "$65B", strength: 92, series: [139, 141, 143.5, 145, 146.5, 147.5, 148.35] },
    BNBUSDT: { name: "BNB/USDT", fullName: "BNB", icon: "BN", color: "#F0B90B", price: "$584.20", change: "+3.67%", up: true, signal: "Buy", high: "$595", low: "$562", volume: "$2.1B", mcap: "$87B", strength: 78, series: [564, 569, 573, 577, 580, 582, 584.2] },
    XRPUSDT: { name: "XRP/USDT", fullName: "Ripple", icon: "XR", color: "#23292F", price: "$0.5234", change: "-0.45%", up: false, signal: "Hold", high: "$0.535", low: "$0.518", volume: "$1.8B", mcap: "$28B", strength: 55, series: [0.526, 0.5255, 0.525, 0.5245, 0.524, 0.5237, 0.5234] },
    AVAXUSDT: { name: "AVAX/USDT", fullName: "Avalanche", icon: "AV", color: "#E84142", price: "$35.42", change: "+4.21%", up: true, signal: "Buy", high: "$36.80", low: "$33.90", volume: "$1.2B", mcap: "$14B", strength: 76, series: [34, 34.4, 34.8, 35, 35.2, 35.3, 35.42] },
    DOGEUSDT: { name: "DOGE/USDT", fullName: "Dogecoin", icon: "DO", color: "#C2A633", price: "$0.1245", change: "-2.34%", up: false, signal: "Sell", high: "$0.129", low: "$0.121", volume: "$1.5B", mcap: "$18B", strength: 35, series: [0.1275, 0.127, 0.1262, 0.1256, 0.125, 0.1248, 0.1245] },
    LINKUSDT: { name: "LINK/USDT", fullName: "Chainlink", icon: "LI", color: "#2A5ADA", price: "$14.52", change: "+3.45%", up: true, signal: "Buy", high: "$15.10", low: "$13.95", volume: "$892M", mcap: "$8.5B", strength: 74, series: [14, 14.1, 14.25, 14.35, 14.42, 14.48, 14.52] },
    INJUSDT: { name: "INJ/USDT", fullName: "Injective", icon: "IN", color: "#00A3A3", price: "$24.56", change: "+7.89%", up: true, signal: "Strong Buy", high: "$25.80", low: "$22.50", volume: "$678M", mcap: "$2.3B", strength: 91, series: [22.8, 23.2, 23.7, 24, 24.2, 24.4, 24.56] },
  };

  const overlay = document.getElementById("cpDrawerOverlay");
  const detailsDrawer = document.getElementById("cpDetailsDrawer");
  const addDrawer = document.getElementById("cpAddDrawer");
  const alertDrawer = document.getElementById("cpAlertDrawer");
  const allDrawers = [detailsDrawer, addDrawer, alertDrawer];

  const toast = document.getElementById("cpToast");
  const toastTitle = document.getElementById("cpToastTitle");
  const toastMessage = document.getElementById("cpToastMessage");

  const gridView = document.getElementById("cpGridView");
  const tableView = document.getElementById("cpTableView");
  const tbody = document.getElementById("cpTableBody");
  const noResults = document.getElementById("cpNoResults");
  const searchInput = document.getElementById("cpSearch");

  const cards = Array.from(document.querySelectorAll(".cp-card"));
  const rows = Array.from(document.querySelectorAll(".cp-row"));

  // Filter state shared by tabs + filter panel + search
  let activeTab = "all"; // all | btc | eth | defi | favorites
  const advFilters = { signal: "all", cap: "all" };
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
  document.querySelector(".cp-toast-close")?.addEventListener("click", hideToast);

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
  document.querySelectorAll(".cp-close-drawer").forEach((btn) => btn.addEventListener("click", closeDrawers));
  document.getElementById("cpAddPairBtn")?.addEventListener("click", () => openDrawer(addDrawer));
  document.getElementById("cpSetAlertBtn")?.addEventListener("click", () => openDrawer(alertDrawer));

  /* ======================================
   * 04. Tabs + filter panel + search + exchange
   * ====================================== */
  function isFavorite(el) {
    return !!el.querySelector(".cp-star.active");
  }

  function matchesFilters(el) {
    const type = el.dataset.type;
    const signal = el.dataset.signal;
    const cap = el.dataset.cap;
    const pair = (el.dataset.pair || "").toLowerCase();
    const text = el.textContent.toLowerCase();

    const tabMatch =
      activeTab === "all" ||
      activeTab === type ||
      (activeTab === "favorites" && isFavorite(el));

    const signalMatch = advFilters.signal === "all" || signal === advFilters.signal;
    const capMatch = advFilters.cap === "all" || cap === advFilters.cap;
    const searchMatch = !searchTerm || pair.includes(searchTerm) || text.includes(searchTerm);

    return tabMatch && signalMatch && capMatch && searchMatch;
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
  document.querySelectorAll(".cp-tab").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll(".cp-tab").forEach((b) => {
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
  document.getElementById("cpFilterToggle")?.addEventListener("click", () => {
    document.getElementById("cpFilterPanel")?.classList.toggle("hidden");
  });

  // Filter option groups (single-select per group)
  document.querySelectorAll(".cp-filter-group").forEach((group) => {
    const key = group.dataset.group;
    group.querySelectorAll(".cp-filter-opt").forEach((opt) => {
      opt.addEventListener("click", () => {
        group.querySelectorAll(".cp-filter-opt").forEach((o) => {
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

  // Exchange filter (informational toast, mirrors mockup)
  document.getElementById("cpExchange")?.addEventListener("change", (e) => {
    const v = e.target.value;
    showToast("Exchange Filter", `Showing pairs from ${v === "all" ? "all exchanges" : v}`);
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
  function sortBy(value) {
    const cmp = {
      rank: (a, b) => nameOf(a).localeCompare(nameOf(b)),
      price: (a, b) => parseFloat(b.dataset.price) - parseFloat(a.dataset.price),
      change: (a, b) => parseFloat(b.dataset.change) - parseFloat(a.dataset.change),
      strength: (a, b) => parseFloat(b.dataset.strength) - parseFloat(a.dataset.strength),
    }[value];
    if (!cmp) return;
    if (gridView) reorder(gridView, cards, cmp);
    if (tbody) reorder(tbody, rows, cmp);
  }
  document.getElementById("cpSort")?.addEventListener("change", (e) => {
    sortBy(e.target.value);
    showToast("Sorting", `Pairs sorted by ${e.target.value}`);
  });

  document.querySelectorAll(".cp-view-toggle").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll(".cp-view-toggle").forEach((b) => {
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
  document.querySelector('.cp-view-toggle[data-view="grid"]')?.classList.add("text-accent", "bg-accent/10");

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

  document.querySelectorAll(".cp-star").forEach((btn) => {
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
  const selectAll = document.getElementById("cpSelectAll");
  const rowChecks = Array.from(document.querySelectorAll(".cp-row-check"));

  function toggleCheck(cb, on) {
    if (!cb) return;
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
  const dIcon = document.getElementById("cpDetailIcon");
  const dName = document.getElementById("cpDetailName");
  const dFullName = document.getElementById("cpDetailFullName");
  const dPrice = document.getElementById("cpDetailPrice");
  const dChange = document.getElementById("cpDetailChange");
  const dPriceBox = document.getElementById("cpDetailPriceBox");
  const dTrendIconWrap = document.getElementById("cpDetailTrendIconWrap");
  const dTrendIcon = document.getElementById("cpDetailTrendIcon");
  const dSignalBadge = document.getElementById("cpDetailSignal");
  const dSignalBadge2 = document.getElementById("cpDetailSignal2");
  const dHigh = document.getElementById("cpDetailHigh");
  const dLow = document.getElementById("cpDetailLow");
  const dVolume = document.getElementById("cpDetailVolume");
  const dMcap = document.getElementById("cpDetailMcap");
  const dStrengthVal = document.getElementById("cpDetailStrengthVal");
  const dStrengthBar = document.getElementById("cpDetailStrengthBar");

  let detailChart = null;
  let currentPair = "BTCUSDT";

  function setClasses(el, remove, add) {
    if (!el) return;
    el.classList.remove(...remove);
    el.classList.add(...add);
  }

  // Pick a semantic color tuple for the AI signal badge by signal text.
  function signalBadgeClasses(signal) {
    if (signal === "Strong Buy" || signal === "Buy") return ["bg-emerald-500/15", "text-emerald-500"];
    if (signal === "Sell" || signal === "Strong Sell") return ["bg-red-500/15", "text-red-500"];
    return ["bg-amber-500/15", "text-amber-500"];
  }
  const ALL_BADGE = ["bg-emerald-500/15", "text-emerald-500", "bg-red-500/15", "text-red-500", "bg-amber-500/15", "text-amber-500"];

  function strengthColor(v) {
    if (v >= 70) return "bg-emerald-500";
    if (v >= 50) return "bg-amber-500";
    return "bg-red-500";
  }

  function openDetails(key) {
    const p = PAIRS[key];
    if (!p) return;
    currentPair = key;
    const up = p.up;

    if (dIcon) {
      dIcon.textContent = p.icon;
      dIcon.style.background = p.color;
    }
    if (dName) dName.textContent = p.name;
    if (dFullName) dFullName.textContent = p.fullName;
    if (dPrice) dPrice.textContent = p.price;
    if (dChange) dChange.textContent = p.change;
    if (dHigh) dHigh.textContent = p.high;
    if (dLow) dLow.textContent = p.low;
    if (dVolume) dVolume.textContent = p.volume;
    if (dMcap) dMcap.textContent = p.mcap;
    if (dStrengthVal) dStrengthVal.textContent = p.strength + "%";
    if (dStrengthBar) {
      dStrengthBar.style.width = p.strength + "%";
      setClasses(dStrengthBar, ["bg-emerald-500", "bg-amber-500", "bg-red-500"], [strengthColor(p.strength)]);
    }

    // Price box gradient + change color
    setClasses(
      dPriceBox,
      ["from-emerald-500/15", "to-teal-500/10", "border-emerald-500/20", "from-red-500/15", "to-orange-500/10", "border-red-500/20"],
      up ? ["from-emerald-500/15", "to-teal-500/10", "border-emerald-500/20"] : ["from-red-500/15", "to-orange-500/10", "border-red-500/20"]
    );
    setClasses(dChange, ["text-emerald-500", "text-red-500"], [up ? "text-emerald-500" : "text-red-500"]);
    setClasses(dTrendIconWrap, ["bg-emerald-500/15", "text-emerald-500", "bg-red-500/15", "text-red-500"], up ? ["bg-emerald-500/15", "text-emerald-500"] : ["bg-red-500/15", "text-red-500"]);
    dTrendIcon?.setAttribute("data-lucide", up ? "trending-up" : "trending-down");

    // Signal badges (both top + AI analysis)
    const badge = signalBadgeClasses(p.signal);
    setClasses(dSignalBadge, ALL_BADGE, badge);
    setClasses(dSignalBadge2, ALL_BADGE, badge);
    if (dSignalBadge) dSignalBadge.textContent = p.signal;
    if (dSignalBadge2) dSignalBadge2.textContent = p.signal;

    openDrawer(detailsDrawer);
    renderDetailChart(p);
  }

  function renderDetailChart(p) {
    const canvas = document.getElementById("cpDetailChart");
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

  document.querySelectorAll(".cp-details-btn").forEach((btn) =>
    btn.addEventListener("click", () => openDetails(btn.dataset.pair))
  );

  // Detail chart range buttons (informational; series is static)
  document.querySelectorAll(".cp-range-btn").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll(".cp-range-btn").forEach((b) => {
        b.classList.remove("active", "bg-accent/15", "text-accent");
        b.classList.add("text-muted");
      });
      btn.classList.add("active", "bg-accent/15", "text-accent");
      btn.classList.remove("text-muted");
    });
  });

  // Details drawer actions
  function selectAlertPair(key) {
    const sel = document.getElementById("cpAlertPair");
    if (!sel) return;
    const want = PAIRS[key]?.name;
    Array.from(sel.options).forEach((o) => { if (o.textContent === want) o.selected = true; });
  }
  document.querySelector(".cp-detail-trade")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Order Submitted", `Opening trade for ${PAIRS[currentPair]?.name || ""}...`);
  });
  document.querySelector(".cp-detail-alert")?.addEventListener("click", () => {
    selectAlertPair(currentPair);
    openDrawer(alertDrawer);
  });

  // Table row alert buttons
  document.querySelectorAll(".cp-row-alert").forEach((btn) =>
    btn.addEventListener("click", () => {
      selectAlertPair(btn.dataset.pair);
      openDrawer(alertDrawer);
    })
  );

  /* ======================================
   * 08. Add pair / Create alert / Export / Refresh
   * ====================================== */
  document.querySelectorAll(".cp-add-avail").forEach((btn) =>
    btn.addEventListener("click", () => {
      closeDrawers();
      showToast("Pair Added", `${btn.dataset.pair} added to your watchlist`);
    })
  );
  document.querySelector(".cp-add-confirm")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Pair Added", "New pair added to your watchlist");
  });
  document.querySelector(".cp-create-alert")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Alert Created", "You will be notified when conditions are met");
  });

  // Export menu
  const exportBtn = document.getElementById("cpExportBtn");
  const exportMenu = document.getElementById("cpExportMenu");
  exportBtn?.addEventListener("click", (e) => {
    e.stopPropagation();
    exportMenu?.classList.toggle("hidden");
  });
  document.addEventListener("click", () => exportMenu?.classList.add("hidden"));
  document.querySelectorAll(".cp-export-item").forEach((item) =>
    item.addEventListener("click", () => {
      const fmt = (item.dataset.format || "csv").toUpperCase();
      exportMenu?.classList.add("hidden");
      showToast("Export Started", `Exporting pairs as ${fmt}...`);
    })
  );

  // Refresh
  document.getElementById("cpRefreshBtn")?.addEventListener("click", () => {
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
      const canvas = document.getElementById(`cpChart-${key}`);
      if (canvas && !canvas.dataset.init) {
        canvas.dataset.init = "1";
        miniCharts.push(makeMini(canvas, PAIRS[key].series, PAIRS[key].up));
      }
    });
  }
  function initTableCharts() {
    if (!hasChart()) return;
    Object.keys(PAIRS).forEach((key) => {
      const canvas = document.getElementById(`cpTableChart-${key}`);
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
