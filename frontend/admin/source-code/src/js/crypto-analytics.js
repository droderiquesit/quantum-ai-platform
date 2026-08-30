/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: crypto-analytics.js
 * description: Self-contained controller for the Crypto Analytics page
 *              (#crypto-analytics). DOM-only — no HTML is generated in JS;
 *              all markup lives in crypto-analytics.html as static templates.
 * author: SignalAIX

    -------------------------------------------------
    Table of Contents
    -------------------------------------------------
    01. Init & guard
    02. Toast
    03. Main tabs (Overview / Market Data / On-Chain / DeFi / NFT)
    04. Filter chips (All / Trending / New)
    05. Export dropdown
    06. Refresh
    07. Crypto table search + new-tab table searches + DeFi range pills
    08. Crypto-detail / volume / signals drawer
    09. Chart timeframe pills + sector select
    10. Charts: Overview (market/volume/gauge) + lazy per-tab charts
        (Market Data / On-Chain / DeFi / NFT)
    11. Theme re-color
    ================================================== */

(function () {
  /* ------------------------------------------------------------------ */
  /* 01. Init & guard                                                   */
  /* ------------------------------------------------------------------ */
  if (!document.getElementById("crypto-analytics")) return;

  const isDark = () => document.documentElement.classList.contains("dark");

  /* ------------------------------------------------------------------ */
  /* 02. Toast                                                          */
  /* ------------------------------------------------------------------ */
  const toast = document.getElementById("caToast");
  const toastTitle = document.getElementById("caToastTitle");
  const toastMsg = document.getElementById("caToastMessage");
  let toastTimer;

  function showToast(title, message) {
    if (!toast) return;
    if (title) toastTitle.textContent = title;
    if (message) toastMsg.textContent = message;
    toast.classList.add("active");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => toast.classList.remove("active"), 3000);
  }
  document.getElementById("caToastClose")?.addEventListener("click", () => toast.classList.remove("active"));

  // Generic in-drawer toast buttons
  document.querySelectorAll(".ca-toast-btn").forEach((btn) => {
    btn.addEventListener("click", () => showToast(btn.dataset.toastTitle, btn.dataset.toastMsg));
  });

  /* ------------------------------------------------------------------ */
  /* 03. Main tabs                                                      */
  /* ------------------------------------------------------------------ */
  const tabs = document.querySelectorAll(".ca-tab");
  const panels = document.querySelectorAll(".ca-panel");

  function setTabActive(tab, on) {
    tab.classList.toggle("active", on);
    tab.classList.toggle("text-muted", !on);
  }

  // Lazy chart init per tab — each tab's chart(s) init once, when first shown
  // (Chart.js needs a visible/sized canvas). Defined in section 10.
  const tabInitialized = {};
  function initTabCharts(id) {
    if (tabInitialized[id]) return;
    if (typeof Chart === "undefined") return;
    if (id === "market-data") initMarketDataChart();
    else if (id === "on-chain") initOnChainChart();
    else if (id === "defi") initDefiCharts();
    else if (id === "nft") initNftChart();
    else return;
    tabInitialized[id] = true;
  }

  tabs.forEach((tab) => {
    tab.addEventListener("click", () => {
      const id = tab.dataset.tab;
      tabs.forEach((t) => setTabActive(t, t === tab));
      panels.forEach((p) => p.classList.toggle("hidden", p.dataset.tab !== id));
      // init the newly-shown tab's charts after it's visible/sized
      requestAnimationFrame(() => initTabCharts(id));
    });
  });

  /* ------------------------------------------------------------------ */
  /* 04. Filter chips                                                   */
  /* ------------------------------------------------------------------ */
  const filterChips = document.querySelectorAll(".ca-filter");
  function setChipActive(btn, on) {
    btn.classList.toggle("active", on);
    btn.classList.toggle("bg-accent/10", on);
    btn.classList.toggle("border-accent", on);
    btn.classList.toggle("text-accent", on);
    btn.classList.toggle("text-muted", !on);
  }
  filterChips.forEach((btn) => {
    setChipActive(btn, btn.classList.contains("active"));
    btn.addEventListener("click", () => {
      filterChips.forEach((b) => setChipActive(b, b === btn));
      showToast("Filter Applied", `Showing ${btn.textContent.trim()} cryptocurrencies`);
    });
  });

  /* ------------------------------------------------------------------ */
  /* 05. Export dropdown                                                */
  /* ------------------------------------------------------------------ */
  const exportToggle = document.querySelector(".ca-export-toggle");
  const exportMenu = document.getElementById("caExportMenu");
  exportToggle?.addEventListener("click", (e) => {
    e.stopPropagation();
    exportMenu?.classList.toggle("active");
  });
  document.addEventListener("click", (e) => {
    if (!e.target.closest(".dropdown")) exportMenu?.classList.remove("active");
  });
  document.querySelectorAll(".ca-export").forEach((item) => {
    item.addEventListener("click", () => {
      const fmt = (item.dataset.format || "").toUpperCase();
      exportMenu?.classList.remove("active");
      showToast("Export Started", `Exporting data as ${fmt}…`);
      setTimeout(() => showToast("Export Complete", `Your ${fmt} file is ready`), 2000);
    });
  });

  /* ------------------------------------------------------------------ */
  /* 06. Refresh                                                        */
  /* ------------------------------------------------------------------ */
  const refreshIcon = document.getElementById("caRefreshIcon");
  document.querySelector(".ca-refresh")?.addEventListener("click", () => {
    refreshIcon?.classList.add("animate-spin");
    showToast("Refreshing", "Updating market data…");
    setTimeout(() => {
      refreshIcon?.classList.remove("animate-spin");
      showToast("Updated", "Market data refreshed successfully");
    }, 1500);
  });

  /* ------------------------------------------------------------------ */
  /* 07. Crypto table search                                            */
  /* ------------------------------------------------------------------ */
  const search = document.getElementById("caCryptoSearch");
  search?.addEventListener("input", () => {
    const q = search.value.toLowerCase();
    document.querySelectorAll(".ca-crypto-row").forEach((row) => {
      const name = (row.dataset.name || "") + " " + (row.dataset.symbol || "").toLowerCase();
      row.style.display = name.includes(q) ? "" : "none";
    });
  });

  // New-tab table searches (Market Data / DeFi / NFT) — filter rows by data-name
  function wireSearch(inputId, rowSel) {
    const input = document.getElementById(inputId);
    input?.addEventListener("input", () => {
      const q = input.value.toLowerCase();
      document.querySelectorAll(rowSel).forEach((row) => {
        row.style.display = (row.dataset.name || "").includes(q) ? "" : "none";
      });
    });
  }
  wireSearch("caTokenSearch", ".ca-token-row");
  wireSearch("caProtocolSearch", ".ca-protocol-row");
  wireSearch("caCollectionSearch", ".ca-collection-row");

  // DeFi TVL range pills
  const defiRanges = document.querySelectorAll(".ca-defi-range");
  defiRanges.forEach((btn) => {
    setPillLook(btn, btn.classList.contains("active"));
    btn.addEventListener("click", () => {
      defiRanges.forEach((b) => setPillLook(b, b === btn));
      showToast("Range Changed", `Showing ${btn.dataset.range} TVL data`);
    });
  });
  function setPillLook(btn, on) {
    btn.classList.toggle("active", on);
    btn.classList.toggle("bg-accent/10", on);
    btn.classList.toggle("border-accent", on);
    btn.classList.toggle("text-accent", on);
    btn.classList.toggle("text-muted", !on);
  }

  /* ------------------------------------------------------------------ */
  /* 08. Drawer (volume / signals / cryptoDetail)                       */
  /* ------------------------------------------------------------------ */
  const drawer = document.getElementById("caDrawer");
  const overlay = document.getElementById("caDrawerOverlay");
  const drawerTitle = document.getElementById("caDrawerTitle");
  const drawerPanels = drawer ? drawer.querySelectorAll(".ca-panel-drawer") : [];

  const TITLES = {
    volume: "Volume Distribution Details",
    signals: "All AI Signals",
    cryptoDetail: "Crypto Details",
  };

  function showDrawerPanel(name) {
    drawerPanels.forEach((p) => (p.hidden = p.dataset.panel !== name));
    if (drawerTitle) drawerTitle.textContent = TITLES[name] || "Details";
  }

  function openDrawer(name) {
    showDrawerPanel(name);
    overlay?.classList.add("active");
    drawer?.classList.add("active");
    if (window.lucide) lucide.createIcons();
  }

  function closeDrawer() {
    overlay?.classList.remove("active");
    drawer?.classList.remove("active");
  }

  document.querySelectorAll(".ca-open-drawer").forEach((btn) => {
    btn.addEventListener("click", () => openDrawer(btn.dataset.drawer));
  });
  document.querySelector(".ca-drawer-close")?.addEventListener("click", closeDrawer);
  overlay?.addEventListener("click", closeDrawer);
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      closeDrawer();
      exportMenu?.classList.remove("active");
    }
  });

  // Crypto-detail: cache refs, populate via DOM updates from the row's data-d
  const cd = {
    iconWrap: document.getElementById("caCdIconWrap"),
    iconLetter: document.getElementById("caCdIconLetter"),
    name: document.getElementById("caCdName"),
    symbol: document.getElementById("caCdSymbol"),
    price: document.getElementById("caCdPrice"),
    change24: document.getElementById("caCdChange24"),
    cap: document.getElementById("caCdCap"),
    vol: document.getElementById("caCdVol"),
    change7: document.getElementById("caCdChange7"),
    rank: document.getElementById("caCdRank"),
  };

  function openCryptoDetail(d) {
    if (cd.iconWrap) cd.iconWrap.style.background = (d.color || "#10b981") + "20";
    if (cd.iconLetter) {
      cd.iconLetter.textContent = (d.symbol || "?")[0];
      cd.iconLetter.style.color = d.color || "#10b981";
    }
    if (cd.name) cd.name.textContent = d.name;
    if (cd.symbol) cd.symbol.textContent = d.symbol;
    if (cd.price) cd.price.textContent = d.price;
    if (cd.change24) {
      cd.change24.textContent = d.change24h;
      cd.change24.className = "text-2xl font-bold mt-1 " + (d.up24 ? "text-emerald-500" : "text-red-500");
    }
    if (cd.cap) cd.cap.textContent = d.cap;
    if (cd.vol) cd.vol.textContent = d.vol;
    if (cd.change7) {
      cd.change7.textContent = d.change7d;
      cd.change7.className = "font-semibold " + (d.up7 ? "text-emerald-500" : "text-red-500");
    }
    if (cd.rank) cd.rank.textContent = d["rank#"];
    openDrawer("cryptoDetail");
  }

  document.querySelectorAll(".ca-crypto-row").forEach((row) => {
    row.addEventListener("click", (e) => {
      if (e.target.closest(".ca-star")) return; // star button doesn't open the drawer
      let data;
      try {
        data = JSON.parse(row.dataset.d);
      } catch (_) {
        return;
      }
      openCryptoDetail(data);
    });
  });

  document.querySelectorAll(".ca-star").forEach((btn) => {
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      showToast("Watchlist Updated", "Toggled favorite for this asset");
    });
  });

  /* ------------------------------------------------------------------ */
  /* 09. Chart timeframe pills + sector select                         */
  /* ------------------------------------------------------------------ */
  const tfPills = document.querySelectorAll(".ca-tf");
  function setPillActive(btn, on) {
    btn.classList.toggle("active", on);
    btn.classList.toggle("bg-accent/10", on);
    btn.classList.toggle("border-accent", on);
    btn.classList.toggle("text-accent", on);
    btn.classList.toggle("text-muted", !on);
  }
  tfPills.forEach((btn) => {
    setPillActive(btn, btn.classList.contains("active"));
    btn.addEventListener("click", () => {
      tfPills.forEach((b) => setPillActive(b, b === btn));
      showToast("Timeframe Changed", `Showing ${btn.dataset.tf} data`);
    });
  });

  document.getElementById("caSectorSelect")?.addEventListener("change", (e) => {
    showToast("Period Changed", `Showing ${e.target.options[e.target.selectedIndex].text} sector performance`);
  });

  /* ------------------------------------------------------------------ */
  /* 10. Charts                                                         */
  /* ------------------------------------------------------------------ */
  let marketChart, volumeChart;
  // Lazy per-tab charts (init once when their tab is first shown)
  let marketDataChart, onChainChart, tvlChart, tvlCategoryChart, nftVolumeChart;

  function axisColors() {
    return { tick: isDark() ? "#94A3B8" : "#64748B", grid: isDark() ? "rgba(255,255,255,0.05)" : "#E2E8F0" };
  }

  // Market Data tab: market cap (line) + volume (bar)
  function initMarketDataChart() {
    const el = document.getElementById("caMarketDataChart");
    if (!el) return;
    const { tick, grid } = axisColors();
    const ctx = el.getContext("2d");
    const fill = ctx.createLinearGradient(0, 0, 0, 320);
    fill.addColorStop(0, "rgba(16,185,129,0.3)");
    fill.addColorStop(1, "rgba(16,185,129,0)");
    if (marketDataChart) marketDataChart.destroy();
    marketDataChart = new Chart(ctx, {
      data: {
        labels: ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"],
        datasets: [
          { type: "line", label: "Market Cap", yAxisID: "y", data: [1.8, 1.9, 2.1, 2.0, 2.3, 2.2, 2.4, 2.3, 2.5, 2.4, 2.6, 2.34], borderColor: "#10B981", backgroundColor: fill, borderWidth: 2, fill: true, tension: 0.4, pointRadius: 0, pointHoverRadius: 6 },
          { type: "bar", label: "Volume", yAxisID: "y1", data: [60, 72, 85, 64, 90, 78, 98, 82, 110, 88, 105, 98.2], backgroundColor: "rgba(99,102,241,0.5)", borderRadius: 4 },
        ],
      },
      options: {
        responsive: true,
        maintainAspectRatio: false,
        plugins: { legend: { labels: { color: tick, usePointStyle: true, pointStyle: "circle", font: { size: 11 } } } },
        scales: {
          x: { grid: { display: false }, ticks: { color: tick, font: { size: 11 } } },
          y: { position: "left", grid: { color: grid }, ticks: { color: tick, font: { size: 11 }, callback: (v) => "$" + v + "T" } },
          y1: { position: "right", grid: { display: false }, ticks: { color: tick, font: { size: 11 }, callback: (v) => "$" + v + "B" } },
        },
        interaction: { intersect: false, mode: "index" },
      },
    });
  }

  // On-Chain tab: active addresses + tx count
  function initOnChainChart() {
    const el = document.getElementById("caOnChainChart");
    if (!el) return;
    const { tick, grid } = axisColors();
    if (onChainChart) onChainChart.destroy();
    onChainChart = new Chart(el.getContext("2d"), {
      type: "line",
      data: {
        labels: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
        datasets: [
          { label: "Active Addresses (K)", data: [980, 1040, 1120, 1080, 1210, 1180, 1240], borderColor: "#10B981", backgroundColor: "rgba(16,185,129,0.1)", borderWidth: 2, fill: true, tension: 0.4, pointRadius: 0, pointHoverRadius: 6 },
          { label: "Transactions (K)", data: [620, 700, 660, 740, 690, 810, 770], borderColor: "#6366F1", backgroundColor: "rgba(99,102,241,0.08)", borderWidth: 2, fill: true, tension: 0.4, pointRadius: 0, pointHoverRadius: 6 },
        ],
      },
      options: {
        responsive: true,
        maintainAspectRatio: false,
        plugins: { legend: { labels: { color: tick, usePointStyle: true, pointStyle: "circle", font: { size: 11 } } } },
        scales: {
          x: { grid: { display: false }, ticks: { color: tick, font: { size: 11 } } },
          y: { grid: { color: grid }, ticks: { color: tick, font: { size: 11 } } },
        },
        interaction: { intersect: false, mode: "index" },
      },
    });
  }

  // DeFi tab: TVL over time (line) + TVL by category (doughnut)
  function initDefiCharts() {
    const { tick, grid } = axisColors();
    const tvlEl = document.getElementById("caTvlChart");
    if (tvlEl) {
      const ctx = tvlEl.getContext("2d");
      const fill = ctx.createLinearGradient(0, 0, 0, 320);
      fill.addColorStop(0, "rgba(16,185,129,0.3)");
      fill.addColorStop(1, "rgba(16,185,129,0)");
      if (tvlChart) tvlChart.destroy();
      tvlChart = new Chart(ctx, {
        type: "line",
        data: {
          labels: ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"],
          datasets: [{ label: "TVL", data: [38, 42, 45, 40, 43, 47, 44, 46, 49, 48, 47, 48.7], borderColor: "#10B981", backgroundColor: fill, borderWidth: 2, fill: true, tension: 0.4, pointRadius: 0, pointHoverRadius: 6 }],
        },
        options: {
          responsive: true,
          maintainAspectRatio: false,
          plugins: { legend: { display: false }, tooltip: { displayColors: false, callbacks: { label: (c) => `$${c.parsed.y}B` } } },
          scales: {
            x: { grid: { display: false }, ticks: { color: tick, font: { size: 11 } } },
            y: { grid: { color: grid }, ticks: { color: tick, font: { size: 11 }, callback: (v) => "$" + v + "B" } },
          },
          interaction: { intersect: false, mode: "index" },
        },
      });
    }
    const catEl = document.getElementById("caTvlCategoryChart");
    if (catEl) {
      if (tvlCategoryChart) tvlCategoryChart.destroy();
      tvlCategoryChart = new Chart(catEl.getContext("2d"), {
        type: "doughnut",
        data: {
          labels: ["Liquid Staking", "Lending", "DEXes", "Bridges", "Others"],
          datasets: [{ data: [37, 26, 19, 8, 10], backgroundColor: ["#10B981", "#6366F1", "#14B8A6", "#0EA5E9", "#F59E0B"], borderWidth: 0, hoverOffset: 8 }],
        },
        options: { responsive: true, maintainAspectRatio: false, cutout: "65%", plugins: { legend: { display: false }, tooltip: { callbacks: { label: (c) => `${c.label}: ${c.parsed}%` } } } },
      });
    }
  }

  // NFT tab: weekly volume (line)
  function initNftChart() {
    const el = document.getElementById("caNftVolumeChart");
    if (!el) return;
    const { tick, grid } = axisColors();
    const ctx = el.getContext("2d");
    const fill = ctx.createLinearGradient(0, 0, 0, 320);
    fill.addColorStop(0, "rgba(139,92,246,0.3)");
    fill.addColorStop(1, "rgba(139,92,246,0)");
    if (nftVolumeChart) nftVolumeChart.destroy();
    nftVolumeChart = new Chart(ctx, {
      type: "line",
      data: {
        labels: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
        datasets: [{ label: "Volume (ETH)", data: [12500, 15800, 14200, 18900, 16400, 21200, 19800], borderColor: "#8B5CF6", backgroundColor: fill, borderWidth: 2, fill: true, tension: 0.4, pointRadius: 0, pointHoverRadius: 6 }],
      },
      options: {
        responsive: true,
        maintainAspectRatio: false,
        plugins: { legend: { display: false }, tooltip: { displayColors: false, callbacks: { label: (c) => `${c.parsed.y.toLocaleString()} ETH` } } },
        scales: {
          x: { grid: { display: false }, ticks: { color: tick, font: { size: 11 } } },
          y: { grid: { color: grid }, ticks: { color: tick, font: { size: 11 }, callback: (v) => v / 1000 + "K" } },
        },
        interaction: { intersect: false, mode: "index" },
      },
    });
  }

  function drawSentimentGauge() {
    const canvas = document.getElementById("caSentimentGauge");
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    ctx.clearRect(0, 0, canvas.width, canvas.height);

    const centerX = canvas.width / 2;
    const centerY = canvas.height;
    const radius = 80;

    const gradient = ctx.createLinearGradient(0, 0, canvas.width, 0);
    gradient.addColorStop(0, "#EF4444");
    gradient.addColorStop(0.25, "#F59E0B");
    gradient.addColorStop(0.5, "#FBBF24");
    gradient.addColorStop(0.75, "#84CC16");
    gradient.addColorStop(1, "#10B981");

    ctx.beginPath();
    ctx.arc(centerX, centerY, radius, Math.PI, 0);
    ctx.strokeStyle = isDark() ? "#1E293B" : "#E2E8F0";
    ctx.lineWidth = 20;
    ctx.lineCap = "round";
    ctx.stroke();

    const value = 0.72;
    ctx.beginPath();
    ctx.arc(centerX, centerY, radius, Math.PI, Math.PI + Math.PI * value);
    ctx.strokeStyle = gradient;
    ctx.lineWidth = 20;
    ctx.lineCap = "round";
    ctx.stroke();

    const needleAngle = Math.PI + Math.PI * value;
    const needleLength = 50;
    ctx.beginPath();
    ctx.moveTo(centerX, centerY);
    ctx.lineTo(centerX + Math.cos(needleAngle) * needleLength, centerY + Math.sin(needleAngle) * needleLength);
    ctx.strokeStyle = isDark() ? "#F1F5F9" : "#0F172A";
    ctx.lineWidth = 3;
    ctx.lineCap = "round";
    ctx.stroke();

    ctx.beginPath();
    ctx.arc(centerX, centerY, 6, 0, Math.PI * 2);
    ctx.fillStyle = isDark() ? "#F1F5F9" : "#0F172A";
    ctx.fill();
  }

  function initCharts() {
    if (typeof Chart === "undefined") return;
    const tick = isDark() ? "#94A3B8" : "#64748B";
    const grid = isDark() ? "#1A1F2E" : "#E2E8F0";

    const marketEl = document.getElementById("caMarketChart");
    if (marketEl) {
      const mctx = marketEl.getContext("2d");
      const fill = mctx.createLinearGradient(0, 0, 0, 320);
      fill.addColorStop(0, "rgba(245,158,11,0.3)");
      fill.addColorStop(1, "rgba(245,158,11,0)");
      if (marketChart) marketChart.destroy();
      marketChart = new Chart(mctx, {
        type: "line",
        data: {
          labels: ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"],
          datasets: [
            {
              label: "Market Cap",
              data: [1.8, 1.9, 2.1, 2.0, 2.3, 2.2, 2.4, 2.3, 2.5, 2.4, 2.6, 2.48],
              borderColor: "#F59E0B",
              backgroundColor: fill,
              borderWidth: 2,
              fill: true,
              tension: 0.4,
              pointRadius: 0,
              pointHoverRadius: 6,
            },
          ],
        },
        options: {
          responsive: true,
          maintainAspectRatio: false,
          plugins: {
            legend: { display: false },
            tooltip: { displayColors: false, callbacks: { label: (c) => `$${c.parsed.y}T` } },
          },
          scales: {
            x: { grid: { display: false }, ticks: { color: tick, font: { size: 11 } } },
            y: { grid: { color: grid }, ticks: { color: tick, font: { size: 11 }, callback: (v) => "$" + v + "T" } },
          },
          interaction: { intersect: false, mode: "index" },
        },
      });
    }

    const volEl = document.getElementById("caVolumeChart");
    if (volEl) {
      if (volumeChart) volumeChart.destroy();
      volumeChart = new Chart(volEl.getContext("2d"), {
        type: "doughnut",
        data: {
          labels: ["Binance", "Coinbase", "OKX", "Bybit", "Others"],
          datasets: [
            {
              data: [31.7, 14.1, 10.9, 9.2, 34.1],
              backgroundColor: ["#F3BA2F", "#0052FF", "#64748B", "#F7931A", "#6366F1"],
              borderWidth: 0,
              hoverOffset: 10,
            },
          ],
        },
        options: {
          responsive: true,
          maintainAspectRatio: false,
          cutout: "70%",
          plugins: {
            legend: { position: "bottom", labels: { color: tick, padding: 16, usePointStyle: true, pointStyle: "circle", font: { size: 11 } } },
            tooltip: { callbacks: { label: (c) => `${c.label}: ${c.parsed}%` } },
          },
        },
      });
    }

    drawSentimentGauge();
  }

  /* ------------------------------------------------------------------ */
  /* 11. Theme re-color                                                 */
  /* ------------------------------------------------------------------ */
  function recolorCharts() {
    const tick = isDark() ? "#94A3B8" : "#64748B";
    const grid = isDark() ? "#1A1F2E" : "#E2E8F0";
    if (marketChart) {
      marketChart.options.scales.x.ticks.color = tick;
      marketChart.options.scales.y.ticks.color = tick;
      marketChart.options.scales.y.grid.color = grid;
      marketChart.update();
    }
    if (volumeChart) {
      volumeChart.options.plugins.legend.labels.color = tick;
      volumeChart.update();
    }
    // Lazy per-tab charts (only present after their tab was opened)
    const g2 = isDark() ? "rgba(255,255,255,0.05)" : "#E2E8F0";
    if (marketDataChart) {
      marketDataChart.options.plugins.legend.labels.color = tick;
      marketDataChart.options.scales.x.ticks.color = tick;
      marketDataChart.options.scales.y.ticks.color = tick;
      marketDataChart.options.scales.y.grid.color = g2;
      marketDataChart.options.scales.y1.ticks.color = tick;
      marketDataChart.update();
    }
    if (onChainChart) {
      onChainChart.options.plugins.legend.labels.color = tick;
      onChainChart.options.scales.x.ticks.color = tick;
      onChainChart.options.scales.y.ticks.color = tick;
      onChainChart.options.scales.y.grid.color = g2;
      onChainChart.update();
    }
    if (tvlChart) {
      tvlChart.options.scales.x.ticks.color = tick;
      tvlChart.options.scales.y.ticks.color = tick;
      tvlChart.options.scales.y.grid.color = g2;
      tvlChart.update();
    }
    if (tvlCategoryChart) tvlCategoryChart.update();
    if (nftVolumeChart) {
      nftVolumeChart.options.scales.x.ticks.color = tick;
      nftVolumeChart.options.scales.y.ticks.color = tick;
      nftVolumeChart.options.scales.y.grid.color = g2;
      nftVolumeChart.update();
    }
    drawSentimentGauge();
  }
  document.getElementById("themeToggle")?.addEventListener("click", () => setTimeout(recolorCharts, 0));

  // Charts depend on the CDN Chart.js (loaded with defer) — init after load.
  if (document.readyState === "complete") initCharts();
  else window.addEventListener("load", initCharts);
})();
