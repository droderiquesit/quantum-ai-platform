/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: trend-scanner.js
 * description: Self-contained controller for the Trend Scanner page
 *              (#trend-scanner). DOM-only — drawers are static panels the JS
 *              shows/hides and patches via textContent / setAttribute; no HTML
 *              is generated in JS.
 * author: SignalAIX

    -------------------------------------------------
    Table of Contents
    -------------------------------------------------
    01. Init & guard
    02. Toast
    03. Tabs
    04. Filter chips + table search
    05. Filter drawer (range, volume pills, reset/apply)
    06. Detail drawer (open from row, patch fields)
    07. Charts (distribution + detail) + theme re-color
    ================================================== */

(function () {
  if (!document.getElementById("trend-scanner")) return;

  const htmlEl = document.documentElement;
  const isDark = () => htmlEl.classList.contains("dark");
  const tickColor = () => (isDark() ? "#94A3B8" : "#64748B");
  const gridColor = () => (isDark() ? "rgba(255,255,255,0.06)" : "rgba(0,0,0,0.06)");
  const refreshIcons = () => window.lucide && lucide.createIcons();

  /* 02. Toast */
  const toast = document.getElementById("tsToast");
  let toastTimer;
  function showToast(title, message) {
    if (!toast) return;
    if (title) document.getElementById("tsToastTitle").textContent = title;
    if (message) document.getElementById("tsToastMessage").textContent = message;
    toast.classList.add("active");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => toast.classList.remove("active"), 3000);
  }
  document.getElementById("tsToastClose")?.addEventListener("click", () => toast.classList.remove("active"));

  document.getElementById("tsScan")?.addEventListener("click", () => showToast("Scanning Markets", "Analyzing trends across all markets..."));
  document.querySelectorAll(".ts-scan-cta").forEach((b) => b.addEventListener("click", () => showToast("Scanning Markets", "Analyzing trends across all markets...")));
  document.querySelectorAll(".ts-export").forEach((b) => b.addEventListener("click", () => showToast("Export Started", `Exporting data as ${b.dataset.format.toUpperCase()}...`)));
  document.querySelectorAll(".ts-watch").forEach((b) =>
    b.addEventListener("click", (e) => {
      e.stopPropagation();
      showToast("Added to Watchlist", `${b.dataset.pair} has been added to your watchlist`);
    })
  );
  document.querySelectorAll(".ts-toast-btn").forEach((b) => b.addEventListener("click", () => showToast(b.dataset.toastTitle, b.dataset.toastMsg)));

  /* 03. Tabs */
  document.querySelectorAll(".ts-tab").forEach((tab) => {
    tab.addEventListener("click", () => {
      document.querySelectorAll(".ts-tab").forEach((t) => t.classList.remove("active"));
      document.querySelectorAll(".tab-content").forEach((c) => c.classList.remove("active"));
      tab.classList.add("active");
      document.getElementById(tab.dataset.tab)?.classList.add("active");
      refreshIcons();
    });
  });

  /* 04. Filter chips + table search */
  document.querySelectorAll(".ts-chip").forEach((chip) => {
    chip.addEventListener("click", () => {
      document.querySelectorAll(".ts-chip").forEach((c) => {
        c.classList.remove("active", "bg-accent/10", "border-accent", "text-accent");
        c.classList.add("text-muted");
      });
      chip.classList.add("active", "bg-accent/10", "border-accent", "text-accent");
      chip.classList.remove("text-muted");
      showToast("Filter Applied", `Showing ${chip.dataset.filter.replace("-", " ")} trends`);
    });
  });

  const rows = Array.from(document.querySelectorAll(".ts-row"));
  document.getElementById("tsTableSearch")?.addEventListener("input", (e) => {
    const term = e.target.value.toLowerCase();
    rows.forEach((r) => (r.style.display = r.textContent.toLowerCase().includes(term) ? "" : "none"));
  });

  /* 05. Filter drawer */
  const overlay = document.getElementById("tsOverlay");
  const filterDrawer = document.getElementById("tsFilterDrawer");
  const detailDrawer = document.getElementById("tsDetailDrawer");

  function openDrawer(d) {
    overlay?.classList.add("active");
    d?.classList.add("active");
    refreshIcons();
  }
  function closeDrawers() {
    overlay?.classList.remove("active");
    filterDrawer?.classList.remove("active");
    detailDrawer?.classList.remove("active");
  }
  document.getElementById("tsFilters")?.addEventListener("click", () => openDrawer(filterDrawer));
  document.querySelectorAll(".ts-drawer-close").forEach((b) => b.addEventListener("click", closeDrawers));
  overlay?.addEventListener("click", closeDrawers);
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") closeDrawers();
  });

  const range = document.getElementById("tsStrengthRange");
  const rangeVal = document.getElementById("tsStrengthValue");
  range?.addEventListener("input", () => { if (rangeVal) rangeVal.textContent = `${range.value}%`; });

  document.querySelectorAll(".ts-vol").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll(".ts-vol").forEach((b) => {
        b.classList.remove("border-accent/40", "bg-accent/10", "text-accent", "font-medium");
        b.classList.add("border-border", "text-muted");
      });
      btn.classList.add("border-accent/40", "bg-accent/10", "text-accent", "font-medium");
      btn.classList.remove("border-border", "text-muted");
    });
  });

  document.querySelector(".ts-reset")?.addEventListener("click", () => {
    if (range) { range.value = 50; if (rangeVal) rangeVal.textContent = "50%"; }
    showToast("Filters Reset", "All filters have been reset to default");
  });
  document.querySelector(".ts-apply")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Filters Applied", "Your filter preferences have been applied");
  });

  /* 06. Detail drawer */
  document.querySelectorAll(".ts-open").forEach((row) => {
    row.addEventListener("click", (e) => {
      if (e.target.closest(".ts-watch")) return;
      const d = row.dataset;
      document.getElementById("tsDetailName").textContent = d.pair;
      document.getElementById("tsDetailDesc").textContent = d.desc;
      document.getElementById("tsDetailPrice").textContent = d.price;
      const ch = document.getElementById("tsDetailChange");
      ch.textContent = d.change;
      ch.className = `text-2xl font-bold ${d.changecls}`;
      const iconWrap = document.getElementById("tsDetailIcon").parentElement;
      iconWrap.className = `w-12 h-12 rounded-xl ${d.iconwrap} flex items-center justify-center shrink-0`;
      openDrawer(detailDrawer);
      initDetailChart();
    });
  });

  /* 07. Charts */
  const charts = {};
  function initDistribution() {
    if (typeof Chart === "undefined") return;
    const el = document.getElementById("tsDistributionChart");
    if (!el || charts.dist) return;
    charts.dist = new Chart(el.getContext("2d"), {
      type: "doughnut",
      data: {
        labels: ["Strong Bullish", "Bullish", "Neutral", "Bearish", "Strong Bearish"],
        datasets: [{ data: [15, 24, 32, 18, 11], backgroundColor: ["#10B981", "#34D399", "#94A3B8", "#F87171", "#EF4444"], borderWidth: 0, spacing: 2 }],
      },
      options: { responsive: true, maintainAspectRatio: false, cutout: "65%", plugins: { legend: { position: "bottom", labels: { color: tickColor(), usePointStyle: true, padding: 14, font: { size: 11 } } } } },
    });
  }
  function initDetailChart() {
    if (typeof Chart === "undefined") return;
    const el = document.getElementById("tsDetailChart");
    if (!el) return;
    if (charts.detail) charts.detail.destroy();
    const ctx = el.getContext("2d");
    const grad = ctx.createLinearGradient(0, 0, 0, 220);
    grad.addColorStop(0, "rgba(99,102,241,0.3)");
    grad.addColorStop(1, "rgba(99,102,241,0)");
    charts.detail = new Chart(ctx, {
      type: "line",
      data: { labels: ["00:00", "04:00", "08:00", "12:00", "16:00", "20:00", "Now"], datasets: [{ data: [1.082, 1.0825, 1.0835, 1.0828, 1.084, 1.0845, 1.0847], borderColor: "#6366F1", backgroundColor: grad, borderWidth: 2, fill: true, tension: 0.4, pointRadius: 3, pointBackgroundColor: "#6366F1" }] },
      options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { display: false } }, scales: { y: { grid: { color: gridColor() }, ticks: { color: tickColor(), callback: (v) => v.toFixed(4) } }, x: { grid: { display: false }, ticks: { color: tickColor() } } } },
    });
  }

  function recolor() {
    if (charts.dist) { charts.dist.options.plugins.legend.labels.color = tickColor(); charts.dist.update(); }
    if (charts.detail) {
      const s = charts.detail.options.scales;
      s.y.grid.color = gridColor(); s.y.ticks.color = tickColor(); s.x.ticks.color = tickColor();
      charts.detail.update();
    }
  }
  document.getElementById("themeToggle")?.addEventListener("click", () => setTimeout(recolor, 0));

  if (document.readyState === "complete") initDistribution();
  else window.addEventListener("load", initDistribution);
})();
