/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: subscription-billing.html
 * description: SignalAIX - Subscription & Billing (Account) Page Controller
 *              Self-contained; mirrors the reference mockup's functionality
 *              (current-plan hero with Change Plan / Cancel; 4 tabs:
 *               Plans & Pricing / Payment Methods / Billing History / Usage
 *               Analytics; monthly/annual price swap via textContent; full
 *               feature-comparison show/hide; payment methods add/edit/remove/
 *               set-as-primary; billing address edit; auto-renew toggle;
 *               invoices table search + status + date filter, view/download/
 *               retry; CSV/PDF export; usage-period chips; and two real
 *               Chart.js charts — signal-usage line + API-distribution doughnut,
 *               lazy-init on the Usage tab + theme re-color).
 *              Add/Edit-card, Edit-billing, Upgrade, Change-plan, Cancel-sub
 *              are page-scoped drawers (mockup modals converted to drawers).
 *              All markup lives in subscription-billing.html; this file only
 *              modifies the DOM (text/values/classes/visibility, swap
 *              data-lucide) and renders Chart.js — never injects HTML strings.
 * author: SignalAIX

    -------------------------------------------------
     01. Init & DOM refs (guarded by #subscription-billing)
     -------------------------------------------------
     02. Toast
     -------------------------------------------------
     03. Drawers
     -------------------------------------------------
     04. Tabs (lazy charts on Usage)
     -------------------------------------------------
     05. Plans: monthly/annual toggle + feature table + plan actions
     -------------------------------------------------
     06. Payment methods + billing address + auto-renew
     -------------------------------------------------
     07. Billing history: search/status/date filter + actions + export
     -------------------------------------------------
     08. Usage charts + period chips + theme re-color + keyboard
     -------------------------------------------------
    ================================================== */

document.addEventListener("DOMContentLoaded", () => {
  /* ======================================
   * 01. Init & DOM refs
   * ====================================== */
  const page = document.getElementById("subscription-billing");
  if (!page) return; // Guard: only run on the Subscription & Billing page

  const refreshIcons = () => window.lucide?.createIcons?.();
  const hasChart = () => typeof Chart !== "undefined";

  const overlay = document.getElementById("sbDrawerOverlay");
  const drawers = {
    addCard: document.getElementById("sbAddCardDrawer"),
    editCard: document.getElementById("sbEditCardDrawer"),
    editBilling: document.getElementById("sbEditBillingDrawer"),
    upgrade: document.getElementById("sbUpgradeDrawer"),
    changePlan: document.getElementById("sbChangePlanDrawer"),
    cancel: document.getElementById("sbCancelDrawer"),
  };
  const allDrawers = Object.values(drawers);

  const toast = document.getElementById("sbToast");
  const toastTitle = document.getElementById("sbToastTitle");
  const toastMessage = document.getElementById("sbToastMessage");
  const toastIcon = document.getElementById("sbToastIcon");

  /* ======================================
   * 02. Toast
   * ====================================== */
  let toastTimer;
  function showToast(title, message, type = "success") {
    if (toastTitle) toastTitle.textContent = title;
    if (toastMessage) toastMessage.textContent = message;
    if (toastIcon) {
      const grad =
        type === "warning"
          ? "from-amber-500 to-orange-500"
          : type === "danger"
            ? "from-red-500 to-rose-500"
            : "from-accent to-teal-500";
      toastIcon.className = `w-10 h-10 rounded-xl bg-gradient-to-br ${grad} flex items-center justify-center shrink-0`;
    }
    toast?.classList.add("active");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(hideToast, 3500);
  }
  function hideToast() {
    toast?.classList.remove("active");
  }
  document.querySelector(".sb-toast-close")?.addEventListener("click", hideToast);

  /* ======================================
   * 03. Drawers
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
  document.querySelectorAll(".sb-close-drawer").forEach((btn) => btn.addEventListener("click", closeDrawers));

  // Hero buttons
  document.getElementById("sbChangePlanBtn")?.addEventListener("click", () => openDrawer(drawers.changePlan));
  document.getElementById("sbCancelBtn")?.addEventListener("click", () => openDrawer(drawers.cancel));
  document.getElementById("sbUpgradeBtn")?.addEventListener("click", () => openDrawer(drawers.upgrade));
  document.getElementById("sbAddCardBtn")?.addEventListener("click", () => openDrawer(drawers.addCard));
  document.getElementById("sbEditBillingBtn")?.addEventListener("click", () => openDrawer(drawers.editBilling));

  // Drawer submit actions
  document.getElementById("sbAddCardSubmit")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Card Added", "Your payment method has been added successfully");
  });
  document.getElementById("sbEditCardSubmit")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Card Updated", "Your card details have been updated");
  });
  document.getElementById("sbBillingSubmit")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Billing Updated", "Your billing address has been updated");
  });
  document.getElementById("sbUpgradeSubmit")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Upgrade Successful", "Welcome to the Enterprise plan!");
  });
  document.getElementById("sbChangePlanSubmit")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Plan Change Scheduled", "Your plan will change at the end of this billing cycle");
  });
  document.getElementById("sbCancelSubmit")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Subscription Cancelled", "Your subscription will end on January 15, 2025", "warning");
  });

  /* ======================================
   * 04. Tabs (lazy charts on Usage)
   * ====================================== */
  const tabs = Array.from(document.querySelectorAll(".sb-tab"));
  const panes = Array.from(document.querySelectorAll(".sb-pane"));
  tabs.forEach((tab) => {
    tab.addEventListener("click", () => {
      const name = tab.dataset.tab;
      tabs.forEach((t) => {
        t.classList.toggle("active", t === tab);
        t.classList.toggle("text-muted", t !== tab);
      });
      panes.forEach((p) => p.classList.toggle("active", p.dataset.pane === name));
      if (name === "usage") requestAnimationFrame(() => initCharts());
      refreshIcons();
    });
  });

  /* ======================================
   * 05. Plans: monthly/annual toggle + feature table + plan actions
   * ====================================== */
  const billingToggle = document.getElementById("sbBillingToggle");
  const monthlyLabel = document.getElementById("sbMonthlyLabel");
  const yearlyLabel = document.getElementById("sbYearlyLabel");
  const prices = {
    starter: { el: document.getElementById("sbStarterPrice"), yearly: "$23", monthly: "$29" },
    basic: { el: document.getElementById("sbBasicPrice"), yearly: "$39", monthly: "$49" },
    pro: { el: document.getElementById("sbProPrice"), yearly: "$63", monthly: "$79" },
    enterprise: { el: document.getElementById("sbEnterprisePrice"), yearly: "$159", monthly: "$199" },
  };
  let isYearly = true;

  function applyBilling() {
    billingToggle?.classList.toggle("active", isYearly);
    billingToggle?.setAttribute("aria-checked", String(isYearly));
    monthlyLabel?.classList.toggle("text-text", !isYearly);
    monthlyLabel?.classList.toggle("text-muted", isYearly);
    yearlyLabel?.classList.toggle("text-text", isYearly);
    yearlyLabel?.classList.toggle("text-muted", !isYearly);
    Object.values(prices).forEach((p) => {
      if (p.el) p.el.textContent = isYearly ? p.yearly : p.monthly;
    });
  }
  billingToggle?.addEventListener("click", () => {
    isYearly = !isYearly;
    applyBilling();
  });

  // Feature comparison show/hide
  const featureTable = document.getElementById("sbFeatureTable");
  const featureIcon = document.getElementById("sbFeatureIcon");
  const featureText = document.getElementById("sbFeatureText");
  document.getElementById("sbFeatureToggle")?.addEventListener("click", () => {
    const hidden = featureTable?.classList.toggle("hidden");
    if (featureIcon) featureIcon.setAttribute("data-lucide", hidden ? "chevron-down" : "chevron-up");
    if (featureText) featureText.textContent = hidden ? "Show All" : "Hide";
    refreshIcons();
  });

  // Plan downgrade buttons
  document.querySelectorAll(".sb-plan-action").forEach((btn) =>
    btn.addEventListener("click", () => {
      const plan = btn.dataset.plan || "selected";
      showToast("Downgrade Requested", `Switching to the ${plan} plan at the end of this cycle`, "warning");
    }),
  );

  /* ======================================
   * 06. Payment methods + billing address + auto-renew
   * ====================================== */
  function makePrimaryBadge() {
    const span = document.createElement("span");
    span.className = "sb-primary-badge absolute top-3 right-3 text-xs font-bold text-accent bg-accent/15 px-2 py-1 rounded-full";
    span.textContent = "Primary";
    return span;
  }

  // Edit card
  document.querySelectorAll(".sb-card-edit").forEach((btn) =>
    btn.addEventListener("click", () => openDrawer(drawers.editCard)),
  );

  // Remove card (DOM-only node removal)
  function bindDelete(btn) {
    btn.addEventListener("click", () => {
      const card = btn.closest(".sb-card");
      const name = card?.dataset.card || "Payment method";
      card?.remove();
      showToast("Card Removed", `${name} has been removed`, "warning");
    });
  }
  document.querySelectorAll(".sb-card-delete").forEach(bindDelete);

  // Set as primary (toggle styling + badge; DOM only)
  document.querySelectorAll(".sb-card-primary").forEach((btn) =>
    btn.addEventListener("click", () => {
      const card = btn.closest(".sb-card");
      if (!card) return;
      document.querySelectorAll("#sbCardList .sb-card").forEach((c) => {
        c.classList.remove("relative", "border-2", "border-accent/30", "bg-accent/5");
        c.classList.add("border", "border-border");
        c.querySelector(".sb-primary-badge")?.remove();
        const setBtn = c.querySelector(".sb-card-primary");
        if (setBtn) setBtn.classList.remove("hidden");
      });
      card.classList.remove("border", "border-border");
      card.classList.add("relative", "border-2", "border-accent/30", "bg-accent/5");
      card.prepend(makePrimaryBadge());
      btn.classList.add("hidden");
      showToast("Primary Card Updated", `${card.dataset.card || "This card"} is now your default`);
    }),
  );

  // Auto-renew toggle
  const autoRenew = document.getElementById("sbAutoRenew");
  autoRenew?.addEventListener("click", () => {
    const active = autoRenew.classList.toggle("active");
    autoRenew.setAttribute("aria-checked", String(active));
    showToast(
      active ? "Auto-Renew Enabled" : "Auto-Renew Disabled",
      active ? "Your subscription will automatically renew" : "Your subscription will not automatically renew",
      active ? "success" : "warning",
    );
  });

  /* ======================================
   * 07. Billing history: filter + actions + export
   * ====================================== */
  const invoiceRows = Array.from(document.querySelectorAll(".sb-invoice-row"));
  const historyNoResults = document.getElementById("sbHistoryNoResults");
  const historySearch = document.getElementById("sbHistorySearch");
  const statusFilter = document.getElementById("sbStatusFilter");
  const dateFilter = document.getElementById("sbDateFilter");
  const now = new Date();

  function applyHistoryFilters() {
    const search = (historySearch?.value || "").toLowerCase();
    const status = statusFilter?.value || "all";
    const days = dateFilter?.value || "all";
    let visible = 0;
    invoiceRows.forEach((row) => {
      const text = row.textContent.toLowerCase();
      const date = new Date(row.dataset.date);
      const daysDiff = Math.floor((now - date) / 86400000);
      let show = true;
      if (search && !text.includes(search)) show = false;
      if (status !== "all" && row.dataset.status !== status) show = false;
      if (days !== "all" && daysDiff > parseInt(days, 10)) show = false;
      row.style.display = show ? "" : "none";
      if (show) visible += 1;
    });
    historyNoResults?.classList.toggle("hidden", visible !== 0);
  }
  historySearch?.addEventListener("input", applyHistoryFilters);
  statusFilter?.addEventListener("change", applyHistoryFilters);
  dateFilter?.addEventListener("change", applyHistoryFilters);

  document.querySelectorAll(".sb-invoice-view").forEach((btn) =>
    btn.addEventListener("click", () => showToast("Opening Invoice", `Loading invoice ${btn.dataset.id}...`)),
  );
  document.querySelectorAll(".sb-invoice-download").forEach((btn) =>
    btn.addEventListener("click", () => showToast("Downloading", `Invoice ${btn.dataset.id} is being downloaded`)),
  );
  document.querySelectorAll(".sb-invoice-retry").forEach((btn) =>
    btn.addEventListener("click", () =>
      showToast("Retrying Payment", `Attempting to process payment for ${btn.dataset.id}...`, "warning"),
    ),
  );
  document.querySelectorAll(".sb-export").forEach((btn) =>
    btn.addEventListener("click", () => {
      const fmt = (btn.dataset.format || "csv").toUpperCase();
      showToast("Export Started", `Preparing your ${fmt} file...`);
    }),
  );

  /* ======================================
   * 08. Usage charts + period chips + theme re-color + keyboard
   * ====================================== */
  let signalChart, apiChart;
  function tickColor() {
    return document.documentElement.classList.contains("dark") ? "#94A3B8" : "#64748B";
  }
  function gridColor() {
    return document.documentElement.classList.contains("dark") ? "rgba(255,255,255,0.05)" : "rgba(0,0,0,0.05)";
  }

  function initCharts() {
    if (!hasChart()) return;
    const signalCanvas = document.getElementById("sbSignalUsageChart");
    if (signalCanvas && !signalChart) {
      signalChart = new Chart(signalCanvas.getContext("2d"), {
        type: "line",
        data: {
          labels: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
          datasets: [
            {
              label: "Signals Used",
              data: [120, 145, 98, 167, 134, 89, 94],
              borderColor: "#10b981",
              backgroundColor: "rgba(16,185,129,0.12)",
              borderWidth: 3,
              fill: true,
              tension: 0.4,
              pointRadius: 4,
              pointBackgroundColor: "#10b981",
            },
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
    const apiCanvas = document.getElementById("sbApiUsageChart");
    if (apiCanvas && !apiChart) {
      apiChart = new Chart(apiCanvas.getContext("2d"), {
        type: "doughnut",
        data: {
          labels: ["Signal Requests", "Data Fetches", "Webhook Calls", "Bot Actions"],
          datasets: [
            {
              data: [4500, 3800, 2450, 1700],
              backgroundColor: ["#10b981", "#0ea5e9", "#f59e0b", "#8b5cf6"],
              borderWidth: 0,
            },
          ],
        },
        options: {
          responsive: true,
          maintainAspectRatio: false,
          cutout: "65%",
          plugins: {
            legend: { position: "right", labels: { color: tickColor(), usePointStyle: true, padding: 16 } },
          },
        },
      });
    }
  }

  // Usage period chips
  document.querySelectorAll(".sb-period").forEach((chip) =>
    chip.addEventListener("click", () => {
      document.querySelectorAll(".sb-period").forEach((c) => {
        c.classList.remove("active", "border-accent", "bg-accent/15", "text-accent");
        c.classList.add("border-border", "bg-panel", "text-muted");
      });
      chip.classList.add("active", "border-accent", "bg-accent/15", "text-accent");
      chip.classList.remove("border-border", "bg-panel", "text-muted");
      if (!signalChart) return;
      const map = {
        7: [120, 145, 98, 167, 134, 89, 94],
        30: [98, 132, 110, 156, 142, 121, 138],
        90: [110, 124, 135, 118, 149, 130, 142],
      };
      signalChart.data.datasets[0].data = map[chip.dataset.period] || map[7];
      signalChart.update();
    }),
  );

  document.getElementById("themeToggle")?.addEventListener("click", () => {
    setTimeout(() => {
      if (signalChart) {
        signalChart.options.scales.y.grid.color = gridColor();
        signalChart.options.scales.y.ticks.color = tickColor();
        signalChart.options.scales.x.ticks.color = tickColor();
        signalChart.update();
      }
      if (apiChart) {
        apiChart.options.plugins.legend.labels.color = tickColor();
        apiChart.update();
      }
    }, 50);
  });

  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") closeDrawers();
  });

  // Initial render
  applyBilling();
  applyHistoryFilters();
  refreshIcons();
});
