/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: economic-calendar.js
 * description: Self-contained controller for the Economic Calendar page
 *              (#economic-calendar). DOM-only — table rows, list cards, timeline,
 *              mini-calendar and drawer bodies are static templates the JS
 *              shows/hides/filters and patches via textContent; no HTML in JS.
 * author: SignalAIX

    -------------------------------------------------
    Table of Contents
    -------------------------------------------------
    01. Init & guard
    02. Toast
    03. Tabs (lazy-init analytics charts)
    04. Impact + currency chips, search (filter all three views)
    05. Filter drawer + event detail drawer (patch from data-*)
    06. Charts (currency / impact / trends / history) + theme re-color
    ================================================== */

(function () {
  if (!document.getElementById("economic-calendar")) return;

  const htmlEl = document.documentElement;
  const isDark = () => htmlEl.classList.contains("dark");
  const tickColor = () => (isDark() ? "#94A3B8" : "#64748B");
  const gridColor = () => (isDark() ? "rgba(255,255,255,0.06)" : "rgba(0,0,0,0.06)");
  const refreshIcons = () => window.lucide && lucide.createIcons();

  /* 02. Toast */
  const toast = document.getElementById("ecToast");
  let toastTimer;
  function showToast(title, message) {
    if (!toast) return;
    if (title) document.getElementById("ecToastTitle").textContent = title;
    if (message) document.getElementById("ecToastMessage").textContent = message;
    toast.classList.add("active");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => toast.classList.remove("active"), 3000);
  }
  document.getElementById("ecToastClose")?.addEventListener("click", () => toast.classList.remove("active"));
  document.querySelectorAll(".ec-export").forEach((b) => b.addEventListener("click", () => showToast("Export Started", `Exporting data as ${b.dataset.format.toUpperCase()}...`)));
  document.getElementById("ecRefresh")?.addEventListener("click", () => showToast("Refreshing", "Updating economic calendar data..."));
  document.getElementById("ecPrevDay")?.addEventListener("click", () => showToast("Date Changed", "Showing events for previous day"));
  document.getElementById("ecNextDay")?.addEventListener("click", () => showToast("Date Changed", "Showing events for next day"));
  document.getElementById("ecTodayBtn")?.addEventListener("click", () => showToast("Today", "Showing today's events"));
  document.querySelectorAll(".ec-iconbtn").forEach((b) =>
    b.addEventListener("click", (e) => {
      e.stopPropagation();
      showToast(b.dataset.toastTitle, b.dataset.toastMsg);
    })
  );

  /* 03. Tabs */
  document.querySelectorAll(".ec-tab").forEach((tab) => {
    tab.addEventListener("click", () => {
      document.querySelectorAll(".ec-tab").forEach((t) => t.classList.remove("active"));
      document.querySelectorAll(".tab-content").forEach((c) => c.classList.remove("active"));
      tab.classList.add("active");
      document.getElementById(`ec-${tab.dataset.tab}`)?.classList.add("active");
      if (tab.dataset.tab === "analytics") initAnalytics();
      refreshIcons();
    });
  });

  /* 04. Filters + search */
  const events = Array.from(document.querySelectorAll(".ec-event"));
  let activeImpact = "all";
  let activeCcy = new Set();
  let searchTerm = "";
  function applyFilter() {
    events.forEach((el) => {
      const d = el.dataset;
      const okImpact = activeImpact === "all" || d.impact === activeImpact;
      const okCcy = activeCcy.size === 0 || activeCcy.has(d.ccy.toLowerCase());
      const okSearch = !searchTerm || `${d.event} ${d.ccy} ${d.cat}`.toLowerCase().includes(searchTerm);
      el.style.display = okImpact && okCcy && okSearch ? "" : "none";
    });
  }
  document.querySelectorAll(".ec-impact").forEach((chip) => {
    chip.addEventListener("click", () => {
      document.querySelectorAll(".ec-impact").forEach((c) => {
        c.classList.remove("active", "bg-accent/10", "border-accent", "text-accent");
        c.classList.add("text-muted");
      });
      chip.classList.add("active", "bg-accent/10", "border-accent", "text-accent");
      chip.classList.remove("text-muted");
      activeImpact = chip.dataset.filter;
      applyFilter();
    });
  });
  document.querySelectorAll(".ec-ccy").forEach((chip) => {
    chip.addEventListener("click", () => {
      const v = chip.dataset.filter;
      if (activeCcy.has(v)) { activeCcy.delete(v); chip.classList.remove("bg-accent/10", "border-accent", "text-accent"); chip.classList.add("text-muted"); }
      else { activeCcy.add(v); chip.classList.add("bg-accent/10", "border-accent", "text-accent"); chip.classList.remove("text-muted"); }
      applyFilter();
    });
  });
  document.getElementById("ecSearch")?.addEventListener("input", (e) => { searchTerm = e.target.value.toLowerCase(); applyFilter(); });

  /* 05. Drawers */
  const overlay = document.getElementById("ecOverlay");
  const filterDrawer = document.getElementById("ecFilterDrawer");
  const detailDrawer = document.getElementById("ecDetailDrawer");
  const impactBadge = { high: ["text-red-500", "bg-red-500/15"], medium: ["text-amber-500", "bg-amber-500/15"], low: ["text-emerald-500", "bg-emerald-500/15"] };

  function closeDrawers() {
    overlay?.classList.remove("active");
    filterDrawer?.classList.remove("active");
    detailDrawer?.classList.remove("active");
  }
  document.getElementById("ecFilters")?.addEventListener("click", () => { overlay?.classList.add("active"); filterDrawer?.classList.add("active"); refreshIcons(); });
  document.querySelectorAll(".ec-drawer-close").forEach((b) => b.addEventListener("click", closeDrawers));
  overlay?.addEventListener("click", closeDrawers);
  document.addEventListener("keydown", (e) => { if (e.key === "Escape") closeDrawers(); });
  document.querySelectorAll(".ec-toast-btn").forEach((b) =>
    b.addEventListener("click", () => { showToast(b.dataset.toastTitle, b.dataset.toastMsg); if (b.hasAttribute("data-close")) closeDrawers(); })
  );

  function setText(id, v) { const el = document.getElementById(id); if (el) el.textContent = v; }
  const pairsBox = document.getElementById("ecDetailPairs");
  const pairTemplate = pairsBox ? pairsBox.children[0].cloneNode(true) : null;

  events.forEach((el) => {
    el.addEventListener("click", (e) => {
      if (e.target.closest(".ec-iconbtn")) return;
      const d = el.dataset;
      setText("ecDetailTitle", d.event);
      setText("ecDetailFlag", d.flag);
      setText("ecDetailCcy", d.ccy);
      setText("ecDetailCat", d.cat);
      setText("ecDetailTime", d.time);
      setText("ecDetailActual", d.actual);
      setText("ecDetailForecast", d.forecast);
      setText("ecDetailPrevious", d.previous);
      setText("ecDetailDesc", d.desc);
      setText("ecDetailSource", d.src);
      setText("ecDetailAi", `Based on historical data and current market conditions, this event may cause increased volatility in ${d.ccy} pairs. Consider adjusting position sizes and stop-loss levels accordingly.`);
      const ib = document.getElementById("ecDetailImpact");
      if (ib) {
        const [t, b] = impactBadge[d.impact] || impactBadge.low;
        ib.className = `inline-flex items-center gap-1 px-2.5 py-1 rounded-md text-xs font-semibold ${b} ${t}`;
        setText("ecDetailImpactText", `${d.impact.charAt(0).toUpperCase() + d.impact.slice(1)} Impact`);
      }
      // affected pairs: clone the static template node, swap textContent only (no markup strings)
      if (pairsBox && pairTemplate) {
        const pairs = (d.pairs || "").split(",").filter(Boolean);
        pairsBox.replaceChildren();
        pairs.forEach((p) => { const n = pairTemplate.cloneNode(true); n.textContent = p; pairsBox.appendChild(n); });
      }
      overlay?.classList.add("active");
      detailDrawer?.classList.add("active");
      refreshIcons();
      initHistoryChart();
    });
  });

  /* 06. Charts */
  const charts = {};
  function initAnalytics() {
    if (typeof Chart === "undefined" || charts.analyticsDone) return;
    charts.analyticsDone = true;
    const cur = document.getElementById("ecCurrencyChart");
    if (cur) charts.cur = new Chart(cur.getContext("2d"), {
      type: "bar",
      data: { labels: ["USD", "EUR", "GBP", "JPY", "AUD", "CAD", "CHF", "NZD"], datasets: [{ data: [4, 2, 2, 1, 1, 1, 1, 1], backgroundColor: ["#6366F1", "#8B5CF6", "#EC4899", "#14B8A6", "#F59E0B", "#EF4444", "#10B981", "#3B82F6"], borderRadius: 8 }] },
      options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { display: false } }, scales: { y: { beginAtZero: true, grid: { color: gridColor() }, ticks: { color: tickColor() } }, x: { grid: { display: false }, ticks: { color: tickColor() } } } },
    });
    const imp = document.getElementById("ecImpactChart");
    if (imp) charts.imp = new Chart(imp.getContext("2d"), {
      type: "doughnut",
      data: { labels: ["High Impact", "Medium Impact", "Low Impact"], datasets: [{ data: [3, 5, 5], backgroundColor: ["#EF4444", "#F59E0B", "#10B981"], borderWidth: 0, hoverOffset: 8 }] },
      options: { responsive: true, maintainAspectRatio: false, cutout: "65%", plugins: { legend: { position: "right", labels: { color: tickColor(), usePointStyle: true, padding: 16 } } } },
    });
    const tr = document.getElementById("ecTrendsChart");
    if (tr) charts.tr = new Chart(tr.getContext("2d"), {
      type: "line",
      data: { labels: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"], datasets: [
        { label: "High Impact", data: [5, 8, 6, 9, 12, 3, 8], borderColor: "#EF4444", backgroundColor: "rgba(239,68,68,0.1)", fill: true, tension: 0.4, borderWidth: 2 },
        { label: "Medium Impact", data: [8, 12, 10, 15, 18, 6, 12], borderColor: "#F59E0B", backgroundColor: "rgba(245,158,11,0.1)", fill: true, tension: 0.4, borderWidth: 2 },
        { label: "Low Impact", data: [12, 15, 14, 18, 22, 8, 15], borderColor: "#10B981", backgroundColor: "rgba(16,185,129,0.1)", fill: true, tension: 0.4, borderWidth: 2 },
      ] },
      options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { position: "top", align: "end", labels: { color: tickColor(), usePointStyle: true, padding: 16 } } }, scales: { y: { beginAtZero: true, grid: { color: gridColor() }, ticks: { color: tickColor() } }, x: { grid: { display: false }, ticks: { color: tickColor() } } } },
    });
  }
  function initHistoryChart() {
    if (typeof Chart === "undefined") return;
    const el = document.getElementById("ecHistoryChart");
    if (!el) return;
    if (charts.hist) charts.hist.destroy();
    charts.hist = new Chart(el.getContext("2d"), {
      type: "line",
      data: { labels: ["Jan", "Feb", "Mar", "Apr", "May", "Jun"], datasets: [{ data: [150, 180, 165, 195, 212, 256], borderColor: "#6366F1", backgroundColor: "rgba(99,102,241,0.1)", fill: true, tension: 0.4, borderWidth: 2, pointRadius: 0 }] },
      options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { display: false } }, scales: { y: { grid: { color: gridColor() }, ticks: { color: tickColor() } }, x: { grid: { display: false }, ticks: { color: tickColor() } } } },
    });
  }
  function recolor() {
    Object.values(charts).forEach((c) => {
      if (!c || typeof c.update !== "function") return;
      const s = c.options.scales || {};
      if (s.y) { if (s.y.grid) s.y.grid.color = gridColor(); if (s.y.ticks) s.y.ticks.color = tickColor(); }
      if (s.x && s.x.ticks) s.x.ticks.color = tickColor();
      if (c.options.plugins?.legend?.labels) c.options.plugins.legend.labels.color = tickColor();
      c.update();
    });
  }
  document.getElementById("themeToggle")?.addEventListener("click", () => setTimeout(recolor, 0));
})();
