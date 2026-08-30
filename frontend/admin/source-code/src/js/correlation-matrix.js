/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: correlation-matrix.js
 * description: Self-contained controller for the Correlation Matrix page
 *              (#correlation-matrix). DOM-only — all 5 matrices and their top
 *              lists are pre-rendered static HTML (build-time, same seed as the
 *              reference) toggled by tab/sort; the detail drawer is patched via
 *              textContent. No HTML generated in JS.
 * author: SignalAIX

    -------------------------------------------------
    Table of Contents
    -------------------------------------------------
    01. Init & guard
    02. Toast
    03. Tabs (show matrix + top list) + timeframe
    04. Sort toggle (show matching top list)
    05. Matrix search + cell click → detail drawer
    06. Filter drawer + history chart + theme re-color
    ================================================== */

(function () {
  if (!document.getElementById("correlation-matrix")) return;

  const htmlEl = document.documentElement;
  const isDark = () => htmlEl.classList.contains("dark");
  const tickColor = () => (isDark() ? "#94A3B8" : "#64748B");
  const gridColor = () => (isDark() ? "rgba(255,255,255,0.06)" : "rgba(0,0,0,0.06)");
  const refreshIcons = () => window.lucide && lucide.createIcons();

  let activeTab = "forex";
  let activeSort = "positive";
  let activeTf = "1D";

  /* 02. Toast */
  const toast = document.getElementById("cmToast");
  let toastTimer;
  function showToast(title, message) {
    if (!toast) return;
    if (title) document.getElementById("cmToastTitle").textContent = title;
    if (message) document.getElementById("cmToastMessage").textContent = message;
    toast.classList.add("active");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => toast.classList.remove("active"), 3000);
  }
  document.getElementById("cmToastClose")?.addEventListener("click", () => toast.classList.remove("active"));
  document.querySelectorAll(".cm-export").forEach((b) => b.addEventListener("click", () => showToast("Export Started", `Exporting correlation matrix as ${b.dataset.format.toUpperCase()}`)));
  document.getElementById("cmRefresh")?.addEventListener("click", () => showToast("Data Refreshed", "Correlation matrix updated"));

  /* 03. Tabs + timeframe */
  function syncViews() {
    document.querySelectorAll(".cm-matrix").forEach((m) => m.classList.toggle("hidden", m.dataset.tab !== activeTab));
    document.querySelectorAll(".cm-toplist").forEach((l) => l.classList.toggle("hidden", !(l.dataset.tab === activeTab && l.dataset.sort === activeSort)));
    bindCells();
    refreshIcons();
  }
  document.querySelectorAll(".cm-tab").forEach((tab) => {
    tab.addEventListener("click", () => {
      document.querySelectorAll(".cm-tab").forEach((t) => t.classList.remove("active"));
      tab.classList.add("active");
      activeTab = tab.dataset.tab;
      syncViews();
      showToast("Tab Changed", `Showing ${activeTab} correlations`);
    });
  });
  document.querySelectorAll(".cm-tf").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll(".cm-tf").forEach((b) => {
        b.classList.remove("active", "bg-accent/10", "border-accent", "text-accent");
        b.classList.add("text-muted");
      });
      btn.classList.add("active", "bg-accent/10", "border-accent", "text-accent");
      btn.classList.remove("text-muted");
      activeTf = btn.dataset.tf;
      const tf = document.getElementById("cmDetailTf");
      if (tf) tf.textContent = activeTf;
      showToast("Timeframe Updated", `Now showing ${activeTf} correlations`);
    });
  });

  /* 04. Sort */
  document.getElementById("cmSort")?.addEventListener("change", (e) => {
    activeSort = e.target.value;
    syncViews();
  });

  /* 05. Search + cell click */
  document.getElementById("cmSearch")?.addEventListener("input", (e) => {
    const q = e.target.value.toLowerCase();
    const visible = document.querySelector(`.cm-matrix[data-tab="${activeTab}"]`);
    visible?.querySelectorAll("tbody tr").forEach((row) => {
      const label = row.querySelector("td .text-text");
      const name = label ? label.textContent.toLowerCase() : "";
      row.style.display = !q || name.includes(q) ? "" : "none";
    });
  });

  const overlay = document.getElementById("cmOverlay");
  const filterDrawer = document.getElementById("cmFilterDrawer");
  const detailDrawer = document.getElementById("cmDetailDrawer");
  function setText(id, v) { const el = document.getElementById(id); if (el) el.textContent = v; }

  function openDetail(pair1, pair2, value) {
    setText("cmDetailTitle", `${pair1} vs ${pair2}`);
    const strength = Math.abs(value) >= 0.7 ? "Strong" : Math.abs(value) >= 0.4 ? "Moderate" : "Weak";
    const direction = value > 0 ? "Positive" : value < 0 ? "Negative" : "Neutral";
    const corrEl = document.getElementById("cmDetailCorr");
    if (corrEl) { corrEl.textContent = value.toFixed(2); corrEl.className = `text-2xl font-bold ${value > 0 ? "text-emerald-500" : value < 0 ? "text-red-500" : "text-muted"}`; }
    setText("cmDetailStrength", strength);
    setText("cmDetailDir", direction);
    setText("cmDetailTf", activeTf);
    setText("cmDetailR2", (value * value).toFixed(3));
    setText("cmDetailCI", `[${(value - 0.08).toFixed(2)}, ${(value + 0.08).toFixed(2)}]`);
    const marker = document.getElementById("cmDetailMarker");
    if (marker) marker.style.left = `${((value + 1) / 2) * 100}%`;
    overlay?.classList.add("active");
    detailDrawer?.classList.add("active");
    refreshIcons();
  }

  function bindCells() {
    document.querySelectorAll(`.cm-matrix[data-tab="${activeTab}"] .cm-cell`).forEach((cell) => {
      if (cell.dataset.bound) return;
      cell.dataset.bound = "1";
      cell.addEventListener("click", () => openDetail(cell.dataset.pair1, cell.dataset.pair2, parseFloat(cell.dataset.value)));
    });
    document.querySelectorAll(`.cm-toplist[data-tab="${activeTab}"] .cm-top`).forEach((row) => {
      if (row.dataset.bound) return;
      row.dataset.bound = "1";
      row.addEventListener("click", () => openDetail(row.dataset.pair1, row.dataset.pair2, parseFloat(row.dataset.value)));
    });
  }

  /* 06. Drawers + chart */
  function closeDrawers() {
    overlay?.classList.remove("active");
    filterDrawer?.classList.remove("active");
    detailDrawer?.classList.remove("active");
  }
  document.getElementById("cmFilters")?.addEventListener("click", () => { overlay?.classList.add("active"); filterDrawer?.classList.add("active"); refreshIcons(); });
  document.querySelectorAll(".cm-drawer-close").forEach((b) => b.addEventListener("click", closeDrawers));
  overlay?.addEventListener("click", closeDrawers);
  document.addEventListener("keydown", (e) => { if (e.key === "Escape") closeDrawers(); });
  document.querySelectorAll(".cm-toast-btn").forEach((b) =>
    b.addEventListener("click", () => { showToast(b.dataset.toastTitle, b.dataset.toastMsg); if (b.hasAttribute("data-close")) closeDrawers(); })
  );

  let chart;
  function seedSeries(p1, p2) {
    // deterministic pseudo-series in [-1,1] from the pair names
    const base = (p1 + p2).split("").reduce((a, c) => a + c.charCodeAt(0), 0);
    return Array.from({ length: 31 }, (_, i) => {
      const r = Math.sin(base + i * 1.7) * 10000;
      return Math.round(((r - Math.floor(r)) * 1.6 - 0.3) * 100) / 100;
    });
  }
  function initChart() {
    if (typeof Chart === "undefined") return;
    const el = document.getElementById("cmHistoryChart");
    if (!el) return;
    const p1 = document.getElementById("cmHistPair1")?.value || "EURUSD";
    const p2 = document.getElementById("cmHistPair2")?.value || "GBPUSD";
    const data = seedSeries(p1, p2);
    if (chart) chart.destroy();
    chart = new Chart(el.getContext("2d"), {
      type: "line",
      data: { labels: data.map((_, i) => `D-${30 - i}`), datasets: [{ label: "Correlation", data, borderColor: "#8B5CF6", backgroundColor: "rgba(139,92,246,0.1)", borderWidth: 2, fill: true, tension: 0.4, pointRadius: 0 }] },
      options: { responsive: true, maintainAspectRatio: false, interaction: { mode: "index", intersect: false }, plugins: { legend: { display: false } }, scales: { x: { grid: { color: gridColor() }, ticks: { color: tickColor(), maxTicksLimit: 8, font: { size: 11 } } }, y: { min: -1, max: 1, grid: { color: gridColor() }, ticks: { color: tickColor(), font: { size: 11 }, callback: (v) => v.toFixed(1) } } } },
    });
  }
  document.getElementById("cmHistPair1")?.addEventListener("change", initChart);
  document.getElementById("cmHistPair2")?.addEventListener("change", initChart);
  document.getElementById("themeToggle")?.addEventListener("click", () => setTimeout(initChart, 0));

  bindCells();
  if (document.readyState === "complete") initChart();
  else window.addEventListener("load", initChart);
})();
