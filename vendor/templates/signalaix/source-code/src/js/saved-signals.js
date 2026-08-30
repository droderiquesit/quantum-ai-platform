/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: saved-signals.html
 * description: SignalAIX - Saved Signals (Watchlists) Page Controller
 *              Self-contained; mirrors the reference mockup's functionality
 *              (category tabs, quick-filter chips, search, sort, grid/table view
 *               toggle, per-signal checkbox selection + bulk actions bar,
 *               delete/bulk-delete, per-signal detail drawer with a real Chart.js
 *               price chart, filter drawer with checkbox/range/date options,
 *               add-signal drawer, export menu, toast).
 *              All markup lives in saved-signals.html; this file only modifies the
 *              DOM (text/values/classes/visibility, reorder existing nodes) and
 *              renders Chart.js — it never injects HTML strings.
 * author: SignalAIX

    -------------------------------------------------
     01. Init & DOM refs (guarded by #saved-signals)
     -------------------------------------------------
     02. Signal data store
     -------------------------------------------------
     03. Toast
     -------------------------------------------------
     04. Drawers (detail / filter / add)
     -------------------------------------------------
     05. Tabs + quick filters + search (grid/row filtering)
     -------------------------------------------------
     06. Sort + view toggle
     -------------------------------------------------
     07. Selection (checkboxes, select-all, bulk bar)
     -------------------------------------------------
     08. Delete / bulk delete
     -------------------------------------------------
     09. Detail drawer populate + per-open chart
     -------------------------------------------------
     10. Filter drawer / Add signal / Export + theme re-color + shortcuts
     -------------------------------------------------
    ================================================== */

document.addEventListener("DOMContentLoaded", () => {
  /* ======================================
   * 01. Init & DOM refs
   * ====================================== */
  const page = document.getElementById("saved-signals");
  if (!page) return; // Guard: only run on the Saved Signals page

  const refreshIcons = () => window.lucide?.createIcons?.();
  const hasChart = () => typeof Chart !== "undefined";

  /* ======================================
   * 02. Signal data store (mirrors mockup signalsData)
   * ====================================== */
  const SIGNALS = {
    SIG001: { id: "SIG001", pair: "EUR/USD", type: "forex", action: "buy", entry: "1.0845", tp: "1.0920", sl: "1.0790", confidence: 87, status: "active", source: "ai", saved: "2h ago", notes: "Strong bullish momentum detected. Multiple timeframe confluence observed. RSI showing oversold conditions on H4.", analysis: "Price broke above key resistance at 1.0830. Expecting continuation towards 1.0920 target. Fundamental support from dovish Fed stance.", rr: "1:1.5", tf: "H4", tp1: "1.0880", tp2: "1.0920", tp3: "1.0960", series: [1.082, 1.0828, 1.0835, 1.0841, 1.0838, 1.0843, 1.0845] },
    SIG002: { id: "SIG002", pair: "BTC/USDT", type: "crypto", action: "sell", entry: "68,450", tp: "65,200", sl: "70,100", confidence: 72, status: "pending", source: "manual", saved: "5h ago", notes: "Double top formation on daily chart. Bearish divergence on RSI.", analysis: "Bitcoin showing signs of exhaustion at current levels. Volume declining on upward moves. Watch for breakdown below 67,000.", rr: "1:2", tf: "D1", tp1: "66,800", tp2: "65,200", tp3: "63,500", series: [69100, 68950, 68800, 68700, 68600, 68500, 68450] },
    SIG003: { id: "SIG003", pair: "GBP/JPY", type: "forex", action: "buy", entry: "191.45", tp: "193.80", sl: "190.20", confidence: 91, status: "tp1-hit", source: "ai", saved: "1d ago", notes: "TP1 reached successfully. Trail stop to entry.", analysis: "Strong risk-on sentiment supporting GBP. BoJ maintaining dovish stance. AI detected breakout pattern.", rr: "1:1.88", tf: "H1", tp1: "192.30", tp2: "193.00", tp3: "193.80", series: [190.6, 190.85, 191.05, 191.2, 191.32, 191.4, 191.45] },
    SIG004: { id: "SIG004", pair: "ETH/USDT", type: "crypto", action: "sell", entry: "3,845", tp: "3,620", sl: "3,980", confidence: 95, status: "completed", source: "ai", saved: "2d ago", notes: "All take profit levels hit. Trade closed successfully.", analysis: "Perfect execution. AI identified bearish flag pattern accurately.", rr: "1:1.67", tf: "H4", tp1: "3,750", tp2: "3,680", tp3: "3,620", series: [3920, 3905, 3890, 3878, 3865, 3852, 3845] },
    SIG005: { id: "SIG005", pair: "USD/CAD", type: "forex", action: "buy", entry: "1.3625", tp: "1.3780", sl: "1.3540", confidence: 78, status: "active", source: "manual", saved: "3d ago", notes: "Oil prices declining supporting USD/CAD upside.", analysis: "Fundamental play on oil weakness. Technical support at 1.3600 holding.", rr: "1:1.82", tf: "D1", tp1: "1.3680", tp2: "1.3730", tp3: "1.3780", series: [1.3588, 1.3596, 1.3604, 1.3611, 1.3617, 1.3622, 1.3625] },
    SIG006: { id: "SIG006", pair: "SOL/USDT", type: "crypto", action: "buy", entry: "185.50", tp: "205.00", sl: "172.00", confidence: 65, status: "sl-hit", source: "ai", saved: "4d ago", notes: "Stop loss triggered. Market reversed unexpectedly.", analysis: "Post-analysis: Broader market selloff caused by macro factors.", rr: "1:1.44", tf: "H4", tp1: "192.00", tp2: "198.00", tp3: "205.00", series: [180.2, 181.6, 182.9, 183.8, 184.5, 185.1, 185.5] },
  };

  const STATUS_LABEL = { active: "Active", pending: "Pending", completed: "Completed", "tp1-hit": "TP1 Hit", "sl-hit": "SL Hit" };
  const STATUS_COLOR = {
    active: ["bg-emerald-500/15", "text-emerald-500"],
    pending: ["bg-amber-500/15", "text-amber-500"],
    completed: ["bg-emerald-500/15", "text-emerald-500"],
    "tp1-hit": ["bg-blue-500/15", "text-blue-500"],
    "sl-hit": ["bg-red-500/15", "text-red-500"],
  };

  const overlay = document.getElementById("ssDrawerOverlay");
  const detailDrawer = document.getElementById("ssDetailDrawer");
  const filterDrawer = document.getElementById("ssFilterDrawer");
  const addDrawer = document.getElementById("ssAddDrawer");
  const allDrawers = [detailDrawer, filterDrawer, addDrawer];

  const toast = document.getElementById("ssToast");
  const toastTitle = document.getElementById("ssToastTitle");
  const toastMessage = document.getElementById("ssToastMessage");

  const gridView = document.getElementById("ssGridView");
  const tableView = document.getElementById("ssTableView");
  const tbody = document.getElementById("ssTableBody");
  const noResults = document.getElementById("ssNoResults");
  const searchInput = document.getElementById("ssSearch");

  let cards = Array.from(document.querySelectorAll(".ss-card"));
  let rows = Array.from(document.querySelectorAll(".ss-row"));

  // Filter state
  let activeTab = "all"; // all | forex | crypto | ai
  let quickFilter = "all"; // all | buy | sell | active | completed | high-confidence
  let searchTerm = "";
  let currentView = "grid";
  const selected = new Set();

  /* ======================================
   * 03. Toast
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
  document.querySelector(".ss-toast-close")?.addEventListener("click", hideToast);

  /* ======================================
   * 04. Drawers (detail / filter / add)
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
  document.querySelectorAll(".ss-close-drawer").forEach((btn) => btn.addEventListener("click", closeDrawers));
  document.getElementById("ssFilterBtn")?.addEventListener("click", () => openDrawer(filterDrawer));
  document.getElementById("ssAddSignalBtn")?.addEventListener("click", () => openDrawer(addDrawer));

  /* ======================================
   * 05. Tabs + quick filters + search
   * ====================================== */
  function matchesFilters(el) {
    const sig = SIGNALS[el.dataset.id];
    if (!sig) return false;

    const tabMatch =
      activeTab === "all" ||
      (activeTab === "forex" && sig.type === "forex") ||
      (activeTab === "crypto" && sig.type === "crypto") ||
      (activeTab === "ai" && sig.source === "ai");

    const quickMatch =
      quickFilter === "all" ||
      (quickFilter === "buy" && sig.action === "buy") ||
      (quickFilter === "sell" && sig.action === "sell") ||
      (quickFilter === "active" && sig.status === "active") ||
      (quickFilter === "completed" && sig.status === "completed") ||
      (quickFilter === "high-confidence" && sig.confidence >= 85);

    const searchMatch =
      !searchTerm ||
      sig.pair.toLowerCase().includes(searchTerm) ||
      sig.id.toLowerCase().includes(searchTerm) ||
      sig.notes.toLowerCase().includes(searchTerm);

    return tabMatch && quickMatch && searchMatch;
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

  // Category tabs
  document.querySelectorAll(".ss-tab").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll(".ss-tab").forEach((b) => {
        b.classList.remove("active");
        b.classList.add("text-muted");
      });
      btn.classList.add("active");
      btn.classList.remove("text-muted");
      activeTab = btn.dataset.tab;
      applyFilters();
    });
  });

  // Quick filter chips (single-select)
  document.querySelectorAll(".ss-chip").forEach((chip) => {
    chip.addEventListener("click", () => {
      document.querySelectorAll(".ss-chip").forEach((c) => {
        c.classList.remove("active", "border-accent", "bg-accent/15", "text-accent");
        c.classList.add("border-border", "bg-panel", "text-muted");
      });
      chip.classList.add("active", "border-accent", "bg-accent/15", "text-accent");
      chip.classList.remove("border-border", "bg-panel", "text-muted");
      quickFilter = chip.dataset.filter;
      applyFilters();
    });
  });

  // Search
  searchInput?.addEventListener("input", (e) => {
    searchTerm = e.target.value.toLowerCase();
    applyFilters();
  });

  /* ======================================
   * 06. Sort + view toggle
   * ====================================== */
  const AGE_RANK = { "2h ago": 1, "5h ago": 2, "1d ago": 3, "2d ago": 4, "3d ago": 5, "4d ago": 6 };
  function reorder(parent, items, cmp) {
    items.slice().sort(cmp).forEach((node) => parent.appendChild(node));
  }
  function sortBy(value) {
    const ageOf = (el) => AGE_RANK[SIGNALS[el.dataset.id]?.saved] || 0;
    const confOf = (el) => parseInt(el.dataset.confidence, 10) || 0;
    const pairOf = (el) => SIGNALS[el.dataset.id]?.pair || "";
    const cmp = {
      newest: (a, b) => ageOf(a) - ageOf(b),
      oldest: (a, b) => ageOf(b) - ageOf(a),
      "confidence-high": (a, b) => confOf(b) - confOf(a),
      "confidence-low": (a, b) => confOf(a) - confOf(b),
      "pair-asc": (a, b) => pairOf(a).localeCompare(pairOf(b)),
      "pair-desc": (a, b) => pairOf(b).localeCompare(pairOf(a)),
    }[value];
    if (!cmp) return;
    if (gridView) reorder(gridView, cards, cmp);
    if (tbody) reorder(tbody, rows, cmp);
  }
  document.getElementById("ssSort")?.addEventListener("change", (e) => {
    sortBy(e.target.value);
    showToast("Sorted", "Signals reordered");
  });

  document.querySelectorAll(".ss-view-toggle").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll(".ss-view-toggle").forEach((b) => {
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
      }
    });
  });
  document.querySelector('.ss-view-toggle[data-view="grid"]')?.classList.add("text-accent", "bg-accent/10");

  /* ======================================
   * 07. Selection (checkboxes, select-all, bulk bar)
   * ====================================== */
  const bulkBar = document.getElementById("ssBulkBar");
  const selectedCount = document.getElementById("ssSelectedCount");
  const tableSelectAll = document.getElementById("ssTableSelectAll");

  function setCheck(cb, on) {
    if (!cb) return;
    cb.classList.toggle("checked", on);
    cb.setAttribute("aria-checked", on ? "true" : "false");
  }

  function syncId(id, on) {
    document.querySelectorAll(`.ss-check[data-id="${id}"], .ss-row-check[data-id="${id}"]`).forEach((cb) => setCheck(cb, on));
  }

  function updateBulkBar() {
    if (selected.size > 0) {
      bulkBar?.classList.remove("hidden");
      if (selectedCount) selectedCount.textContent = String(selected.size);
    } else {
      bulkBar?.classList.add("hidden");
    }
  }

  function toggleSelection(id) {
    if (selected.has(id)) {
      selected.delete(id);
      syncId(id, false);
    } else {
      selected.add(id);
      syncId(id, true);
    }
    updateBulkBar();
  }

  function bindChecks() {
    document.querySelectorAll(".ss-check, .ss-row-check").forEach((cb) => {
      if (cb.dataset.bound) return;
      cb.dataset.bound = "1";
      const handler = (e) => {
        e.stopPropagation();
        toggleSelection(cb.dataset.id);
      };
      cb.addEventListener("click", handler);
      cb.addEventListener("keydown", (e) => {
        if (e.key === " " || e.key === "Enter") {
          e.preventDefault();
          handler(e);
        }
      });
    });
  }
  bindChecks();

  function selectAll(on) {
    rows.forEach((row) => {
      const id = row.dataset.id;
      if (on) selected.add(id);
      else selected.delete(id);
      syncId(id, on);
    });
    setCheck(tableSelectAll, on);
    updateBulkBar();
  }
  tableSelectAll?.addEventListener("click", () => selectAll(!tableSelectAll.classList.contains("checked")));
  tableSelectAll?.addEventListener("keydown", (e) => {
    if (e.key === " " || e.key === "Enter") {
      e.preventDefault();
      tableSelectAll.click();
    }
  });

  document.getElementById("ssClearSelectionBtn")?.addEventListener("click", () => {
    selected.forEach((id) => syncId(id, false));
    selected.clear();
    setCheck(tableSelectAll, false);
    updateBulkBar();
  });

  document.getElementById("ssBulkCheckbox")?.addEventListener("click", () => {
    selected.forEach((id) => syncId(id, false));
    selected.clear();
    setCheck(tableSelectAll, false);
    updateBulkBar();
  });

  document.getElementById("ssBulkExportBtn")?.addEventListener("click", () => {
    showToast("Export Started", `Exporting ${selected.size} selected signal(s)...`);
  });

  /* ======================================
   * 08. Delete / bulk delete
   * ====================================== */
  function deleteSignal(id) {
    document.querySelectorAll(`[data-id="${id}"]`).forEach((el) => {
      if (el.classList.contains("ss-card") || el.classList.contains("ss-row")) {
        el.remove();
      }
    });
    delete SIGNALS[id];
    selected.delete(id);
    cards = cards.filter((c) => c.dataset.id !== id);
    rows = rows.filter((r) => r.dataset.id !== id);
    updateBulkBar();
    applyFilters();
  }

  document.addEventListener("click", (e) => {
    const delBtn = e.target.closest?.(".ss-delete-btn");
    if (delBtn) {
      e.stopPropagation();
      deleteSignal(delBtn.dataset.id);
      showToast("Signal Deleted", "The signal has been removed from your saved list");
    }
  });

  document.getElementById("ssBulkDeleteBtn")?.addEventListener("click", () => {
    const count = selected.size;
    if (count === 0) return;
    Array.from(selected).forEach((id) => deleteSignal(id));
    selected.clear();
    updateBulkBar();
    showToast("Signals Deleted", `${count} signal(s) removed from your saved list`);
  });

  /* ======================================
   * 09. Detail drawer populate + per-open chart
   * ====================================== */
  const d = {
    iconWrap: document.getElementById("ssdIconWrap"),
    iconText: document.getElementById("ssdIconText"),
    iconCrypto: document.getElementById("ssdIconCrypto"),
    pair: document.getElementById("ssdPair"),
    action: document.getElementById("ssdAction"),
    status: document.getElementById("ssdStatus"),
    confText: document.getElementById("ssdConfText"),
    confBar: document.getElementById("ssdConfBar"),
    entry: document.getElementById("ssdEntry"),
    tp: document.getElementById("ssdTp"),
    sl: document.getElementById("ssdSl"),
    tp1: document.getElementById("ssdTp1"),
    tp2: document.getElementById("ssdTp2"),
    tp3: document.getElementById("ssdTp3"),
    rr: document.getElementById("ssdRr"),
    tf: document.getElementById("ssdTf"),
    sourceIcon: document.getElementById("ssdSourceIcon"),
    source: document.getElementById("ssdSource"),
    saved: document.getElementById("ssdSaved"),
    notes: document.getElementById("ssdNotes"),
    analysis: document.getElementById("ssdAnalysis"),
  };

  let detailChart = null;
  let currentSig = null;

  function setColor(el, classes) {
    if (!el) return;
    el.classList.remove("bg-emerald-500/15", "text-emerald-500", "bg-red-500/15", "text-red-500", "bg-amber-500/15", "text-amber-500", "bg-blue-500/15", "text-blue-500");
    el.classList.add(...classes);
  }

  function openDetail(id) {
    const sig = SIGNALS[id];
    if (!sig) return;
    currentSig = sig;
    const buy = sig.action === "buy";

    // Icon tile
    if (d.iconWrap) {
      d.iconWrap.classList.remove("from-emerald-500", "to-teal-500", "from-amber-500", "to-orange-500");
      if (sig.type === "crypto") d.iconWrap.classList.add("from-amber-500", "to-orange-500");
      else d.iconWrap.classList.add("from-emerald-500", "to-teal-500");
    }
    if (sig.type === "crypto") {
      d.iconText?.classList.add("hidden");
      d.iconCrypto?.classList.remove("hidden");
    } else {
      d.iconCrypto?.classList.add("hidden");
      d.iconText?.classList.remove("hidden");
      if (d.iconText) d.iconText.textContent = sig.pair.split("/")[0];
    }

    if (d.pair) d.pair.textContent = sig.pair;

    if (d.action) {
      d.action.textContent = sig.action.toUpperCase();
      setColor(d.action, buy ? ["bg-emerald-500/15", "text-emerald-500"] : ["bg-red-500/15", "text-red-500"]);
    }
    if (d.status) {
      d.status.textContent = STATUS_LABEL[sig.status] || sig.status;
      setColor(d.status, STATUS_COLOR[sig.status] || ["bg-emerald-500/15", "text-emerald-500"]);
    }

    // Confidence
    const cc = sig.confidence >= 80 ? "text-emerald-500" : sig.confidence >= 60 ? "text-amber-500" : "text-red-500";
    const bc = sig.confidence >= 80 ? "bg-emerald-500" : sig.confidence >= 60 ? "bg-amber-500" : "bg-red-500";
    if (d.confText) {
      d.confText.textContent = sig.confidence + "%";
      d.confText.classList.remove("text-emerald-500", "text-amber-500", "text-red-500");
      d.confText.classList.add(cc);
    }
    if (d.confBar) {
      d.confBar.classList.remove("bg-emerald-500", "bg-amber-500", "bg-red-500");
      d.confBar.classList.add(bc);
      d.confBar.style.width = sig.confidence + "%";
    }

    if (d.entry) d.entry.textContent = sig.entry;
    if (d.tp) d.tp.textContent = sig.tp;
    if (d.sl) d.sl.textContent = sig.sl;
    if (d.tp1) d.tp1.textContent = sig.tp1;
    if (d.tp2) d.tp2.textContent = sig.tp2;
    if (d.tp3) d.tp3.textContent = sig.tp3;
    if (d.rr) d.rr.textContent = sig.rr;
    if (d.tf) d.tf.textContent = sig.tf;
    if (d.saved) d.saved.textContent = sig.saved;
    if (d.notes) d.notes.textContent = sig.notes;
    if (d.analysis) d.analysis.textContent = sig.analysis;
    if (d.source) d.source.textContent = sig.source === "ai" ? "AI" : "Manual";
    d.sourceIcon?.setAttribute("data-lucide", sig.source === "ai" ? "sparkles" : "user");

    openDrawer(detailDrawer);
    renderDetailChart(sig);
  }

  function renderDetailChart(sig) {
    const canvas = document.getElementById("ssDetailChart");
    if (!canvas || !hasChart()) return;
    if (detailChart) {
      detailChart.destroy();
      detailChart = null;
    }
    requestAnimationFrame(() => {
      const isLight = !document.documentElement.classList.contains("dark");
      const tick = isLight ? "#64748B" : "#94A3B8";
      const color = sig.action === "buy" ? "#10b981" : "#ef4444";
      detailChart = new Chart(canvas.getContext("2d"), {
        type: "line",
        data: {
          labels: sig.series.map((_, i) => i + 1),
          datasets: [
            {
              data: sig.series,
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
            y: { display: true, grid: { color: "rgba(148,163,184,0.1)" }, ticks: { color: tick, font: { size: 10 } } },
          },
          interaction: { intersect: false, mode: "index" },
        },
      });
    });
  }

  document.addEventListener("click", (e) => {
    const viewBtn = e.target.closest?.(".ss-view-btn");
    if (viewBtn) {
      e.stopPropagation();
      openDetail(viewBtn.dataset.id);
    }
  });

  // Detail drawer chart range buttons (visual switch + re-render with same series)
  document.querySelectorAll(".ssd-range-btn").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll(".ssd-range-btn").forEach((b) => {
        b.classList.remove("active", "bg-accent/15", "text-accent");
        b.classList.add("text-muted");
      });
      btn.classList.add("active", "bg-accent/15", "text-accent");
      btn.classList.remove("text-muted");
      if (currentSig) renderDetailChart(currentSig);
    });
  });

  document.querySelector(".ss-detail-share")?.addEventListener("click", () => {
    showToast("Share Signal", `Sharing ${currentSig?.pair || "signal"}...`);
  });
  document.querySelector(".ss-detail-execute")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Execute Trade", `Opening trade for ${currentSig?.pair || "signal"}...`);
  });

  /* ======================================
   * 10. Filter drawer / Add signal / Export + theme + shortcuts
   * ====================================== */
  document.getElementById("ssApplyFiltersBtn")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Filters Applied", "Your filters have been applied");
    applyFilters();
  });
  document.getElementById("ssResetFiltersBtn")?.addEventListener("click", () => {
    document.querySelectorAll(".ss-filter-opt").forEach((cb) => { cb.checked = true; });
    const cmin = document.getElementById("ssConfMin");
    const cmax = document.getElementById("ssConfMax");
    const dfrom = document.getElementById("ssDateFrom");
    const dto = document.getElementById("ssDateTo");
    if (cmin) cmin.value = "0";
    if (cmax) cmax.value = "100";
    if (dfrom) dfrom.value = "";
    if (dto) dto.value = "";
    showToast("Filters Reset", "All filters have been reset");
  });

  document.querySelector(".ss-save-signal")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Signal Saved", "Your signal has been added to the saved list");
  });

  // Export menu
  const exportBtn = document.getElementById("ssExportBtn");
  const exportMenu = document.getElementById("ssExportMenu");
  exportBtn?.addEventListener("click", (e) => {
    e.stopPropagation();
    exportMenu?.classList.toggle("hidden");
  });
  document.addEventListener("click", () => exportMenu?.classList.add("hidden"));
  document.querySelectorAll(".ss-export-item").forEach((item) =>
    item.addEventListener("click", () => {
      const fmt = (item.dataset.format || "csv").toUpperCase();
      exportMenu?.classList.add("hidden");
      showToast("Export Started", `Exporting saved signals as ${fmt}...`);
    })
  );

  // Theme re-color (open detail chart only)
  document.getElementById("themeToggle")?.addEventListener("click", () => {
    setTimeout(() => {
      if (detailChart && detailDrawer?.classList.contains("active")) {
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
  applyFilters();
  refreshIcons();
});
