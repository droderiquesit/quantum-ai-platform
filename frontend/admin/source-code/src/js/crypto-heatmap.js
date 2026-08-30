/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: crypto-heatmap.js
 * description: Self-contained controller for the Crypto Heatmap page
 *              (#crypto-heatmap). DOM-only — the heatmap cells, table rows and
 *              drawer bodies are static templates the JS shows/hides, reorders,
 *              or patches via textContent (no HTML generated in JS).
 * author: SignalAIX

    -------------------------------------------------
    Table of Contents
    -------------------------------------------------
    01. Init & guard
    02. Toast
    03. Category tabs (filter cells + rows)
    04. Timeframe + sector chips
    05. View toggle (heatmap / list)
    06. Search
    07. Sort (reorder existing nodes — DOM only)
    08. Export + refresh
    09. Drawer (coin / ai / filter) + cap pills
    ================================================== */

(function () {
  if (!document.getElementById("crypto-heatmap")) return;
  const refreshIcons = () => window.lucide && lucide.createIcons();

  /* 02. Toast */
  const toast = document.getElementById("chToast");
  let toastTimer;
  function showToast(title, message) {
    if (!toast) return;
    if (title) document.getElementById("chToastTitle").textContent = title;
    if (message) document.getElementById("chToastMessage").textContent = message;
    toast.classList.add("active");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => toast.classList.remove("active"), 3000);
  }
  document.getElementById("chToastClose")?.addEventListener("click", () => toast.classList.remove("active"));
  document.querySelectorAll(".ch-toast-btn").forEach((b) =>
    b.addEventListener("click", () => {
      showToast(b.dataset.toastTitle, b.dataset.toastMsg);
      if (b.hasAttribute("data-close")) closeDrawer();
    })
  );

  const cells = Array.from(document.querySelectorAll(".ch-cell"));
  const rows = Array.from(document.querySelectorAll(".ch-row"));
  const heatEmpty = document.querySelector(".ch-empty");
  const listEmpty = document.querySelector(".ch-list-empty");
  let activeCat = "all";
  let searchTerm = "";

  function applyFilters() {
    let visCells = 0, visRows = 0;
    const match = (el) => (activeCat === "all" || el.dataset.cat === activeCat) && (!searchTerm || el.dataset.name.includes(searchTerm) || (el.dataset.symbol || "").includes(searchTerm));
    cells.forEach((c) => { const ok = match(c); c.style.display = ok ? "" : "none"; if (ok) visCells++; });
    rows.forEach((r) => { const ok = match(r); r.style.display = ok ? "" : "none"; if (ok) visRows++; });
    heatEmpty?.classList.toggle("hidden", visCells !== 0);
    listEmpty?.classList.toggle("hidden", visRows !== 0);
  }

  /* 03. Category tabs */
  document.querySelectorAll(".ch-tab").forEach((tab) => {
    tab.addEventListener("click", () => {
      activeCat = tab.dataset.cat;
      document.querySelectorAll(".ch-tab").forEach((t) => t.classList.remove("active"));
      tab.classList.add("active");
      applyFilters();
    });
  });

  /* 04. Timeframe + sector chips */
  function wireChips(selector, onPick) {
    const chips = document.querySelectorAll(selector);
    chips.forEach((btn) => {
      setChip(btn, btn.classList.contains("active"));
      btn.addEventListener("click", () => {
        chips.forEach((b) => setChip(b, b === btn));
        onPick(btn);
      });
    });
  }
  function setChip(btn, on) {
    btn.classList.toggle("active", on);
    btn.classList.toggle("bg-accent/10", on);
    btn.classList.toggle("border-accent", on);
    btn.classList.toggle("text-accent", on);
    btn.classList.toggle("text-muted", !on);
  }
  wireChips(".ch-tf", (b) => showToast("Timeframe Changed", `Showing ${b.dataset.tf} changes`));
  wireChips(".ch-sector", (b) => showToast("Sector View", `Showing ${b.dataset.sector} performance`));

  /* 05. View toggle */
  const heatmapContainer = document.getElementById("chHeatmapContainer");
  const listContainer = document.getElementById("chListContainer");
  document.querySelectorAll(".ch-view").forEach((btn) => {
    btn.addEventListener("click", () => {
      const view = btn.dataset.view;
      document.querySelectorAll(".ch-view").forEach((b) => {
        const on = b === btn;
        b.classList.toggle("active", on);
        b.classList.toggle("bg-accent", on);
        b.classList.toggle("border-accent", on);
        b.classList.toggle("text-white", on);
        b.classList.toggle("bg-panel", !on);
        b.classList.toggle("border-border", !on);
        b.classList.toggle("text-text", !on);
      });
      heatmapContainer?.classList.toggle("hidden", view !== "heatmap");
      listContainer?.classList.toggle("hidden", view !== "list");
    });
  });
  // initialise the active view button look
  const activeView = document.querySelector(".ch-view.active");
  if (activeView) {
    activeView.classList.add("bg-accent", "border-accent", "text-white");
    activeView.classList.remove("bg-panel", "border-border", "text-text", "text-muted");
  }

  /* 06. Search */
  document.getElementById("chSearch")?.addEventListener("input", (e) => {
    searchTerm = e.target.value.toLowerCase();
    applyFilters();
  });

  /* 07. Sort — reorder the existing cell/row nodes (DOM-only, no markup) */
  const grid = document.getElementById("chHeatmapGrid");
  const tbody = document.getElementById("chTableBody");
  let ascending = false;

  // numeric sort keys parsed from each node's static text (no extra data needed)
  function num(str) { return parseFloat((str || "").replace(/[^0-9.\-]/g, "")) || 0; }
  function capValue(el) {
    // market cap text lives in the table row's 7th cell; for cells use order as proxy
    return num(el.querySelector?.("td:nth-child(7)")?.textContent) || 0;
  }

  function sortNodes(nodes, container, keyFn) {
    nodes
      .slice()
      .sort((a, b) => (ascending ? keyFn(a) - keyFn(b) : keyFn(b) - keyFn(a)))
      .forEach((n) => container.appendChild(n));
  }

  function runSort() {
    const by = document.getElementById("chSort")?.value || "market_cap";
    // rows have full data in their cells; sort rows, then mirror order onto cells by symbol
    const rowKey = (r) => {
      if (by === "name") return 0; // handled below via localeCompare
      const map = { market_cap: 7, volume: 8, change: 5 };
      const idx = map[by] || 7;
      return num(r.querySelector(`td:nth-child(${idx})`)?.textContent);
    };
    if (by === "name") {
      rows
        .slice()
        .sort((a, b) => (ascending ? a.dataset.name.localeCompare(b.dataset.name) : b.dataset.name.localeCompare(a.dataset.name)))
        .forEach((n) => tbody.appendChild(n));
    } else {
      sortNodes(rows, tbody, rowKey);
    }
    // reorder cells to match the row order (by symbol)
    const order = Array.from(tbody.querySelectorAll(".ch-row")).map((r) => r.dataset.symbol);
    order.forEach((sym) => {
      const cell = cells.find((c) => (c.dataset.symbol || "").toLowerCase() === sym);
      if (cell && grid) grid.appendChild(cell);
    });
  }

  document.getElementById("chSort")?.addEventListener("change", runSort);
  document.getElementById("chSortOrder")?.addEventListener("click", () => {
    ascending = !ascending;
    runSort();
    showToast("Sort Order Changed", ascending ? "Ascending" : "Descending");
  });

  /* 08. Export + refresh */
  document.querySelectorAll(".ch-export").forEach((item) => {
    item.addEventListener("click", () => {
      const fmt = (item.dataset.format || "").toUpperCase();
      item.closest(".dropdown-menu")?.classList.remove("active");
      showToast("Export Started", `Exporting heatmap as ${fmt}…`);
    });
  });
  document.getElementById("chRefresh")?.addEventListener("click", () => {
    const icon = document.getElementById("chRefreshIcon");
    icon?.classList.add("animate-spin");
    setTimeout(() => icon?.classList.remove("animate-spin"), 900);
    showToast("Data Updated", "Market data refreshed");
  });

  /* 09. Drawer */
  const drawer = document.getElementById("chDrawer");
  const overlay = document.getElementById("chDrawerOverlay");
  const drawerTitle = document.getElementById("chDrawerTitle");
  const panels = drawer ? drawer.querySelectorAll(".ch-panel") : [];
  const TITLES = { coin: "Coin Details", ai: "AI Market Intelligence", filter: "Filter Settings" };

  function showPanel(name) {
    panels.forEach((p) => (p.hidden = p.dataset.panel !== name));
    if (drawerTitle) drawerTitle.textContent = TITLES[name] || "Details";
  }
  function openDrawer(name) {
    showPanel(name);
    overlay?.classList.add("active");
    drawer?.classList.add("active");
    refreshIcons();
  }
  function closeDrawer() {
    overlay?.classList.remove("active");
    drawer?.classList.remove("active");
  }

  // coin data for the coin-detail panel
  const COINS = {
    BTC: { name: "Bitcoin", price: "$68,542.30", c24: "+2.34%", c1: "+0.45%", c7: "+5.67%", cap: "$1.35T", vol: "$28.5B" },
    ETH: { name: "Ethereum", price: "$3,542.18", c24: "+1.87%", c1: "-0.32%", c7: "+3.21%", cap: "$425.0B", vol: "$15.2B" },
    SOL: { name: "Solana", price: "$178.45", c24: "+8.45%", c1: "+1.24%", c7: "+12.30%", cap: "$82.0B", vol: "$4.1B" },
    BNB: { name: "BNB", price: "$612.80", c24: "+0.92%", c1: "+0.18%", c7: "-1.45%", cap: "$89.0B", vol: "$1.8B" },
    XRP: { name: "Ripple", price: "$2.34", c24: "-3.21%", c1: "-1.12%", c7: "+4.56%", cap: "$134.0B", vol: "$6.8B" },
    ADA: { name: "Cardano", price: "$1.08", c24: "+4.12%", c1: "+0.56%", c7: "+7.89%", cap: "$38.0B", vol: "$1.2B" },
    AVAX: { name: "Avalanche", price: "$42.18", c24: "+6.82%", c1: "+2.10%", c7: "+9.40%", cap: "$17.0B", vol: "$890.0M" },
    DOGE: { name: "Dogecoin", price: "$0.1284", c24: "-5.60%", c1: "-2.40%", c7: "-8.20%", cap: "$18.5B", vol: "$2.1B" },
    PEPE: { name: "Pepe", price: "$0.0000182", c24: "+12.30%", c1: "+4.50%", c7: "+18.40%", cap: "$7.6B", vol: "$1.5B" },
    SHIB: { name: "Shiba Inu", price: "$0.0000245", c24: "-6.40%", c1: "-3.10%", c7: "-9.80%", cap: "$14.4B", vol: "$980.0M" },
    RNDR: { name: "Render", price: "$7.85", c24: "+9.20%", c1: "+2.90%", c7: "+14.50%", cap: "$4.0B", vol: "$320.0M" },
    FET: { name: "Fetch.ai", price: "$1.34", c24: "+10.80%", c1: "+3.40%", c7: "+16.20%", cap: "$3.3B", vol: "$410.0M" },
  };
  function colorClass(v) { return v.startsWith("-") ? "font-semibold text-red-500" : "font-semibold text-emerald-500"; }

  function openCoin(symbol) {
    const m = COINS[symbol] || COINS.BTC;
    const set = (id, val) => { const el = document.getElementById(id); if (el) el.textContent = val; };
    set("chCoinBadge", symbol.slice(0, 3));
    set("chCoinName", m.name);
    set("chCoinSymbol", symbol);
    set("chCoinPrice", m.price);
    set("chCoinCap", m.cap);
    set("chCoinVol", m.vol);
    const colorize = (id, val) => { const el = document.getElementById(id); if (el) { el.textContent = val; el.className = colorClass(val); } };
    colorize("chCoinChange", m.c24);
    colorize("chCoin1h", m.c1);
    colorize("chCoin7d", m.c7);
    openDrawer("coin");
  }

  document.querySelectorAll(".ch-cell, .ch-coin").forEach((el) => {
    el.addEventListener("click", () => openCoin(el.dataset.symbol));
  });
  document.querySelectorAll(".ch-open-drawer").forEach((btn) => {
    btn.addEventListener("click", () => openDrawer(btn.dataset.drawer));
  });
  document.querySelectorAll(".ch-watch").forEach((btn) => {
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      showToast("Added to Watchlist", `${btn.dataset.symbol} added`);
    });
  });
  drawer?.querySelectorAll(".ch-drawer-close").forEach((b) => b.addEventListener("click", closeDrawer));
  overlay?.addEventListener("click", closeDrawer);
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") closeDrawer();
  });

  // cap-filter pills inside the filter drawer
  const capPills = document.querySelectorAll(".ch-cap");
  capPills.forEach((btn) => {
    setChip(btn, btn.classList.contains("active"));
    btn.addEventListener("click", () => capPills.forEach((b) => setChip(b, b === btn)));
  });
})();
