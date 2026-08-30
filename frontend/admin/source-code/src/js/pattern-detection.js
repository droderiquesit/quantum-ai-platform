/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: pattern-detection.js
 * description: Self-contained controller for the Pattern Detection page
 *              (#pattern-detection). DOM-only — the drawer bodies are static
 *              panels the JS shows/hides; no HTML is generated in JS.
 * author: SignalAIX

    -------------------------------------------------
    Table of Contents
    -------------------------------------------------
    01. Init & guard
    02. Toast
    03. Tabs
    04. Filter chips + table search + scan timeframes
    05. Drawer (detail / scan / filter)
    06. Charts (mini sparklines, distribution, accuracy, detail) + theme re-color
    ================================================== */

(function () {
  if (!document.getElementById("pattern-detection")) return;

  const htmlEl = document.documentElement;
  const isDark = () => htmlEl.classList.contains("dark");
  const tickColor = () => (isDark() ? "#94A3B8" : "#64748B");
  const gridColor = () => (isDark() ? "rgba(255,255,255,0.06)" : "rgba(0,0,0,0.06)");
  const refreshIcons = () => window.lucide && lucide.createIcons();

  /* 02. Toast */
  const toast = document.getElementById("pdToast");
  let toastTimer;
  function showToast(title, message) {
    if (!toast) return;
    if (title) document.getElementById("pdToastTitle").textContent = title;
    if (message) document.getElementById("pdToastMessage").textContent = message;
    toast.classList.add("active");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => toast.classList.remove("active"), 3000);
  }
  document.getElementById("pdToastClose")?.addEventListener("click", () => toast.classList.remove("active"));
  document.querySelectorAll(".pd-export").forEach((b) => b.addEventListener("click", () => showToast("Export Started", `Exporting patterns as ${b.dataset.format.toUpperCase()}...`)));
  document.querySelector(".pd-refresh")?.addEventListener("click", () => showToast("Refreshing", "Updating pattern data..."));

  /* 03. Tabs */
  document.querySelectorAll(".pd-tab").forEach((tab) => {
    tab.addEventListener("click", () => {
      document.querySelectorAll(".pd-tab").forEach((t) => t.classList.remove("active"));
      document.querySelectorAll(".tab-content").forEach((c) => c.classList.remove("active"));
      tab.classList.add("active");
      document.getElementById(tab.dataset.tab)?.classList.add("active");
      refreshIcons();
    });
  });

  /* 04. Filter chips + table search + scan timeframes */
  document.querySelectorAll(".pd-chip").forEach((chip) => {
    chip.addEventListener("click", () => {
      document.querySelectorAll(".pd-chip").forEach((c) => {
        c.classList.remove("active", "bg-accent/10", "border-accent", "text-accent");
        c.classList.add("text-muted");
      });
      chip.classList.add("active", "bg-accent/10", "border-accent", "text-accent");
      chip.classList.remove("text-muted");
      showToast("Filter Applied", `Showing ${chip.dataset.filter.replace("-", " ")} patterns`);
    });
  });

  const rows = Array.from(document.querySelectorAll(".pd-row"));
  document.getElementById("pdTableSearch")?.addEventListener("input", (e) => {
    const term = e.target.value.toLowerCase();
    rows.forEach((r) => (r.style.display = r.textContent.toLowerCase().includes(term) ? "" : "none"));
  });

  document.querySelectorAll(".pd-tf").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll(".pd-tf").forEach((b) => {
        b.classList.remove("active", "bg-accent/10", "border-accent", "text-accent");
        b.classList.add("text-muted");
      });
      btn.classList.add("active", "bg-accent/10", "border-accent", "text-accent");
      btn.classList.remove("text-muted");
    });
  });
  const scanConf = document.getElementById("pdScanConf");
  const scanConfVal = document.getElementById("pdScanConfVal");
  scanConf?.addEventListener("input", () => { if (scanConfVal) scanConfVal.textContent = `${scanConf.value}%`; });

  /* 05. Drawer */
  const overlay = document.getElementById("pdOverlay");
  const drawer = document.getElementById("pdDrawer");
  const drawerTitle = document.getElementById("pdDrawerTitle");
  const panels = drawer ? drawer.querySelectorAll(".pd-panel") : [];
  const TITLES = { detail: "Pattern Details", scan: "Quick Pattern Scan", filter: "Advanced Filters" };

  function showPanel(name) {
    panels.forEach((p) => (p.hidden = p.dataset.panel !== name));
    if (drawerTitle) drawerTitle.textContent = TITLES[name] || "Details";
  }
  function openDrawer(name) {
    showPanel(name);
    overlay?.classList.add("active");
    drawer?.classList.add("active");
    refreshIcons();
    if (name === "detail") initDetailChart();
  }
  function closeDrawer() {
    overlay?.classList.remove("active");
    drawer?.classList.remove("active");
  }
  document.querySelectorAll(".pd-open-drawer").forEach((btn) => btn.addEventListener("click", () => openDrawer(btn.dataset.drawer)));
  document.getElementById("pdFilters")?.addEventListener("click", () => openDrawer("filter"));
  drawer?.querySelectorAll(".pd-drawer-close").forEach((b) => b.addEventListener("click", closeDrawer));
  overlay?.addEventListener("click", closeDrawer);
  document.addEventListener("keydown", (e) => { if (e.key === "Escape") closeDrawer(); });

  document.querySelectorAll(".pd-toast-btn").forEach((b) =>
    b.addEventListener("click", () => {
      showToast(b.dataset.toastTitle, b.dataset.toastMsg);
      if (b.hasAttribute("data-close")) closeDrawer();
    })
  );

  /* 06. Charts */
  const charts = {};
  const sparks = [];
  function mini(id, color, data) {
    const el = document.getElementById(id);
    if (!el || typeof Chart === "undefined") return;
    sparks.push(new Chart(el.getContext("2d"), {
      type: "line",
      data: { labels: data.map(() => ""), datasets: [{ data, borderColor: color, borderWidth: 2, tension: 0.3, pointRadius: 0, fill: false }] },
      options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { display: false } }, scales: { x: { display: false }, y: { display: false } } },
    }));
  }
  function initCharts() {
    if (typeof Chart === "undefined" || charts.done) return;
    charts.done = true;
    mini("pdMini1", "#10B981", [65, 40, 50, 35, 55, 35, 60, 75, 85, 90]);
    mini("pdMini2", "#EF4444", [30, 50, 45, 70, 90, 65, 80, 60, 40, 35]);
    mini("pdMini3", "#10B981", [30, 55, 35, 58, 40, 60, 45, 62, 55, 85]);
    mini("pdMini4", "#8B5CF6", [60, 55, 50, 45, 35, 30, 45, 60, 70, 80]);

    const dist = document.getElementById("pdDistChart");
    if (dist) charts.dist = new Chart(dist.getContext("2d"), {
      type: "doughnut",
      data: { labels: ["Chart Patterns", "Candlestick", "Harmonic", "AI Detected"], datasets: [{ data: [95, 72, 45, 35], backgroundColor: ["#0EA5E9", "#8B5CF6", "#10B981", "#F97316"], borderWidth: 0, hoverOffset: 8 }] },
      options: { responsive: true, maintainAspectRatio: false, cutout: "70%", plugins: { legend: { display: false } } },
    });

    const acc = document.getElementById("pdAccuracyChart");
    if (acc) charts.acc = new Chart(acc.getContext("2d"), {
      type: "line",
      data: { labels: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"], datasets: [{ data: [92, 94, 91, 96, 93, 95, 94.7], borderColor: "#6366F1", backgroundColor: "rgba(99,102,241,0.1)", fill: true, tension: 0.4, borderWidth: 3, pointRadius: 3, pointBackgroundColor: "#6366F1" }] },
      options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { display: false } }, scales: { y: { min: 85, max: 100, grid: { color: gridColor() }, ticks: { color: tickColor(), callback: (v) => v + "%" } }, x: { grid: { display: false }, ticks: { color: tickColor() } } } },
    });
  }
  function initDetailChart() {
    if (typeof Chart === "undefined") return;
    const el = document.getElementById("pdDetailChart");
    if (!el) return;
    if (charts.detail) charts.detail.destroy();
    charts.detail = new Chart(el.getContext("2d"), {
      type: "line",
      data: { labels: Array.from({ length: 12 }, () => ""), datasets: [{ data: [60, 40, 48, 32, 50, 30, 45, 58, 70, 78, 85, 90], borderColor: "#10B981", backgroundColor: "rgba(16,185,129,0.12)", fill: true, tension: 0.35, borderWidth: 2, pointRadius: 0 }] },
      options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { display: false } }, scales: { x: { display: false }, y: { display: false } } },
    });
  }

  function recolor() {
    if (charts.acc) { const s = charts.acc.options.scales; s.y.grid.color = gridColor(); s.y.ticks.color = tickColor(); s.x.ticks.color = tickColor(); charts.acc.update(); }
  }
  document.getElementById("themeToggle")?.addEventListener("click", () => setTimeout(recolor, 0));

  if (document.readyState === "complete") initCharts();
  else window.addEventListener("load", initCharts);
})();
