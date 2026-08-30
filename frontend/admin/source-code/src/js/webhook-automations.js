/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: webhook-automations.html
 * description: SignalAIX - Webhook Automations Page Controller
 *              Self-contained; mirrors the reference mockup's functionality
 *              (5 tabs Endpoints/Incoming/Outgoing/Logs/Templates, endpoint
 *               search + status filter chips, per-webhook enable/disable toggle +
 *               status dot, copy endpoint URL / payload code to clipboard, test /
 *               retry / view-logs card actions, log search + status-class chips +
 *               log-detail drawer, create/edit-webhook form drawer (edit reuses
 *               it) with auth-field toggle + event/type selectors, filter drawer,
 *               templates use/preview, export menu, toast).
 *              All markup lives in webhook-automations.html; this file only
 *              modifies the DOM (text/values/classes/visibility) and copies to
 *              clipboard — it never injects HTML strings.
 * author: SignalAIX

    -------------------------------------------------
     01. Init & DOM refs (guarded by #webhook-automations)
     -------------------------------------------------
     02. Toast
     -------------------------------------------------
     03. Drawers + export menu
     -------------------------------------------------
     04. Tabs (panes)
     -------------------------------------------------
     05. Endpoints: search + status chips + card actions + toggle
     -------------------------------------------------
     06. Copy-to-clipboard (URLs + code blocks)
     -------------------------------------------------
     07. Logs: search + status chips + view detail + retry
     -------------------------------------------------
     08. Outgoing table actions + templates
     -------------------------------------------------
     09. Create / Edit form drawer + filter drawer + keyboard
     -------------------------------------------------
    ================================================== */

document.addEventListener("DOMContentLoaded", () => {
  /* ======================================
   * 01. Init & DOM refs
   * ====================================== */
  const page = document.getElementById("webhook-automations");
  if (!page) return; // Guard: only run on the Webhook Automations page

  const refreshIcons = () => window.lucide?.createIcons?.();

  const overlay = document.getElementById("whDrawerOverlay");
  const formDrawer = document.getElementById("whFormDrawer");
  const logDrawer = document.getElementById("whLogDrawer");
  const filterDrawer = document.getElementById("whFilterDrawer");
  const allDrawers = [formDrawer, logDrawer, filterDrawer];

  const toast = document.getElementById("whToast");
  const toastTitle = document.getElementById("whToastTitle");
  const toastMessage = document.getElementById("whToastMessage");

  /* ======================================
   * 02. Toast
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
  document.querySelector(".wh-toast-close")?.addEventListener("click", hideToast);

  /* ======================================
   * 03. Drawers + export menu
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
  document.querySelectorAll(".wh-close-drawer").forEach((btn) => btn.addEventListener("click", closeDrawers));

  document.getElementById("whFilterBtn")?.addEventListener("click", () => openDrawer(filterDrawer));

  const exportMenu = document.getElementById("whExportMenu");
  document.getElementById("whExportBtn")?.addEventListener("click", (e) => {
    e.stopPropagation();
    exportMenu?.classList.toggle("hidden");
  });
  document.addEventListener("click", () => exportMenu?.classList.add("hidden"));
  document.querySelectorAll(".wh-export-item").forEach((item) => {
    item.addEventListener("click", () => {
      exportMenu?.classList.add("hidden");
      showToast("Export Started", `Exporting webhooks as ${item.dataset.format.toUpperCase()}`);
    });
  });

  /* ======================================
   * 04. Tabs (panes)
   * ====================================== */
  const tabs = Array.from(document.querySelectorAll(".wh-tab"));
  const panes = Array.from(document.querySelectorAll(".wh-pane"));
  function activateTab(name) {
    tabs.forEach((t) => {
      const on = t.dataset.tab === name;
      t.classList.toggle("active", on);
      t.classList.toggle("text-muted", !on);
    });
    panes.forEach((p) => p.classList.toggle("active", p.dataset.pane === name));
    refreshIcons();
  }
  tabs.forEach((t) => t.addEventListener("click", () => activateTab(t.dataset.tab)));

  /* ======================================
   * 05. Endpoints: search + status chips + card actions + toggle
   * ====================================== */
  const grid = document.getElementById("whGrid");
  const noResults = document.getElementById("whNoResults");
  const cards = Array.from(document.querySelectorAll(".wh-card"));
  const endpointSearch = document.getElementById("whEndpointSearch");
  let activeStatus = "all"; // all | active | paused | error
  let searchTerm = "";

  function matchesCard(card) {
    const status = card.dataset.status;
    const statusMatch = activeStatus === "all" || status === activeStatus;
    const text = (card.dataset.name + " " + card.dataset.url + " " + card.dataset.dir).toLowerCase();
    const searchMatch = !searchTerm || text.includes(searchTerm);
    return statusMatch && searchMatch;
  }
  function applyCardFilters() {
    let visible = 0;
    cards.forEach((card) => {
      const show = matchesCard(card);
      card.style.display = show ? "" : "none";
      if (show) visible += 1;
    });
    if (noResults) noResults.classList.toggle("hidden", visible !== 0);
  }

  document.querySelectorAll(".wh-chip").forEach((chip) => {
    chip.addEventListener("click", () => {
      document.querySelectorAll(".wh-chip").forEach((c) => {
        c.classList.remove("active", "border-accent", "bg-accent/15", "text-accent");
        c.classList.add("border-border", "bg-panel", "text-muted");
      });
      chip.classList.add("active", "border-accent", "bg-accent/15", "text-accent");
      chip.classList.remove("border-border", "bg-panel", "text-muted");
      activeStatus = chip.dataset.filter;
      applyCardFilters();
    });
  });
  endpointSearch?.addEventListener("input", (e) => {
    searchTerm = e.target.value.toLowerCase();
    applyCardFilters();
  });

  // Per-card enable/disable toggle (+ status dot repaint)
  function repaintDot(card) {
    const dot = card.querySelector(".wh-card-dot");
    if (!dot) return;
    const map = { active: "bg-emerald-500", paused: "bg-amber-500", error: "bg-red-500" };
    dot.className = `wh-card-dot w-2 h-2 rounded-full ${map[card.dataset.status] || "bg-amber-500"}`;
  }
  cards.forEach((card) => {
    const toggle = card.querySelector(".wh-card-toggle");
    toggle?.addEventListener("click", (e) => {
      e.stopPropagation();
      const on = toggle.classList.toggle("active");
      toggle.setAttribute("aria-checked", on ? "true" : "false");
      card.dataset.status = on ? "active" : "paused";
      repaintDot(card);
      applyCardFilters();
      showToast("Status Updated", `${card.dataset.name} ${on ? "enabled" : "disabled"}`);
    });
    toggle?.addEventListener("keydown", (e) => {
      if (e.key === " " || e.key === "Enter") {
        e.preventDefault();
        toggle.click();
      }
    });

    card.querySelector(".wh-test-btn")?.addEventListener("click", () => {
      showToast("Testing Webhook", "Sending test request...");
      setTimeout(() => showToast("Test Successful", `${card.dataset.name} responded 200 OK`), 1500);
    });
    card.querySelector(".wh-retry-btn")?.addEventListener("click", () => {
      showToast("Retrying", `Attempting to reconnect ${card.dataset.name}...`);
    });
    card.querySelector(".wh-config-btn")?.addEventListener("click", () => openEditForm(card.dataset.name));
    card.querySelector(".wh-logs-btn")?.addEventListener("click", () => activateTab("logs"));
  });

  document.getElementById("whAddCard")?.addEventListener("click", () => openCreateForm());

  /* ======================================
   * 06. Copy-to-clipboard
   * ====================================== */
  function copyText(text, msg) {
    if (navigator.clipboard?.writeText) {
      navigator.clipboard.writeText(text).then(
        () => showToast("Copied", msg),
        () => showToast("Copy Failed", "Could not access clipboard"),
      );
    } else {
      showToast("Copied", msg);
    }
  }

  // Card endpoint URLs (use the full data-url, not the truncated display)
  document.querySelectorAll(".wh-copy-url").forEach((btn) => {
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      const card = btn.closest(".wh-card");
      const url = card?.dataset.url || btn.previousElementSibling?.textContent?.trim() || "";
      copyText(url, "Endpoint URL copied to clipboard");
    });
  });

  // Main incoming URL
  document.getElementById("whCopyMainUrl")?.addEventListener("click", () => {
    const url = document.getElementById("whMainUrl")?.textContent?.trim() || "";
    copyText(url, "Webhook URL copied to clipboard");
  });

  // Code blocks (payload / headers)
  document.querySelectorAll(".wh-copy-code").forEach((btn) => {
    btn.addEventListener("click", () => {
      const block = btn.closest("div").parentElement;
      const code = block?.querySelector(".wh-code")?.textContent || "";
      copyText(code, "Code copied to clipboard");
      const label = btn.querySelector(".wh-copy-code-label");
      if (label) {
        label.textContent = "Copied";
        setTimeout(() => (label.textContent = "Copy"), 2000);
      }
    });
  });

  /* ======================================
   * 07. Logs: search + status chips + view detail + retry
   * ====================================== */
  const logs = Array.from(document.querySelectorAll(".wh-log"));
  const logsNoResults = document.getElementById("whLogsNoResults");
  let activeLog = "all"; // all | 2xx | 4xx | 5xx
  let logSearch = "";

  function applyLogFilters() {
    let visible = 0;
    logs.forEach((log) => {
      const classMatch = activeLog === "all" || log.dataset.class === activeLog;
      const text = (log.dataset.name + " " + log.dataset.message + " " + log.dataset.code).toLowerCase();
      const searchMatch = !logSearch || text.includes(logSearch);
      const show = classMatch && searchMatch;
      log.style.display = show ? "" : "none";
      if (show) visible += 1;
    });
    if (logsNoResults) logsNoResults.classList.toggle("hidden", visible !== 0);
  }
  document.querySelectorAll(".wh-log-chip").forEach((chip) => {
    chip.addEventListener("click", () => {
      document.querySelectorAll(".wh-log-chip").forEach((c) => {
        c.classList.remove("active", "border-accent", "bg-accent/15", "text-accent");
        c.classList.add("border-border", "bg-panel", "text-muted");
      });
      chip.classList.add("active", "border-accent", "bg-accent/15", "text-accent");
      chip.classList.remove("border-border", "bg-panel", "text-muted");
      activeLog = chip.dataset.log;
      applyLogFilters();
    });
  });
  document.getElementById("whLogsSearch")?.addEventListener("input", (e) => {
    logSearch = e.target.value.toLowerCase();
    applyLogFilters();
  });

  // Log detail drawer
  const CODE_COLOR = {
    "2xx": "text-emerald-500",
    "4xx": "text-amber-500",
    "5xx": "text-red-500",
  };
  let activeLogEl = null;
  function openLogDetail(log) {
    activeLogEl = log;
    document.getElementById("whLogName").textContent = log.dataset.name;
    document.getElementById("whLogTime").textContent = log.dataset.time;
    document.getElementById("whLogMethod").textContent = log.dataset.method;
    document.getElementById("whLogDuration").textContent = log.dataset.duration;
    document.getElementById("whLogSize").textContent = log.dataset.size;
    document.getElementById("whLogMessage").textContent = log.dataset.message;
    const codeEl = document.getElementById("whLogCode");
    codeEl.textContent = log.dataset.code;
    codeEl.className = `text-lg font-bold font-mono ${CODE_COLOR[log.dataset.class] || "text-text"}`;
    // Show Retry only on failed (4xx/5xx) requests
    const retryBtn = document.getElementById("whLogRetryBtn");
    retryBtn?.classList.toggle("hidden", log.dataset.class === "2xx");
    openDrawer(logDrawer);
  }
  logs.forEach((log) => {
    log.querySelector(".wh-log-view")?.addEventListener("click", () => openLogDetail(log));
    log.querySelector(".wh-log-retry")?.addEventListener("click", () =>
      showToast("Retry Initiated", `Retrying ${log.dataset.name} request...`),
    );
  });
  document.getElementById("whLogRetryBtn")?.addEventListener("click", () => {
    closeDrawers();
    if (activeLogEl) showToast("Retry Initiated", `Retrying ${activeLogEl.dataset.name} request...`);
  });
  document.getElementById("whLoadMore")?.addEventListener("click", () =>
    showToast("Loading", "Fetching older log entries..."),
  );

  /* ======================================
   * 08. Outgoing table actions + templates
   * ====================================== */
  document.querySelectorAll(".wh-row-edit").forEach((btn) => {
    btn.addEventListener("click", () => openEditForm(btn.dataset.name));
  });
  document.querySelectorAll(".wh-row-delete").forEach((btn) => {
    btn.addEventListener("click", () => {
      const row = btn.closest("tr");
      row?.remove();
      showToast("Webhook Deleted", `${btn.dataset.name} has been deleted`);
    });
  });
  document.querySelectorAll(".wh-use-template").forEach((btn) => {
    btn.addEventListener("click", () => showToast("Template Applied", `${btn.dataset.template} template has been applied`));
  });
  document.querySelectorAll(".wh-preview-template").forEach((btn) => {
    btn.addEventListener("click", () => showToast("Preview", `Showing ${btn.dataset.template} template preview`));
  });
  document.getElementById("whCustomTemplate")?.addEventListener("click", () =>
    showToast("Custom Template", "Opening template editor..."),
  );

  /* ======================================
   * 09. Create / Edit form drawer + filter drawer + keyboard
   * ====================================== */
  const formTitle = document.getElementById("whFormTitle");
  const formSubmitLabel = document.getElementById("whFormSubmitLabel");
  const formName = document.getElementById("whFormName");
  let editingName = null;

  // Webhook type selector (single-select)
  document.querySelectorAll(".wh-type-btn").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll(".wh-type-btn").forEach((b) => {
        b.classList.remove("active", "border-accent", "bg-accent/15");
        b.classList.add("border-border");
      });
      btn.classList.add("active", "border-accent", "bg-accent/15");
      btn.classList.remove("border-border");
    });
  });

  // Trigger-event multi-select toggle
  document.querySelectorAll(".wh-event-btn").forEach((btn) => {
    btn.addEventListener("click", () => {
      const on = btn.classList.toggle("active");
      btn.classList.toggle("border-accent", on);
      btn.classList.toggle("bg-accent/15", on);
      btn.classList.toggle("text-accent", on);
      btn.classList.toggle("border-border", !on);
      btn.classList.toggle("bg-panel", !on);
      btn.classList.toggle("text-muted", !on);
    });
  });

  // Authentication field toggling
  const authType = document.getElementById("whAuthType");
  const apiKeyField = document.getElementById("whApiKeyField");
  const bearerField = document.getElementById("whBearerField");
  const basicFields = document.getElementById("whBasicFields");
  function syncAuthFields() {
    const v = authType?.value || "none";
    apiKeyField?.classList.toggle("hidden", v !== "api_key");
    bearerField?.classList.toggle("hidden", v !== "bearer");
    basicFields?.classList.toggle("hidden", v !== "basic");
  }
  authType?.addEventListener("change", syncAuthFields);

  function openCreateForm() {
    editingName = null;
    if (formTitle) formTitle.textContent = "Create New Webhook";
    if (formSubmitLabel) formSubmitLabel.textContent = "Save Webhook";
    if (formName) formName.value = "";
    openDrawer(formDrawer);
  }
  function openEditForm(name) {
    editingName = name;
    if (formTitle) formTitle.textContent = "Edit Webhook";
    if (formSubmitLabel) formSubmitLabel.textContent = "Save Changes";
    if (formName) formName.value = name || "";
    openDrawer(formDrawer);
  }
  document.getElementById("whNewBtn")?.addEventListener("click", () => openCreateForm());

  document.getElementById("whFormSubmit")?.addEventListener("click", () => {
    closeDrawers();
    if (editingName) showToast("Webhook Updated", `${editingName} changes saved`);
    else showToast("Webhook Created", "Your webhook has been created successfully");
  });
  document.getElementById("whFormTest")?.addEventListener("click", () =>
    showToast("Testing Configuration", "Validating webhook settings..."),
  );

  // Filter drawer
  document.querySelector(".wh-apply-filters")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Filters Applied", "Your filter settings have been applied");
  });
  document.querySelector(".wh-reset-filters")?.addEventListener("click", () => {
    filterDrawer?.querySelectorAll(".custom-checkbox").forEach((cb) => (cb.checked = true));
    filterDrawer?.querySelectorAll("input[type='number']").forEach((n) => (n.value = ""));
  });

  // Keyboard shortcuts
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      closeDrawers();
      exportMenu?.classList.add("hidden");
    }
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
      e.preventDefault();
      endpointSearch?.focus();
    }
  });

  // Initial render
  syncAuthFields();
  applyCardFilters();
  applyLogFilters();
  refreshIcons();
});
