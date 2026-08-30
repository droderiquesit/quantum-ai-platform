/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: auto-signal-forwarder.html
 * description: SignalAIX - Auto-Signal Forwarder Page Controller
 *              Self-contained; mirrors the reference mockup's functionality
 *              (4 tabs: Forwarding Rules / Channels / Activity Logs / Settings;
 *               status filter chips + search over rule cards; per-rule enable/
 *               disable toggle + edit + delete + retry; create-rule drawer that
 *               edit reuses; advanced filter drawer; activity-log filter chips +
 *               search; settings actions; export menu; toast).
 *              All markup lives in auto-signal-forwarder.html; this file only
 *              modifies the DOM (text/values/classes/visibility) — it never
 *              injects HTML strings.
 * author: SignalAIX

    -------------------------------------------------
     01. Init & DOM refs (guarded by #auto-signal-forwarder)
     -------------------------------------------------
     02. Toast
     -------------------------------------------------
     03. Drawers (form / filter) + export dropdown
     -------------------------------------------------
     04. Tabs (rules / channels / logs / settings)
     -------------------------------------------------
     05. Rules: status chips + filter drawer + search (card filtering)
     -------------------------------------------------
     06. Per-rule toggle / edit / delete / retry / selection
     -------------------------------------------------
     07. Create / Edit rule form drawer
     -------------------------------------------------
     08. Activity logs: chips + search
     -------------------------------------------------
     09. Channels + settings actions + keyboard shortcuts
     -------------------------------------------------
    ================================================== */

document.addEventListener("DOMContentLoaded", () => {
  /* ======================================
   * 01. Init & DOM refs
   * ====================================== */
  const page = document.getElementById("auto-signal-forwarder");
  if (!page) return; // Guard: only run on the Auto-Signal Forwarder page

  const refreshIcons = () => window.lucide?.createIcons?.();

  const overlay = document.getElementById("asfDrawerOverlay");
  const formDrawer = document.getElementById("asfFormDrawer");
  const filterDrawer = document.getElementById("asfFilterDrawer");
  const allDrawers = [formDrawer, filterDrawer];

  const toast = document.getElementById("asfToast");
  const toastTitle = document.getElementById("asfToastTitle");
  const toastMessage = document.getElementById("asfToastMessage");

  const grid = document.getElementById("asfRulesGrid");
  const noResults = document.getElementById("asfNoResults");
  const searchInput = document.getElementById("asfRulesSearch");
  const rules = Array.from(document.querySelectorAll(".asf-rule"));

  // Rule filter state (chips + filter drawer + search)
  let activeStatus = "all"; // all | active | paused | error
  const advFilters = { status: "all", source: "all" };
  let searchTerm = "";

  // Status badge presentation
  const STATUS = {
    active: { label: "Active", dot: "bg-emerald-500", badge: "bg-emerald-500/15 text-emerald-500" },
    paused: { label: "Paused", dot: "bg-amber-500", badge: "bg-amber-500/15 text-amber-500" },
    error: { label: "Error", dot: "bg-red-500", badge: "bg-red-500/15 text-red-500" },
  };

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
  document.querySelector(".asf-toast-close")?.addEventListener("click", hideToast);

  /* ======================================
   * 03. Drawers + export dropdown
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
  document.querySelectorAll(".asf-close-drawer").forEach((btn) => btn.addEventListener("click", closeDrawers));

  document.getElementById("asfNewRuleBtn")?.addEventListener("click", () => openCreateForm());
  document.getElementById("asfFilterBtn")?.addEventListener("click", () => openDrawer(filterDrawer));

  // Export dropdown
  const exportMenu = document.getElementById("asfExportMenu");
  function closeMenus() {
    exportMenu?.classList.add("hidden");
  }
  document.getElementById("asfExportBtn")?.addEventListener("click", (e) => {
    e.stopPropagation();
    exportMenu?.classList.toggle("hidden");
  });
  document.addEventListener("click", () => closeMenus());
  document.querySelectorAll(".asf-export-item").forEach((item) => {
    item.addEventListener("click", () => {
      closeMenus();
      showToast("Export Started", `Downloading rules as ${item.dataset.format.toUpperCase()}`);
    });
  });

  /* ======================================
   * 04. Tabs (rules / channels / logs / settings)
   * ====================================== */
  const panes = Array.from(document.querySelectorAll(".asf-pane"));
  document.querySelectorAll(".asf-tab").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll(".asf-tab").forEach((b) => {
        b.classList.remove("active");
        b.classList.add("text-muted");
      });
      btn.classList.add("active");
      btn.classList.remove("text-muted");
      const tab = btn.dataset.tab;
      panes.forEach((p) => p.classList.toggle("active", p.dataset.pane === tab));
      refreshIcons();
    });
  });

  /* ======================================
   * 05. Rules: status chips + filter drawer + search
   * ====================================== */
  function matchesFilters(el) {
    const status = el.dataset.status;
    const source = el.dataset.source || "";
    const name = (el.dataset.name || "").toLowerCase();
    const pairs = (el.dataset.pairs || "").toLowerCase();
    const srcLower = source.toLowerCase();

    const chipMatch = activeStatus === "all" || status === activeStatus;
    const advStatusMatch = advFilters.status === "all" || status === advFilters.status;
    const advSourceMatch = advFilters.source === "all" || source === advFilters.source;
    const searchMatch =
      !searchTerm || name.includes(searchTerm) || pairs.includes(searchTerm) || srcLower.includes(searchTerm);

    return chipMatch && advStatusMatch && advSourceMatch && searchMatch;
  }

  function applyFilters() {
    let visible = 0;
    rules.forEach((el) => {
      const show = matchesFilters(el);
      el.style.display = show ? "" : "none";
      if (show) visible += 1;
    });
    if (noResults) noResults.classList.toggle("hidden", visible !== 0);
  }

  // Status chips
  document.querySelectorAll(".asf-chip").forEach((chip) => {
    chip.addEventListener("click", () => {
      document.querySelectorAll(".asf-chip").forEach((c) => {
        c.classList.remove("active", "border-accent", "bg-accent/15", "text-accent");
        c.classList.add("border-border", "bg-panel", "text-muted");
      });
      chip.classList.add("active", "border-accent", "bg-accent/15", "text-accent");
      chip.classList.remove("border-border", "bg-panel", "text-muted");
      activeStatus = chip.dataset.filter;
      applyFilters();
    });
  });

  // Filter drawer option groups (single-select per group)
  document.querySelectorAll(".asf-filter-group").forEach((group) => {
    const key = group.dataset.group;
    group.querySelectorAll(".asf-filter-opt").forEach((opt) => {
      opt.addEventListener("click", () => {
        group.querySelectorAll(".asf-filter-opt").forEach((o) => {
          o.classList.remove("active", "border-accent", "bg-accent/15", "text-accent");
          o.classList.add("border-border", "bg-panel", "text-muted");
        });
        opt.classList.add("active", "border-accent", "bg-accent/15", "text-accent");
        opt.classList.remove("border-border", "bg-panel", "text-muted");
        if (key) advFilters[key] = opt.dataset.value;
      });
    });
  });
  document.querySelector(".asf-apply-filters")?.addEventListener("click", () => {
    applyFilters();
    closeDrawers();
    showToast("Filters Applied", "Results updated");
  });
  document.querySelector(".asf-reset-filters")?.addEventListener("click", () => {
    document.querySelectorAll(".asf-filter-group").forEach((group) => {
      group.querySelectorAll(".asf-filter-opt").forEach((o, i) => {
        const first = i === 0;
        o.classList.toggle("active", first);
        o.classList.toggle("border-accent", first);
        o.classList.toggle("bg-accent/15", first);
        o.classList.toggle("text-accent", first);
        o.classList.toggle("border-border", !first);
        o.classList.toggle("bg-panel", !first);
        o.classList.toggle("text-muted", !first);
      });
    });
    advFilters.status = "all";
    advFilters.source = "all";
    applyFilters();
  });

  // Search
  searchInput?.addEventListener("input", (e) => {
    searchTerm = e.target.value.toLowerCase();
    applyFilters();
  });

  /* ======================================
   * 06. Per-rule toggle / edit / delete / retry / selection
   * ====================================== */
  function setStatus(card, status) {
    card.dataset.status = status;
    const s = STATUS[status];
    const badge = card.querySelector(".asf-status-badge");
    if (badge && s) {
      badge.className = `asf-status-badge inline-flex items-center gap-1.5 px-2.5 py-1 rounded-lg text-xs font-semibold shrink-0 ${s.badge}`;
      const dot = badge.querySelector("span");
      if (dot) dot.className = `w-2 h-2 rounded-full ${s.dot}`;
      badge.childNodes.forEach((n) => {
        if (n.nodeType === Node.TEXT_NODE) n.textContent = s.label;
      });
    }
  }

  rules.forEach((card) => {
    // Selection checkbox
    const check = card.querySelector(".asf-check");
    function toggleCheck() {
      const on = check.classList.toggle("checked");
      check.setAttribute("aria-checked", on ? "true" : "false");
    }
    check?.addEventListener("click", (e) => {
      e.stopPropagation();
      toggleCheck();
    });
    check?.addEventListener("keydown", (e) => {
      if (e.key === " " || e.key === "Enter") {
        e.preventDefault();
        toggleCheck();
      }
    });

    // Enable/disable toggle (the shared .js-toggle-switch flips its own .active state)
    card.querySelector(".asf-toggle")?.addEventListener("click", () => {
      // Read state after common.js has toggled .active
      const on = card.querySelector(".asf-toggle")?.classList.contains("active");
      setStatus(card, on ? "active" : "paused");
      showToast("Rule Updated", `${card.dataset.name} ${on ? "activated" : "paused"}`);
    });

    // Edit
    card.querySelector(".asf-edit-btn")?.addEventListener("click", (e) => {
      e.stopPropagation();
      openEditForm(card);
    });

    // Delete
    card.querySelector(".asf-delete-btn")?.addEventListener("click", (e) => {
      e.stopPropagation();
      card.remove();
      showToast("Rule Deleted", `${card.dataset.name} removed`);
      applyFilters();
    });

    // Retry (error rules)
    card.querySelector(".asf-retry-btn")?.addEventListener("click", (e) => {
      e.stopPropagation();
      showToast("Retrying", `Reconnecting ${card.dataset.name}...`);
    });
  });

  /* ======================================
   * 07. Create / Edit rule form drawer
   * ====================================== */
  const formTitle = document.getElementById("asfFormTitle");
  const formSubmit = document.getElementById("asfFormSubmit");
  const formSubmitLabel = document.getElementById("asfFormSubmitLabel");
  const formName = document.getElementById("asfFormName");
  let editingCard = null;

  // Signal-type chips (multi-select toggle)
  document.querySelectorAll(".asf-type-btn").forEach((btn) => {
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

  // Confidence range
  const confidenceRange = document.getElementById("asfConfidenceRange");
  const confidenceValue = document.getElementById("asfConfidenceValue");
  confidenceRange?.addEventListener("input", () => {
    if (confidenceValue) confidenceValue.textContent = `${confidenceRange.value}%`;
  });

  function openCreateForm() {
    editingCard = null;
    if (formTitle) formTitle.textContent = "Create New Rule";
    if (formSubmitLabel) formSubmitLabel.textContent = "Save Rule";
    if (formName) formName.value = "";
    openDrawer(formDrawer);
  }
  function openEditForm(card) {
    editingCard = card;
    if (formTitle) formTitle.textContent = "Edit Rule";
    if (formSubmitLabel) formSubmitLabel.textContent = "Save Changes";
    if (formName) formName.value = card.dataset.name || "";
    openDrawer(formDrawer);
  }

  formSubmit?.addEventListener("click", () => {
    closeDrawers();
    if (editingCard) {
      const name = formName?.value?.trim();
      if (name) {
        editingCard.dataset.name = name;
        const title = editingCard.querySelector(".min-w-0 .font-semibold");
        if (title) title.textContent = name;
      }
      showToast("Rule Updated", `${editingCard.dataset.name} changes saved`);
    } else {
      showToast("Rule Created", "Your forwarding rule is now active");
    }
  });

  /* ======================================
   * 08. Activity logs: chips + search
   * ====================================== */
  const logs = Array.from(document.querySelectorAll(".asf-log"));
  const logsNoResults = document.getElementById("asfLogsNoResults");
  let logStatus = "all";
  let logSearch = "";

  function applyLogFilters() {
    let visible = 0;
    logs.forEach((el) => {
      const statusMatch = logStatus === "all" || el.dataset.status === logStatus;
      const searchMatch = !logSearch || (el.dataset.text || "").includes(logSearch);
      const show = statusMatch && searchMatch;
      el.style.display = show ? "" : "none";
      if (show) visible += 1;
    });
    if (logsNoResults) logsNoResults.classList.toggle("hidden", visible !== 0);
  }

  document.querySelectorAll(".asf-log-chip").forEach((chip) => {
    chip.addEventListener("click", () => {
      document.querySelectorAll(".asf-log-chip").forEach((c) => {
        c.classList.remove("active", "border-accent", "bg-accent/15", "text-accent");
        c.classList.add("border-border", "bg-panel", "text-muted");
      });
      chip.classList.add("active", "border-accent", "bg-accent/15", "text-accent");
      chip.classList.remove("border-border", "bg-panel", "text-muted");
      logStatus = chip.dataset.filter;
      applyLogFilters();
    });
  });
  document.getElementById("asfLogsSearch")?.addEventListener("input", (e) => {
    logSearch = e.target.value.toLowerCase();
    applyLogFilters();
  });
  document.getElementById("asfLogRange")?.addEventListener("change", (e) => {
    showToast("Logs Updated", `Showing logs for ${e.target.value}`);
  });
  document.querySelectorAll(".asf-log-retry").forEach((btn) => {
    btn.addEventListener("click", () => showToast("Retrying", "Attempting to redeliver signal..."));
  });
  document.getElementById("asfLoadMore")?.addEventListener("click", () => {
    showToast("Logs", "Loading more activity logs...");
  });

  /* ======================================
   * 09. Channels + settings actions + keyboard shortcuts
   * ====================================== */
  document.querySelectorAll(".asf-channel-add").forEach((btn) => {
    btn.addEventListener("click", () => showToast("Coming Soon", "Channel management will be available soon"));
  });
  document.getElementById("asfSaveSettings")?.addEventListener("click", () => {
    showToast("Settings Saved", "Your forwarding settings have been updated");
  });
  document.getElementById("asfTplPreview")?.addEventListener("click", () => {
    showToast("Preview", "Rendering message template preview...");
  });
  document.getElementById("asfTplReset")?.addEventListener("click", () => {
    showToast("Template Reset", "Message template restored to default");
  });

  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      closeDrawers();
      closeMenus();
    }
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
      e.preventDefault();
      searchInput?.focus();
    }
  });

  // Initial render
  applyFilters();
  applyLogFilters();
  refreshIcons();
});
