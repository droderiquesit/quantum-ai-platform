/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: open-positions.html
 * description: SignalAIX - Open Positions Page Controller
 *              Self-contained; mirrors the reference mockup's functionality
 *              (category tabs, quick-filter chips, search, sort, table/card
 *               view toggle, row/all selection + bulk actions, position-details
 *               drawer with a real per-open price chart, modify-position drawer,
 *               new-position drawer, filter drawer, export menu, toast).
 *              All markup lives in open-positions.html; this file only modifies
 *              the DOM (text/values/classes/visibility, reorder existing nodes)
 *              and renders Chart.js — it never injects HTML strings.
 * author: SignalAIX

    -------------------------------------------------
     01. Init & DOM refs (guarded by #open-positions)
     -------------------------------------------------
     02. Toast
     -------------------------------------------------
     03. Drawers (details / modify / new / filter)
     -------------------------------------------------
     04. Tabs + quick chips + search + filter-drawer state (row/card filtering)
     -------------------------------------------------
     05. Sort + view toggle
     -------------------------------------------------
     06. Selection (row checkboxes, select-all, bulk actions)
     -------------------------------------------------
     07. Details drawer populate + per-open chart
     -------------------------------------------------
     08. Modify / New / Export
     -------------------------------------------------
     09. Theme re-color + keyboard shortcuts
     -------------------------------------------------
    ================================================== */

document.addEventListener("DOMContentLoaded", () => {
  /* ======================================
   * 01. Init & DOM refs
   * ====================================== */
  const page = document.getElementById("open-positions");
  if (!page) return; // Guard: only run on the Open Positions page

  const html = document.documentElement;
  const refreshIcons = () => window.lucide?.createIcons?.();

  // Static per-position metadata (mirrors the mockup's positionsData; used to
  // populate the drawers and the per-open chart via DOM updates only).
  const POSITIONS = {
    POS001: { symbol: "EUR/USD", market: "forex", type: "long", size: "1.5", lev: "1:30", entry: "1.0842", current: "1.0912", pnl: 1050, pnlText: "+$1,050", pnlPct: "+6.45%", tp: "1.0950", sl: "1.0780", risk: "low", signal: "AI", duration: "3h 24m", opened: "2025-01-15 09:30:00", series: [1.0842, 1.0848, 1.0851, 1.0860, 1.0872, 1.0895, 1.0912] },
    POS002: { symbol: "BTC/USD", market: "crypto", type: "long", size: "0.25", lev: "1:10", entry: "$42,500.00", current: "$43,250.00", pnl: 1875, pnlText: "+$1,875", pnlPct: "+1.76%", tp: "$45,000.00", sl: "$41,000.00", risk: "medium", signal: "AI", duration: "8h 15m", opened: "2025-01-15 04:45:00", series: [42500, 42620, 42710, 42880, 43010, 43180, 43250] },
    POS003: { symbol: "GBP/JPY", market: "forex", type: "short", size: "2", lev: "1:30", entry: "188.450", current: "187.820", pnl: 1260, pnlText: "+$1,260", pnlPct: "+0.33%", tp: "186.500", sl: "189.500", risk: "high", signal: "Manual", duration: "1h 45m", opened: "2025-01-15 11:15:00", series: [188.45, 188.3, 188.12, 188.0, 187.92, 187.86, 187.82] },
    POS004: { symbol: "ETH/USD", market: "crypto", type: "long", size: "2.5", lev: "1:10", entry: "$2,280.00", current: "$2,345.00", pnl: 1625, pnlText: "+$1,625", pnlPct: "+2.85%", tp: "$2,500.00", sl: "$2,150.00", risk: "medium", signal: "AI", duration: "5h 30m", opened: "2025-01-15 07:30:00", series: [2280, 2295, 2310, 2322, 2330, 2340, 2345] },
    POS005: { symbol: "USD/JPY", market: "forex", type: "short", size: "1", lev: "1:30", entry: "148.250", current: "148.650", pnl: -268, pnlText: "-$268", pnlPct: "-0.27%", tp: "147.000", sl: "149.000", risk: "low", signal: "AI", duration: "2h 10m", opened: "2025-01-15 10:50:00", series: [148.25, 148.32, 148.4, 148.48, 148.55, 148.6, 148.65] },
    POS006: { symbol: "XRP/USD", market: "crypto", type: "long", size: "5,000", lev: "1:5", entry: "0.5820", current: "0.6125", pnl: 1525, pnlText: "+$1,525", pnlPct: "+5.24%", tp: "0.6500", sl: "0.5500", risk: "high", signal: "AI", duration: "12h 20m", opened: "2025-01-15 00:40:00", series: [0.582, 0.59, 0.598, 0.604, 0.608, 0.611, 0.6125] },
    POS007: { symbol: "AUD/USD", market: "forex", type: "long", size: "2.5", lev: "1:30", entry: "0.6585", current: "0.6540", pnl: -1125, pnlText: "-$1,125", pnlPct: "-0.68%", tp: "0.6700", sl: "0.6480", risk: "medium", signal: "Manual", duration: "6h 55m", opened: "2025-01-15 06:05:00", series: [0.6585, 0.6578, 0.657, 0.6562, 0.6555, 0.6548, 0.654] },
    POS008: { symbol: "SOL/USD", market: "crypto", type: "short", size: "15", lev: "1:5", entry: "98.5000", current: "95.8000", pnl: 405, pnlText: "+$405", pnlPct: "+2.74%", tp: "90.0000", sl: "105.0000", risk: "high", signal: "AI", duration: "4h 40m", opened: "2025-01-15 08:20:00", series: [98.5, 97.9, 97.4, 96.9, 96.4, 96.0, 95.8] },
    POS009: { symbol: "EUR/GBP", market: "forex", type: "long", size: "1.8", lev: "1:30", entry: "0.8542", current: "0.8568", pnl: 468, pnlText: "+$468", pnlPct: "+0.30%", tp: "0.8620", sl: "0.8480", risk: "low", signal: "AI", duration: "1h 30m", opened: "2025-01-15 11:30:00", series: [0.8542, 0.8548, 0.8552, 0.8558, 0.8562, 0.8566, 0.8568] },
    POS010: { symbol: "DOGE/USD", market: "crypto", type: "long", size: "10,000", lev: "1:5", entry: "0.0825", current: "0.0798", pnl: -270, pnlText: "-$270", pnlPct: "-3.27%", tp: "0.0950", sl: "0.0750", risk: "high", signal: "Manual", duration: "9h 15m", opened: "2025-01-15 03:45:00", series: [0.0825, 0.0819, 0.0814, 0.081, 0.0805, 0.0801, 0.0798] },
  };
  const ALL_IDS = Object.keys(POSITIONS);

  const overlay = document.getElementById("opDrawerOverlay");
  const detailsDrawer = document.getElementById("opDetailsDrawer");
  const modifyDrawer = document.getElementById("opModifyDrawer");
  const newDrawer = document.getElementById("opNewDrawer");
  const filterDrawer = document.getElementById("opFilterDrawer");
  const allDrawers = [detailsDrawer, modifyDrawer, newDrawer, filterDrawer];

  const toast = document.getElementById("opToast");
  const toastTitle = document.getElementById("opToastTitle");
  const toastMessage = document.getElementById("opToastMessage");

  const tableView = document.getElementById("opTableView");
  const cardView = document.getElementById("opCardView");
  const tbody = document.getElementById("opTableBody");
  const rows = Array.from(document.querySelectorAll(".op-row"));
  const cards = Array.from(document.querySelectorAll(".op-card"));
  const noResults = document.getElementById("opNoResults");
  const searchInput = document.getElementById("opSearch");

  // Filter state shared by tabs + chips + search + filter drawer
  let activeTab = "all"; // all | forex | crypto | profit | loss
  let activeChip = "all"; // all | long | short | high-risk | ai-signal
  let searchTerm = "";
  const advFilters = { market: "all", type: "all", pnl: "all", risk: "all" };
  let currentView = "table";

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
  document.querySelector(".op-toast-close")?.addEventListener("click", hideToast);

  /* ======================================
   * 03. Drawers (details / modify / new / filter)
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
  document.querySelectorAll(".op-close-drawer").forEach((btn) =>
    btn.addEventListener("click", closeDrawers)
  );
  document.getElementById("opFilterBtn")?.addEventListener("click", () => openDrawer(filterDrawer));
  document.getElementById("opNewPositionBtn")?.addEventListener("click", () => openDrawer(newDrawer));

  /* ======================================
   * 04. Tabs + quick chips + search + filter-drawer state
   * ====================================== */
  function matchesFilters(el) {
    const market = el.dataset.market;
    const type = el.dataset.type;
    const risk = el.dataset.risk;
    const signal = el.dataset.signal;
    const pnl = parseFloat(el.dataset.pnl);
    const text = el.textContent.toLowerCase();

    // Tab
    const tabMatch =
      activeTab === "all" ||
      (activeTab === "forex" && market === "forex") ||
      (activeTab === "crypto" && market === "crypto") ||
      (activeTab === "profit" && pnl > 0) ||
      (activeTab === "loss" && pnl < 0);

    // Quick chip
    const chipMatch =
      activeChip === "all" ||
      (activeChip === "long" && type === "long") ||
      (activeChip === "short" && type === "short") ||
      (activeChip === "high-risk" && risk === "high") ||
      (activeChip === "ai-signal" && signal === "AI");

    // Advanced filter drawer
    const advMarket = advFilters.market === "all" || market === advFilters.market;
    const advType = advFilters.type === "all" || type === advFilters.type;
    const advRisk = advFilters.risk === "all" || risk === advFilters.risk;
    const advPnl =
      advFilters.pnl === "all" ||
      (advFilters.pnl === "profit" && pnl > 0) ||
      (advFilters.pnl === "loss" && pnl < 0);

    const searchMatch = !searchTerm || text.includes(searchTerm);

    return tabMatch && chipMatch && advMarket && advType && advRisk && advPnl && searchMatch;
  }

  function applyFilters() {
    let visible = 0;
    rows.forEach((row) => {
      const show = matchesFilters(row);
      row.style.display = show ? "" : "none";
      if (show) visible += 1;
    });
    cards.forEach((card) => {
      card.style.display = matchesFilters(card) ? "" : "none";
    });
    if (noResults) noResults.classList.toggle("hidden", visible !== 0);
  }

  // Tabs
  document.querySelectorAll(".op-tab").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll(".op-tab").forEach((b) => {
        b.classList.remove("active");
        b.classList.add("text-muted");
      });
      btn.classList.add("active");
      btn.classList.remove("text-muted");
      activeTab = btn.dataset.tab;
      applyFilters();
    });
  });

  // Quick filter chips
  document.querySelectorAll(".op-chip").forEach((chip) => {
    chip.addEventListener("click", () => {
      document.querySelectorAll(".op-chip").forEach((c) => {
        c.classList.remove("active", "border-accent", "bg-accent/15", "text-accent");
        c.classList.add("border-border", "bg-panel", "text-muted");
      });
      chip.classList.add("active", "border-accent", "bg-accent/15", "text-accent");
      chip.classList.remove("border-border", "bg-panel", "text-muted");
      activeChip = chip.dataset.filter;
      applyFilters();
    });
  });

  // Search
  searchInput?.addEventListener("input", (e) => {
    searchTerm = e.target.value.toLowerCase();
    applyFilters();
  });

  // Filter drawer option groups
  document.querySelectorAll(".op-filter-group").forEach((group) => {
    group.querySelectorAll(".op-filter-opt").forEach((opt) => {
      opt.addEventListener("click", () => {
        group.querySelectorAll(".op-filter-opt").forEach((o) => {
          o.classList.remove("active", "border-accent", "bg-accent/15", "text-accent");
          o.classList.add("border-border", "bg-panel", "text-muted");
        });
        opt.classList.add("active", "border-accent", "bg-accent/15", "text-accent");
        opt.classList.remove("border-border", "bg-panel", "text-muted");
      });
    });
  });

  document.querySelector(".op-apply-filters")?.addEventListener("click", () => {
    document.querySelectorAll(".op-filter-group").forEach((group) => {
      const g = group.dataset.group;
      const sel = group.querySelector(".op-filter-opt.active");
      if (g && sel) advFilters[g] = sel.dataset.value;
    });
    closeDrawers();
    applyFilters();
    showToast("Filters Applied", "Your filter settings have been applied");
  });

  document.querySelector(".op-reset-filters")?.addEventListener("click", () => {
    document.querySelectorAll(".op-filter-group").forEach((group) => {
      group.querySelectorAll(".op-filter-opt").forEach((o) => {
        const isAll = o.dataset.value === "all";
        o.classList.toggle("active", isAll);
        o.classList.toggle("border-accent", isAll);
        o.classList.toggle("bg-accent/15", isAll);
        o.classList.toggle("text-accent", isAll);
        o.classList.toggle("border-border", !isAll);
        o.classList.toggle("bg-panel", !isAll);
        o.classList.toggle("text-muted", !isAll);
      });
      if (group.dataset.group) advFilters[group.dataset.group] = "all";
    });
  });

  /* ======================================
   * 05. Sort + view toggle
   * ====================================== */
  const sortSelect = document.getElementById("opSort");
  function reorder(parent, items, cmp) {
    items.slice().sort(cmp).forEach((node) => parent.appendChild(node));
  }
  function sortBy(value) {
    const cmp = {
      newest: (a, b) => new Date(b.dataset.opened) - new Date(a.dataset.opened),
      oldest: (a, b) => new Date(a.dataset.opened) - new Date(b.dataset.opened),
      "profit-high": (a, b) => parseFloat(b.dataset.pnl) - parseFloat(a.dataset.pnl),
      "profit-low": (a, b) => parseFloat(a.dataset.pnl) - parseFloat(b.dataset.pnl),
      "amount-high": (a, b) => parseFloat(b.dataset.size.replace(/,/g, "")) - parseFloat(a.dataset.size.replace(/,/g, "")),
      "amount-low": (a, b) => parseFloat(a.dataset.size.replace(/,/g, "")) - parseFloat(b.dataset.size.replace(/,/g, "")),
    }[value];
    if (!cmp) return;
    if (tbody) reorder(tbody, rows, cmp);
    if (cardView) reorder(cardView, cards, cmp);
  }
  sortSelect?.addEventListener("change", (e) => sortBy(e.target.value));

  // Column-header sort buttons (toggle a few presets)
  const thState = {};
  document.querySelectorAll(".op-sort-th").forEach((btn) => {
    btn.addEventListener("click", () => {
      const key = btn.dataset.sort;
      const desc = !thState[key];
      thState[key] = desc;
      if (key === "symbol") {
        const cmp = (a, b) => {
          const av = a.querySelector(".font-semibold")?.textContent || "";
          const bv = b.querySelector(".font-semibold")?.textContent || "";
          return desc ? bv.localeCompare(av) : av.localeCompare(bv);
        };
        if (tbody) reorder(tbody, rows, cmp);
        if (cardView) reorder(cardView, cards, (a, b) => {
          const av = a.querySelector("h3")?.textContent || "";
          const bv = b.querySelector("h3")?.textContent || "";
          return desc ? bv.localeCompare(av) : av.localeCompare(bv);
        });
      } else if (key === "pnl") {
        sortBy(desc ? "profit-high" : "profit-low");
      } else if (key === "size") {
        sortBy(desc ? "amount-high" : "amount-low");
      }
    });
  });

  document.querySelectorAll(".op-view-toggle").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll(".op-view-toggle").forEach((b) => {
        b.classList.remove("active", "text-accent", "bg-accent/10");
        b.classList.add("text-muted");
      });
      btn.classList.add("active", "text-accent", "bg-accent/10");
      btn.classList.remove("text-muted");
      currentView = btn.dataset.view;
      if (currentView === "table") {
        tableView?.classList.remove("hidden");
        cardView?.classList.add("hidden");
        cardView?.classList.remove("grid");
      } else {
        tableView?.classList.add("hidden");
        cardView?.classList.remove("hidden");
        cardView?.classList.add("grid");
      }
    });
  });
  // Set initial active styling on table toggle
  document.querySelector('.op-view-toggle[data-view="table"]')?.classList.add("text-accent", "bg-accent/10");

  /* ======================================
   * 06. Selection (row checkboxes, select-all, bulk actions)
   * ====================================== */
  const selected = new Set();
  const selectAll = document.getElementById("opSelectAll");
  const selectedCount = document.getElementById("opSelectedCount");
  const bulkActions = document.getElementById("opBulkActions");

  function updateSelectionUI() {
    if (selectedCount) selectedCount.textContent = String(selected.size);
    bulkActions?.classList.toggle("hidden", selected.size === 0);
    bulkActions?.classList.toggle("flex", selected.size > 0);
    selectAll?.classList.toggle("checked", selected.size === ALL_IDS.length && selected.size > 0);
    selectAll?.setAttribute("aria-checked", selected.size === ALL_IDS.length && selected.size > 0 ? "true" : "false");
  }

  rows.forEach((row) => {
    const cb = row.querySelector(".op-checkbox");
    const id = row.dataset.id;
    function toggle() {
      if (selected.has(id)) {
        selected.delete(id);
        cb?.classList.remove("checked");
        cb?.setAttribute("aria-checked", "false");
      } else {
        selected.add(id);
        cb?.classList.add("checked");
        cb?.setAttribute("aria-checked", "true");
      }
      updateSelectionUI();
    }
    cb?.addEventListener("click", toggle);
    cb?.addEventListener("keydown", (e) => {
      if (e.key === " " || e.key === "Enter") {
        e.preventDefault();
        toggle();
      }
    });
  });

  selectAll?.addEventListener("click", () => {
    const checkAll = selected.size !== ALL_IDS.length;
    selected.clear();
    rows.forEach((row) => {
      const cb = row.querySelector(".op-checkbox");
      if (checkAll) {
        selected.add(row.dataset.id);
        cb?.classList.add("checked");
        cb?.setAttribute("aria-checked", "true");
      } else {
        cb?.classList.remove("checked");
        cb?.setAttribute("aria-checked", "false");
      }
    });
    updateSelectionUI();
  });

  document.getElementById("opBulkClose")?.addEventListener("click", () => {
    showToast("Positions Closed", `${selected.size} positions have been closed`);
    selected.clear();
    rows.forEach((row) => {
      const cb = row.querySelector(".op-checkbox");
      cb?.classList.remove("checked");
      cb?.setAttribute("aria-checked", "false");
    });
    updateSelectionUI();
  });
  document.getElementById("opBulkModify")?.addEventListener("click", () => {
    showToast("Bulk Modify", "Bulk modification feature coming soon");
  });

  /* ======================================
   * 07. Details drawer populate + per-open chart
   * ====================================== */
  const dIconWrap = document.getElementById("opDetailIconWrap");
  const dIcon = document.getElementById("opDetailIcon");
  const dSymbol = document.getElementById("opDetailSymbol");
  const dId = document.getElementById("opDetailId");
  const dPlBox = document.getElementById("opDetailPlBox");
  const dPlLabel = document.getElementById("opDetailPlLabel");
  const dPl = document.getElementById("opDetailPl");
  const dPlPct = document.getElementById("opDetailPlPct");
  const dTypeBadge = document.getElementById("opDetailTypeBadge");
  const dTypeIcon = document.getElementById("opDetailTypeIcon");
  const dTypeText = document.getElementById("opDetailTypeText");
  const dEntry = document.getElementById("opDetailEntry");
  const dCurrent = document.getElementById("opDetailCurrent");
  const dSize = document.getElementById("opDetailSize");
  const dLeverage = document.getElementById("opDetailLeverage");
  const dTp = document.getElementById("opDetailTp");
  const dSl = document.getElementById("opDetailSl");
  const dRiskDot = document.getElementById("opDetailRiskDot");
  const dRisk = document.getElementById("opDetailRisk");
  const dSignalBadge = document.getElementById("opDetailSignalBadge");
  const dSignalIcon = document.getElementById("opDetailSignalIcon");
  const dSignalText = document.getElementById("opDetailSignalText");
  const dDuration = document.getElementById("opDetailDuration");
  const dOpened = document.getElementById("opDetailOpened");
  const dMarket = document.getElementById("opDetailMarket");

  let detailChart = null;
  let currentId = "POS001";

  const RISK_BG = { low: "bg-emerald-500", medium: "bg-amber-500", high: "bg-red-500" };

  function setClasses(el, remove, add) {
    if (!el) return;
    el.classList.remove(...remove);
    el.classList.add(...add);
  }

  function openDetails(id) {
    const p = POSITIONS[id];
    if (!p) return;
    currentId = id;
    const profit = p.pnl >= 0;
    const isCrypto = p.market === "crypto";

    // Icon
    setClasses(dIconWrap, ["bg-indigo-500/15", "bg-amber-500/15"], [isCrypto ? "bg-amber-500/15" : "bg-indigo-500/15"]);
    setClasses(dIcon, ["text-indigo-500", "text-amber-500"], [isCrypto ? "text-amber-500" : "text-indigo-500"]);
    dIcon?.setAttribute("data-lucide", isCrypto ? "bitcoin" : "dollar-sign");

    if (dSymbol) dSymbol.textContent = p.symbol;
    if (dId) dId.textContent = `Position #${id}`;

    // P/L box + values
    setClasses(
      dPlBox,
      ["from-emerald-500/20", "to-teal-500/10", "from-red-500/20", "to-orange-500/10"],
      profit ? ["from-emerald-500/20", "to-teal-500/10"] : ["from-red-500/20", "to-orange-500/10"]
    );
    setClasses(dPlLabel, ["text-emerald-500", "text-red-500"], [profit ? "text-emerald-500" : "text-red-500"]);
    setClasses(dPl, ["text-emerald-500", "text-red-500"], [profit ? "text-emerald-500" : "text-red-500"]);
    setClasses(dPlPct, ["text-emerald-500", "text-red-500"], [profit ? "text-emerald-500" : "text-red-500"]);
    if (dPl) dPl.textContent = p.pnlText;
    if (dPlPct) dPlPct.textContent = p.pnlPct;

    // Type badge
    const long = p.type === "long";
    setClasses(
      dTypeBadge,
      ["bg-emerald-500/15", "text-emerald-500", "bg-red-500/15", "text-red-500"],
      long ? ["bg-emerald-500/15", "text-emerald-500"] : ["bg-red-500/15", "text-red-500"]
    );
    dTypeIcon?.setAttribute("data-lucide", long ? "arrow-up" : "arrow-down");
    if (dTypeText) dTypeText.textContent = p.type.toUpperCase();

    if (dEntry) dEntry.textContent = p.entry;
    if (dCurrent) dCurrent.textContent = p.current;
    if (dSize) dSize.textContent = p.size;
    if (dLeverage) dLeverage.textContent = p.lev;
    if (dTp) dTp.textContent = p.tp;
    if (dSl) dSl.textContent = p.sl;

    // Risk
    setClasses(dRiskDot, ["bg-emerald-500", "bg-amber-500", "bg-red-500"], [RISK_BG[p.risk] || "bg-emerald-500"]);
    if (dRisk) dRisk.textContent = p.risk;

    // Signal badge
    const isAI = p.signal === "AI";
    setClasses(
      dSignalBadge,
      ["bg-violet-500/15", "text-violet-400", "bg-sky-500/15", "text-sky-500"],
      isAI ? ["bg-violet-500/15", "text-violet-400"] : ["bg-sky-500/15", "text-sky-500"]
    );
    if (dSignalIcon) dSignalIcon.style.display = isAI ? "" : "none";
    if (isAI) dSignalIcon?.setAttribute("data-lucide", "sparkles");
    if (dSignalText) dSignalText.textContent = p.signal;

    if (dDuration) dDuration.textContent = p.duration;
    if (dOpened) dOpened.textContent = p.opened;
    if (dMarket) dMarket.textContent = p.market;

    openDrawer(detailsDrawer);
    renderDetailChart(p);
  }

  function renderDetailChart(p) {
    const canvas = document.getElementById("opDetailChart");
    if (!canvas || typeof Chart === "undefined") return;
    if (detailChart) {
      detailChart.destroy();
      detailChart = null;
    }
    requestAnimationFrame(() => {
      const color = p.pnl >= 0 ? "#10b981" : "#ef4444";
      detailChart = new Chart(canvas.getContext("2d"), {
        type: "line",
        data: {
          labels: p.series.map((_, i) => i + 1),
          datasets: [
            {
              data: p.series,
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
            x: { display: false, grid: { display: false } },
            y: { display: false, grid: { display: false } },
          },
        },
      });
    });
  }

  document.querySelectorAll(".op-view-btn").forEach((btn) =>
    btn.addEventListener("click", () => openDetails(btn.dataset.id))
  );

  /* ======================================
   * 08. Modify / New / Export
   * ====================================== */
  const mSubtitle = document.getElementById("opModifySubtitle");
  const mPlBox = document.getElementById("opModifyPlBox");
  const mPlLabel = document.getElementById("opModifyPlLabel");
  const mPl = document.getElementById("opModifyPl");
  const mTp = document.getElementById("opModifyTp");
  const mSl = document.getElementById("opModifySl");

  function openModify(id) {
    const p = POSITIONS[id];
    if (!p) return;
    currentId = id;
    const profit = p.pnl >= 0;
    if (mSubtitle) mSubtitle.textContent = `${p.symbol} - #${id}`;
    setClasses(mPlBox, ["bg-emerald-500/10", "bg-red-500/10"], [profit ? "bg-emerald-500/10" : "bg-red-500/10"]);
    setClasses(mPlLabel, ["text-emerald-500", "text-red-500"], [profit ? "text-emerald-500" : "text-red-500"]);
    setClasses(mPl, ["text-emerald-500", "text-red-500"], [profit ? "text-emerald-500" : "text-red-500"]);
    if (mPl) mPl.textContent = p.pnlText;
    if (mTp) mTp.value = p.tp;
    if (mSl) mSl.value = p.sl;
    openDrawer(modifyDrawer);
  }

  document.querySelectorAll(".op-edit-btn").forEach((btn) =>
    btn.addEventListener("click", () => openModify(btn.dataset.id))
  );

  // Details drawer actions
  document.querySelector(".op-detail-modify")?.addEventListener("click", () => openModify(currentId));
  document.querySelector(".op-detail-duplicate")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Position Duplicated", `${POSITIONS[currentId]?.symbol} position has been duplicated`);
  });
  document.querySelector(".op-detail-close")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Position Closed", `${POSITIONS[currentId]?.symbol} position closed at market price`);
  });

  // Close buttons on rows + cards
  document.querySelectorAll(".op-close-btn").forEach((btn) =>
    btn.addEventListener("click", () => {
      const p = POSITIONS[btn.dataset.id];
      showToast("Position Closed", `${p?.symbol || "Position"} closed at market price`);
    })
  );

  // Modify drawer interactions
  document.querySelectorAll(".op-partial-chip").forEach((chip) => {
    chip.addEventListener("click", () => {
      document.querySelectorAll(".op-partial-chip").forEach((c) => {
        c.classList.remove("active", "border-accent", "bg-accent/15", "text-accent");
        c.classList.add("border-border", "bg-panel", "text-muted");
      });
      chip.classList.add("active", "border-accent", "bg-accent/15", "text-accent");
      chip.classList.remove("border-border", "bg-panel", "text-muted");
    });
  });
  document.querySelector(".op-save-modify")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Position Updated", "Your position has been modified successfully");
  });

  // New position drawer interactions
  document.querySelectorAll(".op-newmarket-chip").forEach((chip) => {
    chip.addEventListener("click", () => {
      document.querySelectorAll(".op-newmarket-chip").forEach((c) => {
        c.classList.remove("active", "border-accent", "bg-accent/15", "text-accent");
        c.classList.add("border-border", "bg-panel", "text-muted");
      });
      chip.classList.add("active", "border-accent", "bg-accent/15", "text-accent");
      chip.classList.remove("border-border", "bg-panel", "text-muted");
    });
  });
  document.querySelectorAll(".op-size-chip").forEach((chip) => {
    chip.addEventListener("click", () => {
      document.querySelectorAll(".op-size-chip").forEach((c) => {
        c.classList.remove("active", "border-accent", "bg-accent/15", "text-accent");
        c.classList.add("border-border", "bg-panel", "text-muted");
      });
      chip.classList.add("active", "border-accent", "bg-accent/15", "text-accent");
      chip.classList.remove("border-border", "bg-panel", "text-muted");
    });
  });
  const longBtn = document.getElementById("opNewLong");
  const shortBtn = document.getElementById("opNewShort");
  longBtn?.addEventListener("click", () => {
    longBtn.classList.add("border-2", "border-emerald-500", "bg-emerald-500/10", "text-emerald-500");
    longBtn.classList.remove("border", "border-border", "text-text");
    shortBtn?.classList.remove("border-2", "border-red-500", "bg-red-500/10", "text-red-500");
    shortBtn?.classList.add("border", "border-border", "text-text");
  });
  shortBtn?.addEventListener("click", () => {
    shortBtn.classList.add("border-2", "border-red-500", "bg-red-500/10", "text-red-500");
    shortBtn.classList.remove("border", "border-border", "text-text");
    longBtn?.classList.remove("border-2", "border-emerald-500", "bg-emerald-500/10", "text-emerald-500");
    longBtn?.classList.add("border", "border-border", "text-text");
  });
  document.querySelector(".op-submit-new")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Position Opened", "Your new position has been opened successfully");
  });

  // Export menu
  const exportBtn = document.getElementById("opExportBtn");
  const exportMenu = document.getElementById("opExportMenu");
  exportBtn?.addEventListener("click", (e) => {
    e.stopPropagation();
    exportMenu?.classList.toggle("hidden");
  });
  document.addEventListener("click", () => exportMenu?.classList.add("hidden"));
  document.querySelectorAll(".op-export-item").forEach((item) => {
    item.addEventListener("click", () => {
      const fmt = (item.dataset.format || "csv").toUpperCase();
      exportMenu?.classList.add("hidden");
      showToast("Export Started", `Exporting positions as ${fmt}...`);
    });
  });

  /* ======================================
   * 09. Theme re-color + keyboard shortcuts
   * ====================================== */
  document.getElementById("themeToggle")?.addEventListener("click", () => {
    if (detailChart && detailsDrawer?.classList.contains("active")) {
      setTimeout(() => detailChart?.update(), 50);
    }
  });

  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") closeDrawers();
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
      e.preventDefault();
      searchInput?.focus();
    }
  });

  // Initial render
  applyFilters();
  updateSelectionUI();
  refreshIcons();
});
