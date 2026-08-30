/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: custom-alerts.html
 * description: SignalAIX - Custom Alerts Watchlist Page Controller
 *              Self-contained; mirrors the reference mockup's functionality
 *              (type tabs, status filter chips, search, sort, bulk menu, export
 *               menu, per-alert enable/disable toggle + edit + delete, create-
 *               alert drawer / edit reuses it / view-alert drawer / filter drawer,
 *               toast, and a real Chart.js 7-day activity bar chart).
 *              All markup lives in custom-alerts.html; this file only modifies the
 *              DOM (text/values/classes/visibility, reorder existing nodes) and
 *              renders Chart.js — it never injects HTML strings.
 * author: SignalAIX

    -------------------------------------------------
     01. Init & DOM refs (guarded by #custom-alerts)
     -------------------------------------------------
     02. Toast
     -------------------------------------------------
     03. Drawers (form / view / filter) + dropdown menus
     -------------------------------------------------
     04. Tabs + status chips + filter drawer + search (card filtering)
     -------------------------------------------------
     05. Sort (reorder existing nodes) + bulk + export
     -------------------------------------------------
     06. Per-alert toggle / edit / delete / selection / view
     -------------------------------------------------
     07. Create / Edit form drawer
     -------------------------------------------------
     08. Activity chart + theme re-color + keyboard shortcuts
     -------------------------------------------------
    ================================================== */

document.addEventListener("DOMContentLoaded", () => {
  /* ======================================
   * 01. Init & DOM refs
   * ====================================== */
  const page = document.getElementById("custom-alerts");
  if (!page) return; // Guard: only run on the Custom Alerts page

  const refreshIcons = () => window.lucide?.createIcons?.();
  const hasChart = () => typeof Chart !== "undefined";

  const overlay = document.getElementById("caDrawerOverlay");
  const formDrawer = document.getElementById("caFormDrawer");
  const viewDrawer = document.getElementById("caViewDrawer");
  const filterDrawer = document.getElementById("caFilterDrawer");
  const allDrawers = [formDrawer, viewDrawer, filterDrawer];

  const toast = document.getElementById("caToast");
  const toastTitle = document.getElementById("caToastTitle");
  const toastMessage = document.getElementById("caToastMessage");

  const grid = document.getElementById("caAlertsGrid");
  const noResults = document.getElementById("caNoResults");
  const searchInput = document.getElementById("caSearch");
  const alerts = Array.from(document.querySelectorAll(".ca-alert"));

  // Filter state shared by tabs + chips + filter drawer + search
  let activeTab = "all"; // all | price | signal | volume | triggered
  let activeStatus = "all"; // all | active | paused | triggered
  const advFilters = { type: "all", status: "all" };
  let searchTerm = "";

  // Status badge presentation (used to repaint a card's status badge + view drawer)
  const STATUS = {
    active: { label: "Active", dot: "bg-emerald-500", badge: "bg-emerald-500/15 text-emerald-500" },
    paused: { label: "Paused", dot: "bg-amber-500", badge: "bg-amber-500/15 text-amber-500" },
    triggered: { label: "Triggered", dot: "bg-red-500", badge: "bg-red-500/15 text-red-500" },
    expired: { label: "Expired", dot: "bg-slate-500", badge: "bg-slate-500/15 text-muted" },
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
  document.querySelector(".ca-toast-close")?.addEventListener("click", hideToast);

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
  document.querySelectorAll(".ca-close-drawer").forEach((btn) => btn.addEventListener("click", closeDrawers));

  document.getElementById("caCreateBtn")?.addEventListener("click", () => openCreateForm());
  document.getElementById("caFilterBtn")?.addEventListener("click", () => openDrawer(filterDrawer));

  // Dropdown menus (export, bulk) — toggle the matching menu, close others
  const exportMenu = document.getElementById("caExportMenu");
  const bulkMenu = document.getElementById("caBulkMenu");
  function closeMenus(except) {
    [exportMenu, bulkMenu].forEach((m) => {
      if (m && m !== except) m.classList.add("hidden");
    });
  }
  document.getElementById("caExportBtn")?.addEventListener("click", (e) => {
    e.stopPropagation();
    closeMenus(exportMenu);
    exportMenu?.classList.toggle("hidden");
  });
  document.getElementById("caBulkBtn")?.addEventListener("click", (e) => {
    e.stopPropagation();
    closeMenus(bulkMenu);
    bulkMenu?.classList.toggle("hidden");
  });
  document.addEventListener("click", () => closeMenus());

  /* ======================================
   * 04. Tabs + status chips + filter drawer + search
   * ====================================== */
  function matchesFilters(el) {
    const type = el.dataset.type;
    const status = el.dataset.status;
    const pair = (el.dataset.pair || "").toLowerCase();
    const condition = (el.dataset.condition || "").toLowerCase();

    // Tabs: all | price | signal | volume | triggered
    const tabMatch =
      activeTab === "all" ||
      (activeTab === "triggered" ? status === "triggered" : type === activeTab);

    // Status chips
    const chipMatch = activeStatus === "all" || status === activeStatus;

    // Filter drawer (type + status, AND-combined with the above)
    const advTypeMatch = advFilters.type === "all" || type === advFilters.type;
    const advStatusMatch = advFilters.status === "all" || status === advFilters.status;

    const searchMatch = !searchTerm || pair.includes(searchTerm) || condition.includes(searchTerm);

    return tabMatch && chipMatch && advTypeMatch && advStatusMatch && searchMatch;
  }

  function applyFilters() {
    let visible = 0;
    alerts.forEach((el) => {
      const show = matchesFilters(el);
      el.style.display = show ? "" : "none";
      if (show) visible += 1;
    });
    if (noResults) noResults.classList.toggle("hidden", visible !== 0);
  }

  // Type tabs
  document.querySelectorAll(".ca-tab").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll(".ca-tab").forEach((b) => {
        b.classList.remove("active");
        b.classList.add("text-muted");
      });
      btn.classList.add("active");
      btn.classList.remove("text-muted");
      activeTab = btn.dataset.tab;
      applyFilters();
    });
  });

  // Status chips
  document.querySelectorAll(".ca-chip").forEach((chip) => {
    chip.addEventListener("click", () => {
      document.querySelectorAll(".ca-chip").forEach((c) => {
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
  document.querySelectorAll(".ca-filter-group").forEach((group) => {
    const key = group.dataset.group;
    group.querySelectorAll(".ca-filter-opt").forEach((opt) => {
      opt.addEventListener("click", () => {
        group.querySelectorAll(".ca-filter-opt").forEach((o) => {
          o.classList.remove("active", "border-accent", "bg-accent/15", "text-accent");
          o.classList.add("border-border", "bg-panel", "text-muted");
        });
        opt.classList.add("active", "border-accent", "bg-accent/15", "text-accent");
        opt.classList.remove("border-border", "bg-panel", "text-muted");
        if (key) advFilters[key] = opt.dataset.value;
      });
    });
  });
  document.querySelector(".ca-apply-filters")?.addEventListener("click", () => {
    applyFilters();
    closeDrawers();
    showToast("Filters Applied", "Results updated");
  });
  document.querySelector(".ca-reset-filters")?.addEventListener("click", () => {
    document.querySelectorAll(".ca-filter-group").forEach((group) => {
      group.querySelectorAll(".ca-filter-opt").forEach((o, i) => {
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
    advFilters.type = "all";
    advFilters.status = "all";
    applyFilters();
  });

  // Search
  searchInput?.addEventListener("input", (e) => {
    searchTerm = e.target.value.toLowerCase();
    applyFilters();
  });

  /* ======================================
   * 05. Sort + bulk + export
   * ====================================== */
  const STATUS_RANK = { active: 0, triggered: 1, paused: 2, expired: 3 };
  function reorder(cmp) {
    if (!grid) return;
    alerts.slice().sort(cmp).forEach((node) => grid.appendChild(node));
  }
  document.getElementById("caSort")?.addEventListener("change", (e) => {
    const v = e.target.value;
    if (v === "pair") {
      reorder((a, b) => (a.dataset.pair || "").localeCompare(b.dataset.pair || ""));
    } else if (v === "status") {
      reorder((a, b) => (STATUS_RANK[a.dataset.status] ?? 9) - (STATUS_RANK[b.dataset.status] ?? 9));
    } else {
      reorder((a, b) => Number(a.dataset.id) - Number(b.dataset.id));
    }
  });

  document.querySelectorAll(".ca-bulk-item").forEach((item) => {
    item.addEventListener("click", () => {
      bulkMenu?.classList.add("hidden");
      const action = item.dataset.action;
      const selected = alerts.filter((el) => el.querySelector(".ca-check.checked"));
      if (action === "delete") {
        selected.forEach((el) => el.remove());
        showToast("Bulk Action", `${selected.length || "No"} alert(s) deleted`);
      } else {
        showToast("Bulk Action", `${action} applied to ${selected.length} alert(s)`);
      }
    });
  });

  document.querySelectorAll(".ca-export-item").forEach((item) => {
    item.addEventListener("click", () => {
      exportMenu?.classList.add("hidden");
      showToast("Export Started", `Downloading alerts as ${item.dataset.format.toUpperCase()}`);
    });
  });

  /* ======================================
   * 06. Per-alert toggle / edit / delete / selection / view
   * ====================================== */
  function setStatus(card, status) {
    card.dataset.status = status;
    const s = STATUS[status];
    const badge = card.querySelector(".ca-status-badge");
    if (badge && s) {
      badge.className = `ca-status-badge inline-flex items-center gap-1.5 px-2.5 py-1 rounded-lg text-xs font-semibold shrink-0 ${s.badge}`;
      const dot = badge.querySelector("span");
      if (dot) dot.className = `w-2 h-2 rounded-full ${s.dot}`;
      // Replace text node label (last child after the dot)
      badge.childNodes.forEach((n) => {
        if (n.nodeType === Node.TEXT_NODE) n.textContent = s.label;
      });
    }
  }

  function toggleStatus(card) {
    const next = card.dataset.status === "active" ? "paused" : "active";
    setStatus(card, next);
    // Swap the toggle button icon
    const btnIcon = card.querySelector(".ca-toggle-btn i");
    if (btnIcon) btnIcon.setAttribute("data-lucide", next === "active" ? "pause" : "play");
    refreshIcons();
    return next;
  }

  alerts.forEach((card) => {
    // Selection checkbox
    const check = card.querySelector(".ca-check");
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

    // Enable/disable toggle
    card.querySelector(".ca-toggle-btn")?.addEventListener("click", (e) => {
      e.stopPropagation();
      const next = toggleStatus(card);
      showToast("Status Updated", `${card.dataset.pair} alert ${next}`);
    });

    // Edit
    card.querySelector(".ca-edit-btn")?.addEventListener("click", (e) => {
      e.stopPropagation();
      openEditForm(card);
    });

    // Delete
    card.querySelector(".ca-delete-btn")?.addEventListener("click", (e) => {
      e.stopPropagation();
      card.remove();
      showToast("Alert Deleted", `${card.dataset.pair} alert removed`);
      applyFilters();
    });

    // Click card body → view
    card.addEventListener("click", () => openView(card));
  });

  /* ----- View drawer ----- */
  let viewCard = null;
  function openView(card) {
    viewCard = card;
    const status = card.dataset.status;
    const s = STATUS[status];
    document.getElementById("caViewPair").textContent = card.dataset.pair;
    document.getElementById("caViewCondition").textContent = card.dataset.condition;
    document.getElementById("caViewTarget").textContent = card.dataset.target || "—";
    document.getElementById("caViewCurrent").textContent = card.dataset.current || "—";
    document.getElementById("caViewTriggered").textContent = card.dataset.triggered || "0";
    document.getElementById("caViewChannels").textContent = card.dataset.channels || "0";

    const statusEl = document.getElementById("caViewStatus");
    const statusDot = document.getElementById("caViewStatusDot");
    const statusLabel = document.getElementById("caViewStatusLabel");
    if (statusEl && s) {
      statusEl.className = `inline-flex items-center gap-1.5 px-2.5 py-1 rounded-lg text-xs font-semibold ${s.badge}`;
      if (statusDot) statusDot.className = `w-2 h-2 rounded-full ${s.dot}`;
      if (statusLabel) statusLabel.textContent = s.label;
    }

    const toggleLabel = document.getElementById("caViewToggleLabel");
    const toggleIcon = document.querySelector("#caViewToggleBtn i");
    if (toggleLabel) toggleLabel.textContent = status === "active" ? "Pause" : "Activate";
    if (toggleIcon) toggleIcon.setAttribute("data-lucide", status === "active" ? "pause" : "play");

    openDrawer(viewDrawer);
  }
  document.getElementById("caViewEditBtn")?.addEventListener("click", () => {
    if (viewCard) openEditForm(viewCard);
  });
  document.getElementById("caViewToggleBtn")?.addEventListener("click", () => {
    if (!viewCard) return;
    const next = toggleStatus(viewCard);
    closeDrawers();
    showToast("Status Updated", `${viewCard.dataset.pair} alert ${next}`);
  });

  /* ======================================
   * 07. Create / Edit form drawer
   * ====================================== */
  const formTitle = document.getElementById("caFormTitle");
  const formSubmit = document.getElementById("caFormSubmit");
  const formSubmitLabel = document.getElementById("caFormSubmitLabel");
  let editingCard = null;

  // Pair selector (single-select)
  document.querySelectorAll(".ca-pair-btn").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll(".ca-pair-btn").forEach((b) => {
        b.classList.remove("active", "border-accent", "bg-accent/15", "text-accent");
        b.classList.add("border-border", "text-muted");
      });
      btn.classList.add("active", "border-accent", "bg-accent/15", "text-accent");
      btn.classList.remove("border-border", "text-muted");
    });
  });
  // Frequency selector (single-select)
  document.querySelectorAll(".ca-freq-btn").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll(".ca-freq-btn").forEach((b) => {
        b.classList.remove("active", "border-accent", "bg-accent/15", "text-accent");
        b.classList.add("border-border", "text-muted");
      });
      btn.classList.add("active", "border-accent", "bg-accent/15", "text-accent");
      btn.classList.remove("border-border", "text-muted");
    });
  });

  function openCreateForm() {
    editingCard = null;
    if (formTitle) formTitle.textContent = "Create New Alert";
    if (formSubmitLabel) formSubmitLabel.textContent = "Create Alert";
    openDrawer(formDrawer);
  }
  function openEditForm(card) {
    editingCard = card;
    if (formTitle) formTitle.textContent = "Edit Alert";
    if (formSubmitLabel) formSubmitLabel.textContent = "Save Changes";
    // Pre-select the pair button matching this card if present
    document.querySelectorAll(".ca-pair-btn").forEach((b) => {
      const match = b.dataset.pair === card.dataset.pair;
      b.classList.toggle("active", match);
      b.classList.toggle("border-accent", match);
      b.classList.toggle("bg-accent/15", match);
      b.classList.toggle("text-accent", match);
      b.classList.toggle("border-border", !match);
      b.classList.toggle("text-muted", !match);
    });
    openDrawer(formDrawer);
  }

  formSubmit?.addEventListener("click", () => {
    closeDrawers();
    if (editingCard) {
      showToast("Alert Updated", `${editingCard.dataset.pair} changes saved`);
    } else {
      showToast("Alert Created", "Your alert is now active");
    }
  });

  document.querySelector(".ca-view-all")?.addEventListener("click", () => {
    showToast("Recent Triggers", "Showing all triggered alerts");
  });

  /* ======================================
   * 08. Activity chart + theme re-color + keyboard shortcuts
   * ====================================== */
  let activityChart;
  function tickColor() {
    return document.documentElement.classList.contains("dark") ? "#94A3B8" : "#64748B";
  }
  function gridColor() {
    return document.documentElement.classList.contains("dark") ? "rgba(255,255,255,0.05)" : "rgba(0,0,0,0.05)";
  }
  function initActivityChart() {
    const canvas = document.getElementById("caActivityChart");
    if (!canvas || !hasChart()) return;
    if (activityChart) activityChart.destroy();
    activityChart = new Chart(canvas.getContext("2d"), {
      type: "bar",
      data: {
        labels: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
        datasets: [
          { label: "Triggered", data: [5, 8, 4, 12, 7, 3, 8], backgroundColor: "#10b981", borderRadius: 6 },
          { label: "Created", data: [2, 3, 1, 4, 2, 1, 2], backgroundColor: "#0ea5e9", borderRadius: 6 },
        ],
      },
      options: {
        responsive: true,
        maintainAspectRatio: false,
        plugins: { legend: { display: false } },
        scales: {
          y: { beginAtZero: true, grid: { color: gridColor() }, ticks: { color: tickColor() } },
          x: { grid: { display: false }, ticks: { color: tickColor() } },
        },
      },
    });
  }

  document.getElementById("themeToggle")?.addEventListener("click", () => {
    setTimeout(() => {
      if (!activityChart) return;
      activityChart.options.scales.y.grid.color = gridColor();
      activityChart.options.scales.y.ticks.color = tickColor();
      activityChart.options.scales.x.ticks.color = tickColor();
      activityChart.update();
    }, 50);
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
  initActivityChart();
  applyFilters();
  refreshIcons();
});
