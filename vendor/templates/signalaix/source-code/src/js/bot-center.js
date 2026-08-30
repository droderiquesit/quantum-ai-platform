/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: bot-center.html
 * description: SignalAIX - Bot Center Page Controller (AUTOMATION section)
 *              Self-contained; mirrors the reference mockup's functionality
 *              (status tabs + strategy filter chips + search filtering of bot
 *               cards, per-bot start/pause/stop/delete with status + icon swap,
 *               create/configure-bot drawer (edit reuses the create form),
 *               per-bot details drawer with a REAL Chart.js performance chart,
 *               export menu, toasts).
 *              All markup lives in bot-center.html; this file only modifies the
 *              DOM (text/values, toggle classes incl. status, swap data-lucide,
 *              show/hide) and renders Chart.js — it never injects HTML strings.
 * author: SignalAIX

    -------------------------------------------------
     01. Init & DOM refs (guarded by #bot-center)
     -------------------------------------------------
     02. Toast
     -------------------------------------------------
     03. Drawers (details / create-edit form)
     -------------------------------------------------
     04. Tabs + strategy chips + search (card filtering)
     -------------------------------------------------
     05. Bot status actions (start / pause / stop / delete)
     -------------------------------------------------
     06. Details drawer populate + per-open chart
     -------------------------------------------------
     07. Create / Edit form drawer
     -------------------------------------------------
     08. Export menu + misc
     -------------------------------------------------
     09. Theme re-color + keyboard shortcuts
     -------------------------------------------------
    ================================================== */

document.addEventListener("DOMContentLoaded", () => {
  /* ======================================
   * 01. Init & DOM refs
   * ====================================== */
  const page = document.getElementById("bot-center");
  if (!page) return; // Guard: only run on the Bot Center page

  const refreshIcons = () => window.lucide?.createIcons?.();

  // Static per-bot metadata (mirrors the mockup data; used to populate the
  // drawers + per-open performance chart via DOM updates only).
  const BOTS = {
    1: { name: "Scalper Pro v2", pair: "EUR/USD", strategy: "scalping", strategyLabel: "Scalping Strategy", icon: "zap", grad: ["from-emerald-500", "to-teal-600"], status: "running", profit: "+$12,458", roi: "+24.5% ROI", winRate: "84.2%", trades: "422 / 501 trades", duration: "4.2m", drawdown: "-8.4%", series: [100, 250, 180, 420, 360, 580, 720, 690, 880] },
    2: { name: "Grid Master BTC", pair: "BTC/USDT", strategy: "grid", strategyLabel: "Grid Strategy", icon: "grid-3x3", grad: ["from-indigo-500", "to-blue-600"], status: "running", profit: "+$8,920", roi: "+17.8% ROI", winRate: "76.8%", trades: "98 / 128 trades", duration: "18m", drawdown: "-6.2%", series: [80, 160, 240, 200, 320, 380, 460, 520, 610] },
    3: { name: "DCA Accumulator", pair: "ETH/USDT", strategy: "dca", strategyLabel: "DCA Strategy", icon: "layers", grad: ["from-violet-500", "to-purple-600"], status: "running", profit: "+$5,670", roi: "+11.3% ROI", winRate: "91.3%", trades: "21 / 23 trades", duration: "2.4h", drawdown: "-3.1%", series: [60, 120, 150, 210, 260, 300, 360, 410, 470] },
    4: { name: "Martingale X", pair: "GBP/USD", strategy: "martingale", strategyLabel: "Martingale Strategy", icon: "repeat", grad: ["from-amber-500", "to-orange-600"], status: "paused", profit: "+$2,140", roi: "+6.8% ROI", winRate: "68.5%", trades: "57 / 83 trades", duration: "35m", drawdown: "-12.5%", series: [120, 90, 160, 140, 210, 180, 240, 220, 270] },
    5: { name: "High Risk Scalper", pair: "XAU/USD", strategy: "scalping", strategyLabel: "Scalping Strategy", icon: "zap", grad: ["from-red-500", "to-rose-600"], status: "stopped", profit: "-$340", roi: "-2.1% ROI", winRate: "42.1%", trades: "37 / 89 trades", duration: "3.1m", drawdown: "-18.7%", series: [200, 180, 240, 160, 190, 120, 150, 90, 110] },
    6: { name: "Crypto Grid Bot", pair: "SOL/USDT", strategy: "grid", strategyLabel: "Grid Strategy", icon: "bitcoin", grad: ["from-teal-500", "to-cyan-600"], status: "running", profit: "+$4,560", roi: "+14.2% ROI", winRate: "72.4%", trades: "48 / 67 trades", duration: "22m", drawdown: "-5.8%", series: [70, 140, 110, 220, 280, 250, 340, 380, 440] },
  };

  const overlay = document.getElementById("bcDrawerOverlay");
  const detailsDrawer = document.getElementById("bcDetailsDrawer");
  const formDrawer = document.getElementById("bcFormDrawer");
  const allDrawers = [detailsDrawer, formDrawer];

  const toast = document.getElementById("bcToast");
  const toastTitle = document.getElementById("bcToastTitle");
  const toastMessage = document.getElementById("bcToastMessage");

  const grid = document.getElementById("bcGrid");
  const bots = Array.from(document.querySelectorAll(".bc-bot"));
  const noResults = document.getElementById("bcNoResults");
  const searchInput = document.getElementById("bcSearch");

  let activeTab = "all"; // all | running | paused | stopped
  let activeChip = "all"; // all | scalping | grid | dca | martingale
  let searchTerm = "";

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
  document.querySelector(".bc-toast-close")?.addEventListener("click", hideToast);

  /* ======================================
   * 03. Drawers (details / create-edit form)
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
  document.querySelectorAll(".bc-close-drawer").forEach((btn) => btn.addEventListener("click", closeDrawers));

  /* ======================================
   * 04. Tabs + strategy chips + search (card filtering)
   * ====================================== */
  function matchesFilters(el) {
    const status = el.dataset.status;
    const strategy = el.dataset.strategy;
    const name = el.querySelector("h3")?.textContent.toLowerCase() || "";

    const tabMatch = activeTab === "all" || status === activeTab;
    const chipMatch = activeChip === "all" || strategy === activeChip;
    const searchMatch = !searchTerm || name.includes(searchTerm);
    return tabMatch && chipMatch && searchMatch;
  }

  function applyFilters() {
    let visible = 0;
    bots.forEach((bot) => {
      const show = matchesFilters(bot);
      bot.style.display = show ? "" : "none";
      if (show) visible += 1;
    });
    noResults?.classList.toggle("hidden", visible !== 0);
  }

  // Status tabs (shared .tab-button.active styling)
  document.querySelectorAll(".bc-tab").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll(".bc-tab").forEach((b) => {
        b.classList.remove("active");
        b.classList.add("text-muted");
      });
      btn.classList.add("active");
      btn.classList.remove("text-muted");
      activeTab = btn.dataset.tab;
      applyFilters();
    });
  });

  // Strategy filter chips
  document.querySelectorAll(".bc-chip").forEach((chip) => {
    chip.addEventListener("click", () => {
      document.querySelectorAll(".bc-chip").forEach((c) => {
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

  /* ======================================
   * 05. Bot status actions (start / pause / stop / delete)
   * ====================================== */
  const STATUS_DOT = { running: "bg-emerald-500", paused: "bg-amber-500", stopped: "bg-red-500" };
  const STATUS_TEXT = { running: "text-emerald-500", paused: "text-amber-500", stopped: "text-red-500" };
  const STATUS_LABEL = { running: "Running", paused: "Paused", stopped: "Stopped" };

  function setStatus(id, status) {
    const card = document.querySelector(`.bc-bot[data-id="${id}"]`);
    if (!card) return;
    card.dataset.status = status;
    if (BOTS[id]) BOTS[id].status = status;
    const dot = card.querySelector(".bc-status-dot");
    const label = card.querySelector(".bc-status-label");
    if (dot) {
      dot.classList.remove("bg-emerald-500", "bg-amber-500", "bg-red-500");
      dot.classList.add(STATUS_DOT[status]);
    }
    if (label) {
      label.classList.remove("text-emerald-500", "text-amber-500", "text-red-500");
      label.classList.add(STATUS_TEXT[status]);
      label.textContent = STATUS_LABEL[status];
    }
    applyFilters();
  }

  // Delegate via class-based listeners (controls are static)
  document.querySelectorAll(".bc-start-btn").forEach((btn) =>
    btn.addEventListener("click", () => {
      setStatus(btn.dataset.id, "running");
      showToast("Bot Started", `${BOTS[btn.dataset.id]?.name || "Bot"} is now running`);
    })
  );
  document.querySelectorAll(".bc-pause-btn").forEach((btn) =>
    btn.addEventListener("click", () => {
      setStatus(btn.dataset.id, "paused");
      showToast("Bot Paused", `${BOTS[btn.dataset.id]?.name || "Bot"} has been paused`);
    })
  );
  document.querySelectorAll(".bc-stop-btn").forEach((btn) =>
    btn.addEventListener("click", () => {
      setStatus(btn.dataset.id, "stopped");
      showToast("Bot Stopped", `${BOTS[btn.dataset.id]?.name || "Bot"} has been stopped`);
    })
  );
  document.querySelectorAll(".bc-delete-btn").forEach((btn) =>
    btn.addEventListener("click", () => {
      if (window.confirm("Are you sure you want to delete this bot?")) {
        const card = document.querySelector(`.bc-bot[data-id="${btn.dataset.id}"]`);
        if (card) card.style.display = "none";
        showToast("Bot Deleted", `${BOTS[btn.dataset.id]?.name || "Bot"} has been deleted`);
        applyFilters();
      }
    })
  );

  /* ======================================
   * 06. Details drawer populate + per-open chart
   * ====================================== */
  const dIconWrap = document.getElementById("bcDetailIconWrap");
  const dIcon = document.getElementById("bcDetailIcon");
  const dName = document.getElementById("bcDetailName");
  const dSub = document.getElementById("bcDetailSub");
  const dStatusDot = document.getElementById("bcDetailStatusDot");
  const dStatusLabel = document.getElementById("bcDetailStatusLabel");
  const dProfit = document.getElementById("bcDetailProfit");
  const dRoi = document.getElementById("bcDetailRoi");
  const dWinRate = document.getElementById("bcDetailWinRate");
  const dTradesRatio = document.getElementById("bcDetailTradesRatio");
  const dDuration = document.getElementById("bcDetailDuration");
  const dDrawdown = document.getElementById("bcDetailDrawdown");
  const dPair1 = document.getElementById("bcDetailTradePair1");
  const dPair2 = document.getElementById("bcDetailTradePair2");
  const dPair3 = document.getElementById("bcDetailTradePair3");

  let detailChart = null;
  let currentId = "1";

  function gradClasses() {
    // collect all gradient classes used so we can clear before re-adding
    const all = new Set();
    Object.values(BOTS).forEach((b) => b.grad.forEach((g) => all.add(g)));
    return Array.from(all);
  }
  const ALL_GRADS = gradClasses();

  function openDetails(id) {
    const b = BOTS[id];
    if (!b) return;
    currentId = id;
    const profit = !b.profit.startsWith("-");

    // Icon tile gradient
    if (dIconWrap) {
      dIconWrap.classList.remove(...ALL_GRADS, "bg-gradient-to-br");
      dIconWrap.classList.add("bg-gradient-to-br", ...b.grad);
    }
    dIcon?.setAttribute("data-lucide", b.icon);

    if (dName) dName.textContent = b.name;
    if (dSub) dSub.textContent = `${b.pair} • ${b.strategyLabel}`;

    // Status
    if (dStatusDot) {
      dStatusDot.classList.remove("bg-emerald-500", "bg-amber-500", "bg-red-500");
      dStatusDot.classList.add(STATUS_DOT[b.status]);
    }
    if (dStatusLabel) {
      dStatusLabel.classList.remove("text-emerald-500", "text-amber-500", "text-red-500");
      dStatusLabel.classList.add(STATUS_TEXT[b.status]);
      dStatusLabel.textContent = STATUS_LABEL[b.status];
    }

    if (dProfit) {
      dProfit.textContent = b.profit;
      dProfit.classList.remove("text-emerald-500", "text-red-500");
      dProfit.classList.add(profit ? "text-emerald-500" : "text-red-500");
    }
    if (dRoi) {
      dRoi.textContent = b.roi;
      dRoi.classList.remove("text-emerald-500", "text-red-500");
      dRoi.classList.add(profit ? "text-emerald-500" : "text-red-500");
    }
    if (dWinRate) dWinRate.textContent = b.winRate;
    if (dTradesRatio) dTradesRatio.textContent = b.trades;
    if (dDuration) dDuration.textContent = b.duration;
    if (dDrawdown) dDrawdown.textContent = b.drawdown;
    if (dPair1) dPair1.textContent = b.pair;
    if (dPair2) dPair2.textContent = b.pair;
    if (dPair3) dPair3.textContent = b.pair;

    openDrawer(detailsDrawer);
    renderDetailChart(b);
  }

  function renderDetailChart(b) {
    const canvas = document.getElementById("bcDetailChart");
    if (!canvas || typeof Chart === "undefined") return;
    if (detailChart) {
      detailChart.destroy();
      detailChart = null;
    }
    requestAnimationFrame(() => {
      const positive = !b.profit.startsWith("-");
      const color = positive ? "#10b981" : "#ef4444";
      detailChart = new Chart(canvas.getContext("2d"), {
        type: "line",
        data: {
          labels: b.series.map((_, i) => i + 1),
          datasets: [
            {
              data: b.series,
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

  document.querySelectorAll(".bc-details-btn").forEach((btn) => btn.addEventListener("click", () => openDetails(btn.dataset.id)));

  // Details drawer actions
  document.querySelector(".bc-detail-settings")?.addEventListener("click", () => openForm(currentId));
  document.querySelector(".bc-detail-stop")?.addEventListener("click", () => {
    setStatus(currentId, "stopped");
    closeDrawers();
    showToast("Bot Stopped", `${BOTS[currentId]?.name || "Bot"} has been stopped`);
  });

  /* ======================================
   * 07. Create / Edit form drawer
   * ====================================== */
  const fTitle = document.getElementById("bcFormTitle");
  const fSubtitle = document.getElementById("bcFormSubtitle");
  const fBanner = document.getElementById("bcFormBanner");
  const fBannerIconWrap = document.getElementById("bcFormBannerIconWrap");
  const fBannerIcon = document.getElementById("bcFormBannerIcon");
  const fBannerName = document.getElementById("bcFormBannerName");
  const fBannerSub = document.getElementById("bcFormBannerSub");
  const fName = document.getElementById("bcFormName");
  const fPair = document.getElementById("bcFormPair");
  const fStrategy = document.getElementById("bcFormStrategy");
  const fSubmitIcon = document.getElementById("bcFormSubmitIcon");
  const fSubmitLabel = document.getElementById("bcFormSubmitLabel");
  const strategyPanels = Array.from(document.querySelectorAll(".bc-strategy-panel"));

  let formMode = "create"; // create | edit

  function showStrategyPanel(strategy) {
    strategyPanels.forEach((p) => p.classList.toggle("hidden", p.dataset.strategy !== strategy));
  }
  fStrategy?.addEventListener("change", () => showStrategyPanel(fStrategy.value));

  function openForm(id) {
    if (id != null && BOTS[id]) {
      // Edit mode
      formMode = "edit";
      const b = BOTS[id];
      currentId = id;
      if (fTitle) fTitle.textContent = "Edit Bot Settings";
      if (fSubtitle) fSubtitle.textContent = "Modify your bot configuration";
      fBanner?.classList.remove("hidden");
      if (fBannerIconWrap) {
        fBannerIconWrap.classList.remove(...ALL_GRADS, "bg-gradient-to-br");
        fBannerIconWrap.classList.add("bg-gradient-to-br", ...b.grad);
      }
      fBannerIcon?.setAttribute("data-lucide", b.icon);
      if (fBannerName) fBannerName.textContent = b.name;
      if (fBannerSub) fBannerSub.textContent = `${b.pair} • ${b.strategyLabel}`;
      if (fName) fName.value = b.name;
      if (fPair) fPair.value = b.pair;
      if (fStrategy) fStrategy.value = b.strategy;
      showStrategyPanel(b.strategy);
      if (fSubmitIcon) fSubmitIcon.setAttribute("data-lucide", "save");
      if (fSubmitLabel) fSubmitLabel.textContent = "Save Changes";
    } else {
      // Create mode
      formMode = "create";
      if (fTitle) fTitle.textContent = "Create New Bot";
      if (fSubtitle) fSubtitle.textContent = "Configure your automated trading bot";
      fBanner?.classList.add("hidden");
      if (fName) fName.value = "";
      if (fStrategy) fStrategy.value = "scalping";
      showStrategyPanel("scalping");
      if (fSubmitIcon) fSubmitIcon.setAttribute("data-lucide", "plus");
      if (fSubmitLabel) fSubmitLabel.textContent = "Create Bot";
    }
    openDrawer(formDrawer);
  }

  document.getElementById("bcCreateBtn")?.addEventListener("click", () => openForm(null));
  document.querySelectorAll(".bc-edit-btn").forEach((btn) => btn.addEventListener("click", () => openForm(btn.dataset.id)));

  document.querySelector(".bc-form-submit")?.addEventListener("click", () => {
    closeDrawers();
    if (formMode === "edit") {
      showToast("Settings Saved", "Bot settings have been updated");
    } else {
      showToast("Bot Created", "Your new trading bot has been created successfully");
    }
  });

  /* ======================================
   * 08. Export menu + misc
   * ====================================== */
  const exportBtn = document.getElementById("bcExportBtn");
  const exportMenu = document.getElementById("bcExportMenu");
  exportBtn?.addEventListener("click", (e) => {
    e.stopPropagation();
    exportMenu?.classList.toggle("hidden");
  });
  document.addEventListener("click", () => exportMenu?.classList.add("hidden"));
  document.querySelectorAll(".bc-export-item").forEach((item) => {
    item.addEventListener("click", () => {
      const fmt = (item.dataset.format || "csv").toUpperCase();
      exportMenu?.classList.add("hidden");
      showToast("Export Started", `Exporting bots as ${fmt}...`);
      setTimeout(() => showToast("Export Complete", `Data exported successfully as ${fmt}`), 1500);
    });
  });

  document.getElementById("bcViewAllBtn")?.addEventListener("click", () => {
    showToast("Bot Activity", "Loading full bot activity history");
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
  refreshIcons();
});
