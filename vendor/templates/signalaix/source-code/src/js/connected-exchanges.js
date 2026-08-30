/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: connected-exchanges.html
 * description: SignalAIX - Connected Exchanges (Account) Page Controller
 *              Self-contained; mirrors the reference mockup's functionality
 *              (status tabs Crypto/Forex/Inactive + search + status-filter menu +
 *               export menu + refresh; exchange cards with balance show/hide,
 *               per-card sync + disconnect (removes node), Manage -> detail drawer
 *               populated from an EXCHANGES map; masked API key show/hide + copy;
 *               Connect-Exchange drawer with selectable exchanges (passphrase shown
 *               for coinbase/kucoin/okx), secret show/hide, test/connect; activity
 *               table exchange filter + export; toasts).
 *              All markup lives in connected-exchanges.html; this file only modifies
 *              the DOM (text/values/classes/visibility, swap data-lucide eye<->eye-off,
 *              remove nodes) — it never injects HTML strings. No Chart.js on this page.
 * author: SignalAIX

    -------------------------------------------------
     01. Init & DOM refs (guarded by #connected-exchanges)
     -------------------------------------------------
     02. Toast + copy helper
     -------------------------------------------------
     03. Drawers + dropdown menus (filter / export)
     -------------------------------------------------
     04. Tabs + search + status filter (card visibility)
     -------------------------------------------------
     05. Card actions: balance show/hide, sync, disconnect, manage
     -------------------------------------------------
     06. Manage drawer: populate, masked key show/hide + copy, secret edit, save
     -------------------------------------------------
     07. Connect drawer: select exchange, secret show/hide, test, connect
     -------------------------------------------------
     08. Activity table filter + export + view log; refresh; keyboard
     -------------------------------------------------
    ================================================== */

document.addEventListener("DOMContentLoaded", () => {
  /* ======================================
   * 01. Init & DOM refs
   * ====================================== */
  const page = document.getElementById("connected-exchanges");
  if (!page) return; // Guard: only run on the Connected Exchanges page

  const refreshIcons = () => window.lucide?.createIcons?.();

  const overlay = document.getElementById("ceDrawerOverlay");
  const addDrawer = document.getElementById("ceAddDrawer");
  const manageDrawer = document.getElementById("ceManageDrawer");
  const allDrawers = [addDrawer, manageDrawer];

  const toast = document.getElementById("ceToast");
  const toastTitle = document.getElementById("ceToastTitle");
  const toastMessage = document.getElementById("ceToastMessage");

  // Static per-exchange metadata used to populate the Manage drawer (DOM-only).
  const EXCHANGES = {
    binance: { name: "Binance", type: "Spot & Futures", logo: "B", color: "#F0B90B", balance: "$45,230.85", conn: "My Binance Account", keyMask: "VmFy••••••••x8Kp", keyFull: "VmFyaW91c0FQSUtleUhlcmUx", status: "connected" },
    coinbase: { name: "Coinbase Pro", type: "Spot Trading", logo: "C", color: "#0052FF", balance: "$28,450.20", conn: "My Coinbase Account", keyMask: "CbPr••••••••9aL2", keyFull: "CbProAPIKeyExample9aL2", status: "connected" },
    kraken: { name: "Kraken", type: "Spot & Margin", logo: "K", color: "#5741D9", balance: "$18,920.45", conn: "My Kraken Account", keyMask: "KrKn••••••••3mQ7", keyFull: "KraKenAPIKeyExample3mQ7", status: "syncing" },
    bybit: { name: "ByBit", type: "Derivatives", logo: "BY", color: "#F7A600", balance: "$22,380.00", conn: "My ByBit Account", keyMask: "ByBt••••••••4kP1", keyFull: "ByBitAPIKeyExample4kP1", status: "connected" },
    oanda: { name: "OANDA", type: "Forex & CFD", logo: "O", color: "#009BAD", balance: "$9,599.50", conn: "My OANDA Account", keyMask: "OnDa••••••••7vR9", keyFull: "OandaAPIKeyExample7vR9", status: "connected" },
  };

  /* ======================================
   * 02. Toast + copy helper
   * ====================================== */
  let toastTimer;
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
  document.querySelector(".ce-toast-close")?.addEventListener("click", hideToast);

  function copyText(text, msg) {
    navigator.clipboard?.writeText(text).then(
      () => showToast("Copied", msg),
      () => showToast("Copied", msg),
    );
  }

  /* ======================================
   * 03. Drawers + dropdown menus
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
  document.querySelectorAll(".ce-close-drawer").forEach((btn) => btn.addEventListener("click", closeDrawers));

  // Filter dropdown
  const filterMenu = document.getElementById("ceFilterMenu");
  document.getElementById("ceFilterBtn")?.addEventListener("click", (e) => {
    e.stopPropagation();
    filterMenu?.classList.toggle("hidden");
    exportMenu?.classList.add("hidden");
  });

  // Export dropdown
  const exportMenu = document.getElementById("ceExportMenu");
  document.getElementById("ceExportBtn")?.addEventListener("click", (e) => {
    e.stopPropagation();
    exportMenu?.classList.toggle("hidden");
    filterMenu?.classList.add("hidden");
  });
  document.addEventListener("click", () => {
    exportMenu?.classList.add("hidden");
    filterMenu?.classList.add("hidden");
  });
  filterMenu?.addEventListener("click", (e) => e.stopPropagation());
  document.querySelectorAll(".ce-export-item").forEach((item) => {
    item.addEventListener("click", () => {
      exportMenu?.classList.add("hidden");
      showToast("Export Started", `Exporting exchanges as ${(item.dataset.format || "").toUpperCase()}...`);
    });
  });

  /* ======================================
   * 04. Tabs + search + status filter (card visibility)
   * ====================================== */
  const cards = Array.from(document.querySelectorAll(".ce-card"));
  const addTile = document.getElementById("ceAddTile");
  const noResults = document.getElementById("ceNoResults");
  const statusChecks = Array.from(document.querySelectorAll(".ce-filter-status"));
  let tabFilter = "all"; // all | crypto | forex | inactive
  let search = "";

  function allowedStatuses() {
    return statusChecks.filter((c) => c.checked).map((c) => c.dataset.status);
  }

  function applyFilters() {
    const allowed = allowedStatuses();
    let visible = 0;
    cards.forEach((card) => {
      const type = card.dataset.type;
      const status = card.dataset.status;
      const name = (card.dataset.name || "").toLowerCase();

      let tabMatch = true;
      if (tabFilter === "crypto") tabMatch = type === "crypto";
      else if (tabFilter === "forex") tabMatch = type === "forex";
      else if (tabFilter === "inactive") tabMatch = status === "disconnected";

      const statusMatch = allowed.includes(status);
      const searchMatch = !search || name.includes(search);
      const show = tabMatch && statusMatch && searchMatch;
      card.style.display = show ? "" : "none";
      if (show) visible += 1;
    });
    // Hide the add tile when filtering (only show on the "all" view with no search)
    if (addTile) addTile.style.display = tabFilter === "all" && !search ? "" : "none";
    noResults?.classList.toggle("hidden", visible !== 0);
  }

  document.querySelectorAll(".ce-tab").forEach((tab) => {
    tab.addEventListener("click", () => {
      document.querySelectorAll(".ce-tab").forEach((t) => {
        t.classList.toggle("active", t === tab);
        t.classList.toggle("text-muted", t !== tab);
      });
      tabFilter = tab.dataset.tab;
      applyFilters();
    });
  });

  document.getElementById("ceSearch")?.addEventListener("input", (e) => {
    search = e.target.value.toLowerCase();
    applyFilters();
  });

  statusChecks.forEach((cb) =>
    cb.addEventListener("change", () => {
      applyFilters();
      showToast("Filters Applied", "Exchange list has been filtered");
    }),
  );

  /* ======================================
   * 05. Card actions: balance show/hide, sync, disconnect, manage
   * ====================================== */
  // Balance show/hide (DOM-only: toggle blur + swap eye<->eye-off)
  document.querySelectorAll(".ce-balance-eye").forEach((btn) => {
    btn.addEventListener("click", () => {
      const value = btn.closest(".ce-card")?.querySelector(".ce-balance");
      const icon = btn.querySelector("i");
      if (!value || !icon) return;
      const hidden = value.classList.toggle("blur-sm");
      icon.setAttribute("data-lucide", hidden ? "eye-off" : "eye");
      btn.title = hidden ? "Show balance" : "Hide balance";
      refreshIcons();
    });
  });

  // Per-card sync
  document.querySelectorAll(".ce-sync").forEach((btn) => {
    btn.addEventListener("click", () => {
      const id = btn.closest(".ce-card")?.dataset.exchange;
      const meta = EXCHANGES[id];
      const label = meta ? meta.name : id;
      const icon = btn.querySelector("i");
      icon?.classList.add("animate-spin");
      showToast("Syncing...", `Synchronizing ${label} data`);
      setTimeout(() => {
        icon?.classList.remove("animate-spin");
        showToast("Sync Complete", `${label} data has been updated`);
      }, 1500);
    });
  });

  // Per-card disconnect (removes the card node)
  document.querySelectorAll(".ce-disconnect").forEach((btn) => {
    btn.addEventListener("click", () => {
      const card = btn.closest(".ce-card");
      const id = card?.dataset.exchange;
      const meta = EXCHANGES[id];
      const label = meta ? meta.name : id;
      card?.remove();
      showToast("Disconnected", `${label} has been disconnected from your account`);
      applyFilters();
    });
  });

  // Add tile + header button -> connect drawer
  document.getElementById("ceAddBtn")?.addEventListener("click", () => openDrawer(addDrawer));
  addTile?.addEventListener("click", () => openDrawer(addDrawer));

  /* ======================================
   * 06. Manage drawer: populate + key show/hide + copy + secret edit + save
   * ====================================== */
  const mgLogo = document.getElementById("ceManageLogo");
  const mgName = document.getElementById("ceManageName");
  const mgType = document.getElementById("ceManageType");
  const mgBalance = document.getElementById("ceManageBalance");
  const mgConn = document.getElementById("ceManageConnName");
  const mgKey = document.getElementById("ceManageKey");
  const mgStatusBox = document.getElementById("ceManageStatusBox");
  const mgStatusDot = document.getElementById("ceManageStatusDot");
  const mgStatusLabel = document.getElementById("ceManageStatusLabel");
  let manageId = "binance";

  function openManage(id) {
    const meta = EXCHANGES[id];
    if (!meta) return;
    manageId = id;
    if (mgName) mgName.textContent = meta.name;
    if (mgType) mgType.textContent = meta.type;
    if (mgBalance) mgBalance.textContent = meta.balance;
    if (mgConn) mgConn.value = meta.conn;
    if (mgLogo) {
      mgLogo.style.backgroundColor = `${meta.color}26`; // ~15% alpha
      const span = mgLogo.querySelector("span");
      if (span) {
        span.textContent = meta.logo;
        span.style.color = meta.color;
      }
    }
    if (mgKey) {
      mgKey.dataset.mask = meta.keyMask;
      mgKey.dataset.full = meta.keyFull;
      mgKey.dataset.revealed = "false";
      mgKey.textContent = meta.keyMask;
    }
    // status box colors
    const syncing = meta.status === "syncing";
    if (mgStatusBox)
      mgStatusBox.className = syncing
        ? "rounded-xl bg-amber-500/10 border border-amber-500/20 p-4 flex flex-col xs:flex-row xs:items-center xs:justify-between gap-3"
        : "rounded-xl bg-emerald-500/10 border border-emerald-500/20 p-4 flex flex-col xs:flex-row xs:items-center xs:justify-between gap-3";
    if (mgStatusDot) mgStatusDot.className = `w-2.5 h-2.5 rounded-full shrink-0 ${syncing ? "bg-amber-500" : "bg-emerald-500"}`;
    if (mgStatusLabel) mgStatusLabel.textContent = syncing ? "Syncing" : "Connected";
    // reset secret edit box
    document.getElementById("ceManageSecretEditBox")?.classList.add("hidden");
    openDrawer(manageDrawer);
  }

  document.querySelectorAll(".ce-manage").forEach((btn) => {
    btn.addEventListener("click", () => openManage(btn.closest(".ce-card")?.dataset.exchange));
  });

  // Masked API key show/hide
  document.getElementById("ceManageKeyEye")?.addEventListener("click", function () {
    const icon = this.querySelector("i");
    if (!mgKey || !icon) return;
    const revealed = mgKey.dataset.revealed === "true";
    if (revealed) {
      mgKey.textContent = mgKey.dataset.mask;
      mgKey.dataset.revealed = "false";
      icon.setAttribute("data-lucide", "eye");
      this.title = "Show";
    } else {
      mgKey.textContent = mgKey.dataset.full;
      mgKey.dataset.revealed = "true";
      icon.setAttribute("data-lucide", "eye-off");
      this.title = "Hide";
    }
    refreshIcons();
  });

  // Copy API key
  document.getElementById("ceManageKeyCopy")?.addEventListener("click", () => {
    if (mgKey) copyText(mgKey.dataset.full || mgKey.textContent, "API key copied to clipboard");
  });

  // Secret edit toggle
  document.getElementById("ceManageSecretEdit")?.addEventListener("click", () => {
    document.getElementById("ceManageSecretEditBox")?.classList.toggle("hidden");
  });
  document.getElementById("ceManageSecretInputEye")?.addEventListener("click", function () {
    togglePassword(document.getElementById("ceManageSecretInput"), this);
  });

  // Sync now (inside drawer)
  document.getElementById("ceManageSync")?.addEventListener("click", function () {
    const meta = EXCHANGES[manageId];
    const icon = this.querySelector("i");
    icon?.classList.add("animate-spin");
    showToast("Syncing...", `Synchronizing ${meta ? meta.name : manageId} data`);
    setTimeout(() => {
      icon?.classList.remove("animate-spin");
      showToast("Sync Complete", `${meta ? meta.name : manageId} data has been updated`);
    }, 1500);
  });

  // Save changes
  document.getElementById("ceManageSave")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Settings Saved", "Exchange settings have been updated");
  });

  // Disconnect from drawer (removes matching card)
  document.getElementById("ceManageDisconnect")?.addEventListener("click", () => {
    const meta = EXCHANGES[manageId];
    document.querySelector(`.ce-card[data-exchange="${manageId}"]`)?.remove();
    closeDrawers();
    showToast("Disconnected", `${meta ? meta.name : manageId} has been disconnected from your account`);
    applyFilters();
  });

  /* ======================================
   * 07. Connect drawer: select exchange, secret show/hide, test, connect
   * ====================================== */
  let selectedExchange = null;
  const passphraseField = document.getElementById("cePassphraseField");
  document.querySelectorAll(".ce-exchange-option").forEach((opt) => {
    opt.addEventListener("click", () => {
      document.querySelectorAll(".ce-exchange-option").forEach((o) => {
        o.classList.remove("border-accent", "bg-accent/10");
        o.classList.add("border-border");
      });
      opt.classList.add("border-accent", "bg-accent/10");
      opt.classList.remove("border-border");
      selectedExchange = opt.dataset.exchange;
      passphraseField?.classList.toggle("hidden", opt.dataset.passphrase !== "true");
    });
  });

  function togglePassword(input, btn) {
    const icon = btn?.querySelector("i");
    if (!input || !icon) return;
    if (input.type === "password") {
      input.type = "text";
      icon.setAttribute("data-lucide", "eye-off");
      btn.title = "Hide";
    } else {
      input.type = "password";
      icon.setAttribute("data-lucide", "eye");
      btn.title = "Show";
    }
    refreshIcons();
  }
  document.getElementById("ceSecretEye")?.addEventListener("click", function () {
    togglePassword(document.getElementById("ceApiSecretInput"), this);
  });

  document.getElementById("ceTestBtn")?.addEventListener("click", () => {
    showToast("Testing Connection", "Verifying API credentials...");
    setTimeout(() => showToast("Connection Successful", "API credentials are valid"), 1500);
  });

  document.getElementById("ceConnectBtn")?.addEventListener("click", () => {
    const apiKey = document.getElementById("ceApiKeyInput")?.value.trim();
    const apiSecret = document.getElementById("ceApiSecretInput")?.value.trim();
    if (!selectedExchange || !apiKey || !apiSecret) {
      showToast("Missing Information", "Please select an exchange and fill in API key & secret");
      return;
    }
    const meta = EXCHANGES[selectedExchange];
    const label = meta ? meta.name : selectedExchange;
    showToast("Connecting...", "Setting up exchange connection");
    setTimeout(() => {
      closeDrawers();
      showToast("Exchange Connected", `${label} has been connected successfully`);
    }, 1500);
  });

  /* ======================================
   * 08. Activity table filter + export + view log; refresh; keyboard
   * ====================================== */
  const activityRows = Array.from(document.querySelectorAll(".ce-activity-row"));
  document.getElementById("ceActivityFilter")?.addEventListener("change", (e) => {
    const val = e.target.value;
    activityRows.forEach((row) => {
      row.style.display = val === "all" || row.dataset.exchange === val ? "" : "none";
    });
    showToast("Filter Applied", `Showing ${val === "all" ? "all exchanges" : val} activity`);
  });
  document.getElementById("ceActivityExport")?.addEventListener("click", () => {
    showToast("Export Started", "Exporting API activity log...");
    setTimeout(() => showToast("Export Complete", "Activity log exported to api_activity.csv"), 1500);
  });
  document.querySelectorAll(".ce-activity-view").forEach((btn) => {
    btn.addEventListener("click", () => showToast("API Log", `Viewing log entry #${btn.dataset.log}`));
  });

  // Header refresh
  document.getElementById("ceRefreshBtn")?.addEventListener("click", function () {
    const icon = this.querySelector("i");
    icon?.classList.add("animate-spin");
    showToast("Refreshing...", "Synchronizing all exchange data");
    setTimeout(() => {
      icon?.classList.remove("animate-spin");
      showToast("Refresh Complete", "All exchange data has been synchronized");
    }, 1500);
  });

  // Keyboard
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      closeDrawers();
      exportMenu?.classList.add("hidden");
      filterMenu?.classList.add("hidden");
    }
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
      e.preventDefault();
      document.getElementById("ceSearch")?.focus();
    }
  });

  // Initial render
  applyFilters();
  refreshIcons();
});
