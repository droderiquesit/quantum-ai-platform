/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: news-sentiment.js
 * description: Self-contained controller for the News & Sentiment page
 *              (#news-sentiment). DOM-only — news cards are static; the detail
 *              drawer is one static panel patched via textContent/className; no
 *              HTML is generated in JS.
 * author: SignalAIX

    -------------------------------------------------
    Table of Contents
    -------------------------------------------------
    01. Init & guard
    02. Toast
    03. Tabs
    04. Sentiment chips (filter cards)
    05. Drawers (news detail patched from data-*, filter)
    06. Sentiment chart + theme re-color
    ================================================== */

(function () {
  if (!document.getElementById("news-sentiment")) return;

  const htmlEl = document.documentElement;
  const isDark = () => htmlEl.classList.contains("dark");
  const tickColor = () => (isDark() ? "#94A3B8" : "#64748B");
  const gridColor = () => (isDark() ? "rgba(255,255,255,0.06)" : "rgba(0,0,0,0.06)");
  const refreshIcons = () => window.lucide && lucide.createIcons();

  /* 02. Toast */
  const toast = document.getElementById("nsToast");
  let toastTimer;
  function showToast(title, message) {
    if (!toast) return;
    if (title) document.getElementById("nsToastTitle").textContent = title;
    if (message) document.getElementById("nsToastMessage").textContent = message;
    toast.classList.add("active");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => toast.classList.remove("active"), 3000);
  }
  document.getElementById("nsToastClose")?.addEventListener("click", () => toast.classList.remove("active"));
  document.querySelectorAll(".ns-export").forEach((b) => b.addEventListener("click", () => showToast("Export Started", `Exporting news as ${b.dataset.format.toUpperCase()}...`)));
  document.getElementById("nsRefresh")?.addEventListener("click", () => showToast("Refreshing", "Loading latest news..."));
  document.querySelectorAll(".ns-bookmark").forEach((b) => b.addEventListener("click", (e) => { e.stopPropagation(); showToast("Bookmarked", "Article saved to your bookmarks"); }));

  /* 03. Tabs */
  document.querySelectorAll(".ns-tab").forEach((tab) => {
    tab.addEventListener("click", () => {
      document.querySelectorAll(".ns-tab").forEach((t) => t.classList.remove("active"));
      document.querySelectorAll(".tab-content").forEach((c) => c.classList.remove("active"));
      tab.classList.add("active");
      document.getElementById(tab.dataset.tab)?.classList.add("active");
      refreshIcons();
    });
  });

  /* 04. Sentiment chips */
  const cards = Array.from(document.querySelectorAll(".ns-card"));
  document.querySelectorAll(".ns-chip").forEach((chip) => {
    chip.addEventListener("click", () => {
      document.querySelectorAll(".ns-chip").forEach((c) => {
        c.classList.remove("active", "bg-accent/10", "border-accent", "text-accent");
        c.classList.add("text-muted");
      });
      chip.classList.add("active", "bg-accent/10", "border-accent", "text-accent");
      chip.classList.remove("text-muted");
      const f = chip.dataset.filter;
      cards.forEach((card) => {
        const d = card.dataset;
        const ok = f === "all" || d.sentiment === f || (f === "high-impact" && d.highimpact === "true") || (f === "breaking" && false);
        card.style.display = ok ? "" : "none";
      });
    });
  });

  /* 05. Drawers */
  const overlay = document.getElementById("nsOverlay");
  const newsDrawer = document.getElementById("nsNewsDrawer");
  const filterDrawer = document.getElementById("nsFilterDrawer");
  const catBadge = { sky: ["text-sky-500", "bg-sky-500/15"], orange: ["text-orange-500", "bg-orange-500/15"], violet: ["text-violet-400", "bg-violet-500/15"], amber: ["text-amber-500", "bg-amber-500/15"] };
  const sentMap = {
    bullish: { t: "text-emerald-500", b: "bg-emerald-500/15", box: "bg-emerald-500/10 border-emerald-500/20", bar: "bg-emerald-500", label: "Bullish" },
    bearish: { t: "text-red-500", b: "bg-red-500/15", box: "bg-red-500/10 border-red-500/20", bar: "bg-red-500", label: "Bearish" },
    neutral: { t: "text-slate-400", b: "bg-slate-500/15", box: "bg-slate-500/10 border-slate-500/20", bar: "bg-slate-400", label: "Neutral" },
  };
  function setText(id, v) { const el = document.getElementById(id); if (el) el.textContent = v; }

  function closeDrawers() {
    overlay?.classList.remove("active");
    newsDrawer?.classList.remove("active");
    filterDrawer?.classList.remove("active");
  }
  document.querySelectorAll(".ns-drawer-close").forEach((b) => b.addEventListener("click", closeDrawers));
  overlay?.addEventListener("click", closeDrawers);
  document.addEventListener("keydown", (e) => { if (e.key === "Escape") closeDrawers(); });
  document.getElementById("nsFilters")?.addEventListener("click", () => { overlay?.classList.add("active"); filterDrawer?.classList.add("active"); refreshIcons(); });
  document.querySelectorAll(".ns-toast-btn").forEach((b) =>
    b.addEventListener("click", () => { showToast(b.dataset.toastTitle, b.dataset.toastMsg); if (b.hasAttribute("data-close")) closeDrawers(); })
  );

  cards.forEach((card) => {
    card.addEventListener("click", (e) => {
      if (e.target.closest(".ns-bookmark")) return;
      const d = card.dataset;
      setText("nsDetailTitle", d.title);
      setText("nsDetailDesc", d.desc);
      setText("nsDetailSource", d.source);
      setText("nsDetailScore", `${d.score}%`);
      // category badge
      const cb = document.getElementById("nsDetailCatBadge");
      if (cb) { const [t, b] = catBadge[d.catbadge] || catBadge.sky; cb.className = `px-2 py-0.5 rounded-md text-xs font-semibold ${b} ${t}`; cb.textContent = d.cat.charAt(0).toUpperCase() + d.cat.slice(1); }
      // sentiment badge + score box + bar
      const sm = sentMap[d.sentiment] || sentMap.neutral;
      const sentBadge = document.getElementById("nsDetailSentBadge");
      if (sentBadge) sentBadge.className = `inline-flex items-center gap-1 px-2 py-0.5 rounded-md text-xs font-semibold ${sm.b} ${sm.t}`;
      setText("nsDetailSentText", sm.label);
      const box = document.getElementById("nsDetailScoreBox");
      if (box) box.className = `p-4 rounded-xl border ${sm.box}`;
      const score = document.getElementById("nsDetailScore");
      if (score) score.className = `text-2xl font-bold ${sm.t}`;
      const bar = document.getElementById("nsDetailScoreBar");
      if (bar) { bar.className = `h-full rounded-full ${sm.bar}`; bar.style.width = `${d.score}%`; }
      overlay?.classList.add("active");
      newsDrawer?.classList.add("active");
      refreshIcons();
    });
  });

  /* 06. Chart */
  let chart;
  function initChart() {
    if (typeof Chart === "undefined" || chart) return;
    const el = document.getElementById("nsSentimentChart");
    if (!el) return;
    chart = new Chart(el.getContext("2d"), {
      type: "line",
      data: {
        labels: ["00:00", "04:00", "08:00", "12:00", "16:00", "20:00", "Now"],
        datasets: [
          { label: "Bullish", data: [62, 68, 72, 75, 70, 68, 72], borderColor: "#10B981", backgroundColor: "rgba(16,185,129,0.1)", fill: true, tension: 0.4, borderWidth: 2, pointRadius: 0 },
          { label: "Bearish", data: [38, 32, 28, 25, 30, 32, 28], borderColor: "#EF4444", backgroundColor: "rgba(239,68,68,0.1)", fill: true, tension: 0.4, borderWidth: 2, pointRadius: 0 },
        ],
      },
      options: { responsive: true, maintainAspectRatio: false, interaction: { intersect: false, mode: "index" }, plugins: { legend: { position: "top", labels: { usePointStyle: true, padding: 16, color: tickColor(), font: { size: 11 } } } }, scales: { x: { grid: { display: false }, ticks: { color: tickColor(), font: { size: 10 } } }, y: { min: 0, max: 100, grid: { color: gridColor() }, ticks: { color: tickColor(), font: { size: 10 }, callback: (v) => v + "%" } } } },
    });
  }
  function recolor() {
    if (!chart) return;
    chart.options.plugins.legend.labels.color = tickColor();
    chart.options.scales.x.ticks.color = tickColor();
    chart.options.scales.y.ticks.color = tickColor();
    chart.options.scales.y.grid.color = gridColor();
    chart.update();
  }
  document.getElementById("themeToggle")?.addEventListener("click", () => setTimeout(recolor, 0));

  if (document.readyState === "complete") initChart();
  else window.addEventListener("load", initChart);
})();
