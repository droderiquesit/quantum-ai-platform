/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: api-integrations.html
 * description: SignalAIX - API Integrations (Automation) Page Controller
 *              Self-contained; mirrors the reference mockup's functionality
 *              (4 tabs: API Keys / Integrations / Documentation / Usage & Limits;
 *               API key list with show/hide masked keys, copy-to-clipboard,
 *               edit/regenerate/revoke/renew/delete, status filter chips + search +
 *               select-all; integration cards connect/disconnect/manage + filter
 *               chips; generate/edit-key drawer, generated-key result drawer,
 *               connect/manage-integration drawer; export menu; copy code snippet;
 *               toast; and a real Chart.js 7-day usage bar chart, lazy-init on the
 *               Usage tab + theme re-color).
 *              All markup lives in api-integrations.html; this file only modifies the
 *              DOM (text/values/classes/visibility, swap data-lucide eye<->eye-off)
 *              and renders Chart.js — it never injects HTML strings.
 * author: SignalAIX

    -------------------------------------------------
     01. Init & DOM refs (guarded by #api-integrations)
     -------------------------------------------------
     02. Toast
     -------------------------------------------------
     03. Drawers + dropdown menu
     -------------------------------------------------
     04. Tabs (lazy chart on Usage)
     -------------------------------------------------
     05. API Keys: filter chips + search + select-all
     -------------------------------------------------
     06. API Keys: show/hide + copy + row actions
     -------------------------------------------------
     07. Integrations: filter chips + connect/manage/disconnect
     -------------------------------------------------
     08. Generate / Edit key form + generated-key drawer
     -------------------------------------------------
     09. Documentation copy + Usage chart + theme re-color + keyboard
     -------------------------------------------------
    ================================================== */

document.addEventListener("DOMContentLoaded", () => {
  /* ======================================
   * 01. Init & DOM refs
   * ====================================== */
  const page = document.getElementById("api-integrations");
  if (!page) return; // Guard: only run on the API Integrations page

  const refreshIcons = () => window.lucide?.createIcons?.();
  const hasChart = () => typeof Chart !== "undefined";

  const overlay = document.getElementById("apiDrawerOverlay");
  const keyDrawer = document.getElementById("apiKeyDrawer");
  const genDrawer = document.getElementById("apiGeneratedDrawer");
  const intDrawer = document.getElementById("apiIntDrawer");
  const allDrawers = [keyDrawer, genDrawer, intDrawer];

  const toast = document.getElementById("apiToast");
  const toastTitle = document.getElementById("apiToastTitle");
  const toastMessage = document.getElementById("apiToastMessage");

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
  document.querySelector(".api-toast-close")?.addEventListener("click", hideToast);

  function copyText(text, msg) {
    navigator.clipboard?.writeText(text).then(
      () => showToast("Copied", msg),
      () => showToast("Copied", msg),
    );
  }

  /* ======================================
   * 03. Drawers + dropdown menu
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
  document.querySelectorAll(".api-close-drawer").forEach((btn) => btn.addEventListener("click", closeDrawers));

  // Export dropdown menu
  const exportMenu = document.getElementById("apiExportMenu");
  document.getElementById("apiExportBtn")?.addEventListener("click", (e) => {
    e.stopPropagation();
    exportMenu?.classList.toggle("hidden");
  });
  document.addEventListener("click", () => exportMenu?.classList.add("hidden"));
  document.querySelectorAll(".api-export-item").forEach((item) => {
    item.addEventListener("click", () => {
      exportMenu?.classList.add("hidden");
      showToast("Export Started", `Exporting data as ${(item.dataset.format || "").toUpperCase()}...`);
    });
  });

  /* ======================================
   * 04. Tabs (lazy chart on Usage)
   * ====================================== */
  const tabs = Array.from(document.querySelectorAll(".api-tab"));
  const panes = Array.from(document.querySelectorAll(".api-pane"));
  tabs.forEach((tab) => {
    tab.addEventListener("click", () => {
      const name = tab.dataset.tab;
      tabs.forEach((t) => {
        t.classList.toggle("active", t === tab);
        t.classList.toggle("text-muted", t !== tab);
      });
      panes.forEach((p) => p.classList.toggle("active", p.dataset.pane === name));
      if (name === "usage") {
        requestAnimationFrame(() => initUsageChart());
      }
      refreshIcons();
    });
  });

  /* ======================================
   * 05. API Keys: filter chips + search + select-all
   * ====================================== */
  const keyRows = Array.from(document.querySelectorAll(".api-key-row"));
  const keysNoResults = document.getElementById("apiKeysNoResults");
  let keyFilter = "all"; // all | active | expired | revoked
  let keySearch = "";

  function applyKeyFilters() {
    let visible = 0;
    keyRows.forEach((row) => {
      const statusMatch = keyFilter === "all" || row.dataset.status === keyFilter;
      const text = (row.dataset.name || "").toLowerCase();
      const searchMatch = !keySearch || text.includes(keySearch);
      const show = statusMatch && searchMatch;
      row.style.display = show ? "" : "none";
      if (show) visible += 1;
    });
    keysNoResults?.classList.toggle("hidden", visible !== 0);
  }

  document.querySelectorAll(".api-key-filter").forEach((chip) => {
    chip.addEventListener("click", () => {
      document.querySelectorAll(".api-key-filter").forEach((c) => {
        c.classList.remove("active", "border-accent", "bg-accent/15", "text-accent");
        c.classList.add("border-border", "bg-panel", "text-muted");
      });
      chip.classList.add("active", "border-accent", "bg-accent/15", "text-accent");
      chip.classList.remove("border-border", "bg-panel", "text-muted");
      keyFilter = chip.dataset.filter;
      applyKeyFilters();
    });
  });

  const keysSearchInput = document.getElementById("apiKeysSearch");
  keysSearchInput?.addEventListener("input", (e) => {
    keySearch = e.target.value.toLowerCase();
    applyKeyFilters();
  });

  const selectAll = document.getElementById("apiSelectAll");
  const rowChecks = Array.from(document.querySelectorAll(".api-row-check"));
  selectAll?.addEventListener("change", () => {
    rowChecks.forEach((cb) => {
      if (cb.closest("tr")?.style.display !== "none") cb.checked = selectAll.checked;
    });
  });

  /* ======================================
   * 06. API Keys: show/hide + copy + row actions
   * ====================================== */
  // Show / hide masked key (DOM-only: swap text + eye<->eye-off)
  document.querySelectorAll(".api-key-eye").forEach((btn) => {
    btn.addEventListener("click", () => {
      const value = btn.closest("td")?.querySelector(".api-key-value");
      const icon = btn.querySelector("i");
      if (!value || !icon) return;
      const revealed = value.dataset.revealed === "true";
      if (revealed) {
        value.textContent = value.dataset.mask;
        value.dataset.revealed = "false";
        icon.setAttribute("data-lucide", "eye");
        btn.title = "Show";
      } else {
        value.textContent = value.dataset.full;
        value.dataset.revealed = "true";
        icon.setAttribute("data-lucide", "eye-off");
        btn.title = "Hide";
      }
      refreshIcons();
    });
  });

  // Copy key
  document.querySelectorAll(".api-key-copy").forEach((btn) => {
    btn.addEventListener("click", () => {
      const value = btn.closest("td")?.querySelector(".api-key-value");
      if (!value) return;
      copyText(value.dataset.full || value.textContent, "API key copied to clipboard");
    });
  });

  // Row actions
  document.querySelectorAll(".api-key-edit").forEach((btn) =>
    btn.addEventListener("click", () => openKeyForm(true)),
  );
  document.querySelectorAll(".api-key-regen").forEach((btn) =>
    btn.addEventListener("click", () => showToast("Key Regenerated", "Your API key has been regenerated")),
  );
  document.querySelectorAll(".api-key-revoke").forEach((btn) =>
    btn.addEventListener("click", () => {
      const row = btn.closest(".api-key-row");
      if (row) {
        row.dataset.status = "revoked";
        const badge = row.querySelector(".api-status-badge");
        const dot = row.querySelector(".api-status-dot");
        const label = row.querySelector(".api-status-label");
        if (badge)
          badge.className =
            "api-status-badge inline-flex items-center gap-1.5 px-2.5 py-1 rounded-lg text-xs font-semibold bg-red-500/15 text-red-500";
        if (dot) dot.className = "api-status-dot w-2 h-2 rounded-full bg-red-500";
        if (label) label.textContent = "Revoked";
        row.classList.add("opacity-60");
      }
      showToast("Key Revoked", "The API key has been revoked");
      applyKeyFilters();
    }),
  );
  document.querySelectorAll(".api-key-renew").forEach((btn) =>
    btn.addEventListener("click", () => showToast("Key Renewed", "The API key has been renewed")),
  );
  document.querySelectorAll(".api-key-delete").forEach((btn) =>
    btn.addEventListener("click", () => {
      btn.closest(".api-key-row")?.remove();
      showToast("Key Deleted", "The API key has been deleted");
      applyKeyFilters();
    }),
  );

  /* ======================================
   * 07. Integrations: filter chips + connect/manage/disconnect
   * ====================================== */
  const intCards = Array.from(document.querySelectorAll(".api-int-card"));

  function applyIntFilter(filter) {
    intCards.forEach((card) => {
      let show = true;
      if (filter === "connected" || filter === "available") {
        show = card.dataset.state === filter;
      } else if (filter !== "all") {
        show = card.dataset.category === filter;
      }
      card.style.display = show ? "" : "none";
    });
  }

  document.querySelectorAll(".api-int-filter").forEach((chip) => {
    chip.addEventListener("click", () => {
      document.querySelectorAll(".api-int-filter").forEach((c) => {
        c.classList.remove("active", "border-accent", "bg-accent/15", "text-accent");
        c.classList.add("border-border", "bg-panel", "text-muted");
      });
      chip.classList.add("active", "border-accent", "bg-accent/15", "text-accent");
      chip.classList.remove("border-border", "bg-panel", "text-muted");
      applyIntFilter(chip.dataset.filter);
    });
  });

  function intName(card) {
    return card?.querySelector("h4")?.textContent.trim() || "service";
  }

  document.querySelectorAll(".api-int-connect").forEach((btn) =>
    btn.addEventListener("click", () => {
      const name = intName(btn.closest(".api-int-card"));
      openIntDrawer("connect", name);
    }),
  );
  document.querySelectorAll(".api-int-manage").forEach((btn) =>
    btn.addEventListener("click", () => {
      const name = intName(btn.closest(".api-int-card"));
      openIntDrawer("manage", name);
    }),
  );
  document.querySelector(".api-int-custom")?.addEventListener("click", () =>
    openIntDrawer("connect", "Custom Webhook"),
  );
  document.querySelectorAll(".api-int-disconnect").forEach((btn) =>
    btn.addEventListener("click", () => {
      const card = btn.closest(".api-int-card");
      showToast("Disconnected", `${intName(card)} has been disconnected`);
    }),
  );

  // Connect / manage integration drawer
  const intTitle = document.getElementById("apiIntTitle");
  const intSub = document.getElementById("apiIntSub");
  const intSubmitLabel = document.getElementById("apiIntSubmitLabel");
  function openIntDrawer(mode, name) {
    if (intTitle) intTitle.textContent = mode === "manage" ? `Manage ${name}` : `Connect ${name}`;
    if (intSub)
      intSub.textContent =
        mode === "manage" ? "Update credentials and permissions" : "Authorize and configure this service";
    if (intSubmitLabel) intSubmitLabel.textContent = mode === "manage" ? "Save Changes" : "Connect";
    openDrawer(intDrawer);
  }
  document.getElementById("apiIntSubmit")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Integration", "Connection settings saved successfully");
  });

  // Secret show/hide in integration drawer
  document.getElementById("apiIntSecretEye")?.addEventListener("click", function () {
    const input = document.getElementById("apiIntSecret");
    const icon = this.querySelector("i");
    if (!input || !icon) return;
    if (input.type === "password") {
      input.type = "text";
      icon.setAttribute("data-lucide", "eye-off");
      this.title = "Hide";
    } else {
      input.type = "password";
      icon.setAttribute("data-lucide", "eye");
      this.title = "Show";
    }
    refreshIcons();
  });

  /* ======================================
   * 08. Generate / Edit key form + generated-key drawer
   * ====================================== */
  const keyDrawerTitle = document.getElementById("apiKeyDrawerTitle");
  const keyDrawerSub = document.getElementById("apiKeyDrawerSub");
  const keySubmitLabel = document.getElementById("apiKeySubmitLabel");

  function openKeyForm(isEdit) {
    if (keyDrawerTitle) keyDrawerTitle.textContent = isEdit ? "Edit API Key" : "Generate New API Key";
    if (keyDrawerSub)
      keyDrawerSub.textContent = isEdit
        ? "Update this key's settings"
        : "Create a new API key for your integrations";
    if (keySubmitLabel) keySubmitLabel.textContent = isEdit ? "Save Changes" : "Generate Key";
    openDrawer(keyDrawer);
  }

  document.getElementById("apiNewKeyBtn")?.addEventListener("click", () => openKeyForm(false));

  document.getElementById("apiKeySubmit")?.addEventListener("click", () => {
    const isEdit = keySubmitLabel?.textContent === "Save Changes";
    if (isEdit) {
      closeDrawers();
      showToast("Key Updated", "Your API key settings have been saved");
      return;
    }
    // generate flow -> show generated-key result drawer
    openDrawer(genDrawer);
  });

  document.getElementById("apiGenCopyBtn")?.addEventListener("click", () => {
    const v = document.getElementById("apiGeneratedValue")?.textContent || "";
    copyText(v, "API key copied to clipboard");
  });
  document.getElementById("apiGenDone")?.addEventListener("click", () => {
    closeDrawers();
    showToast("API Key Created", "Your new API key has been generated successfully");
  });

  /* ======================================
   * 09. Documentation copy + Usage chart + theme re-color + keyboard
   * ====================================== */
  document.getElementById("apiCopyCodeBtn")?.addEventListener("click", () => {
    const code = document.getElementById("apiCodeSnippet")?.innerText || "";
    copyText(code, "Code snippet copied to clipboard");
  });

  document.getElementById("apiUpgradeBtn")?.addEventListener("click", () =>
    showToast("Upgrade Plan", "Opening plan upgrade options..."),
  );

  let usageChart;
  function tickColor() {
    return document.documentElement.classList.contains("dark") ? "#94A3B8" : "#64748B";
  }
  function gridColor() {
    return document.documentElement.classList.contains("dark") ? "rgba(255,255,255,0.05)" : "rgba(0,0,0,0.05)";
  }
  function initUsageChart() {
    const canvas = document.getElementById("apiUsageChart");
    if (!canvas || !hasChart() || usageChart) return;
    usageChart = new Chart(canvas.getContext("2d"), {
      type: "bar",
      data: {
        labels: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
        datasets: [
          {
            label: "API Requests",
            data: [18420, 21340, 19850, 24120, 22680, 15230, 12847],
            backgroundColor: "#10b981",
            borderRadius: 6,
            borderSkipped: false,
          },
        ],
      },
      options: {
        responsive: true,
        maintainAspectRatio: false,
        plugins: {
          legend: { display: false },
          tooltip: {
            callbacks: { label: (c) => `${c.parsed.y.toLocaleString()} requests` },
          },
        },
        scales: {
          y: {
            beginAtZero: true,
            grid: { color: gridColor() },
            ticks: { color: tickColor(), callback: (v) => v / 1000 + "K" },
          },
          x: { grid: { display: false }, ticks: { color: tickColor() } },
        },
      },
    });
  }

  document.getElementById("themeToggle")?.addEventListener("click", () => {
    setTimeout(() => {
      if (!usageChart) return;
      usageChart.options.scales.y.grid.color = gridColor();
      usageChart.options.scales.y.ticks.color = tickColor();
      usageChart.options.scales.x.ticks.color = tickColor();
      usageChart.update();
    }, 50);
  });

  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      closeDrawers();
      exportMenu?.classList.add("hidden");
    }
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
      e.preventDefault();
      keysSearchInput?.focus();
    }
  });

  // Initial render
  applyKeyFilters();
  refreshIcons();
});
