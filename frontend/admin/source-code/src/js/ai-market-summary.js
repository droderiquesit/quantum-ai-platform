/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: ai-market-summary.js
 * description: Self-contained controller for the AI Market Summary page
 *              (#ai-market-summary). DOM-only — no HTML is generated in JS;
 *              all markup (incl. drawer panels) lives in ai-market-summary.html
 *              as static templates the JS only shows/hides or updates.
 * author: SignalAIX

    -------------------------------------------------
    Table of Contents
    -------------------------------------------------
    01. Init & guard
    02. Toast
    03. Tabs (Overview / Forex / Crypto / Sentiment / Predictions)
    04. Timeframe + chart-period chips
    05. Export menu
    06. Top-signals search
    07. Top Movers gainers/losers toggle
    08. Refresh
    09. Drawer (report / sentiment / all-signals / signal-detail)
    10. Charts (init on demand per tab + theme re-color)
    ================================================== */

(function () {
  /* ------------------------------------------------------------------ */
  /* 01. Init & guard                                                   */
  /* ------------------------------------------------------------------ */
  if (!document.getElementById("ai-market-summary")) return;

  const html = document.documentElement;
  const isDark = () => html.classList.contains("dark");
  const tickColor = () => (isDark() ? "#94A3B8" : "#64748B");
  const gridColor = () => (isDark() ? "rgba(255,255,255,0.06)" : "rgba(0,0,0,0.06)");
  const refreshIcons = () => window.lucide && lucide.createIcons();

  /* ------------------------------------------------------------------ */
  /* 02. Toast                                                          */
  /* ------------------------------------------------------------------ */
  const toast = document.getElementById("amsToast");
  const toastTitle = document.getElementById("amsToastTitle");
  const toastMsg = document.getElementById("amsToastMessage");
  let toastTimer;

  function showToast(title, message) {
    if (!toast) return;
    if (title) toastTitle.textContent = title;
    if (message) toastMsg.textContent = message;
    toast.classList.add("active");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => toast.classList.remove("active"), 3000);
  }
  document.getElementById("amsToastClose")?.addEventListener("click", () => toast.classList.remove("active"));

  document.querySelectorAll(".ams-toast-btn").forEach((btn) => {
    btn.addEventListener("click", () => showToast(btn.dataset.toastTitle, btn.dataset.toastMsg));
  });

  /* ------------------------------------------------------------------ */
  /* 03. Tabs                                                           */
  /* ------------------------------------------------------------------ */
  document.querySelectorAll(".ams-tab").forEach((tab) => {
    tab.addEventListener("click", () => {
      const id = tab.dataset.tab;
      document.querySelectorAll(".ams-tab").forEach((t) => t.classList.remove("active"));
      tab.classList.add("active");
      document.querySelectorAll(".ams-pane").forEach((p) => p.classList.remove("active"));
      document.getElementById(`ams-tab-${id}`)?.classList.add("active");
      initTabCharts(id);
      refreshIcons();
    });
  });

  /* ------------------------------------------------------------------ */
  /* 04. Timeframe + chart-period chips                                 */
  /* ------------------------------------------------------------------ */
  document.getElementById("amsTimeframe")?.addEventListener("change", (e) => {
    showToast("Filter Applied", `Showing data for the ${e.target.options[e.target.selectedIndex].text} timeframe`);
  });

  function setChipActive(btn, on) {
    btn.classList.toggle("active", on);
    btn.classList.toggle("bg-accent/10", on);
    btn.classList.toggle("border-accent", on);
    btn.classList.toggle("text-accent", on);
    btn.classList.toggle("text-muted", !on);
  }
  const periodChips = document.querySelectorAll(".ams-period");
  periodChips.forEach((btn) => {
    setChipActive(btn, btn.classList.contains("active"));
    btn.addEventListener("click", () => periodChips.forEach((b) => setChipActive(b, b === btn)));
  });

  /* ------------------------------------------------------------------ */
  /* 05. Export menu                                                    */
  /* ------------------------------------------------------------------ */
  document.querySelectorAll(".ams-export").forEach((item) => {
    item.addEventListener("click", () => {
      const fmt = (item.dataset.format || "").toUpperCase();
      item.closest(".dropdown-menu")?.classList.remove("active");
      showToast("Export Started", `Exporting data as ${fmt}…`);
      setTimeout(() => showToast("Export Complete", `Your ${fmt} file is ready for download`), 1500);
    });
  });

  /* ------------------------------------------------------------------ */
  /* 06. Top-signals search                                             */
  /* ------------------------------------------------------------------ */
  const signalSearch = document.getElementById("amsSignalSearch");
  const signalsEmpty = document.querySelector(".ams-signals-empty");
  signalSearch?.addEventListener("input", () => {
    const term = signalSearch.value.toLowerCase();
    let visible = 0;
    document.querySelectorAll("#amsSignalsBody tr").forEach((row) => {
      const show = row.textContent.toLowerCase().includes(term);
      row.style.display = show ? "" : "none";
      if (show) visible++;
    });
    signalsEmpty?.classList.toggle("hidden", visible !== 0);
  });

  /* ------------------------------------------------------------------ */
  /* 07. Top Movers gainers/losers toggle                               */
  /* ------------------------------------------------------------------ */
  const moversTabs = document.querySelectorAll(".ams-movers-tab");
  function setMoversActive(btn, on) {
    btn.classList.toggle("active", on);
    btn.classList.toggle("bg-accent", on);
    btn.classList.toggle("text-white", on);
    btn.classList.toggle("text-muted", !on);
  }
  moversTabs.forEach((btn) => {
    setMoversActive(btn, btn.classList.contains("active"));
    btn.addEventListener("click", () => {
      const which = btn.dataset.movers;
      moversTabs.forEach((b) => setMoversActive(b, b === btn));
      document.querySelectorAll(".ams-movers-pane").forEach((p) => p.classList.toggle("hidden", p.dataset.movers !== which));
    });
  });

  /* ------------------------------------------------------------------ */
  /* 08. Refresh                                                        */
  /* ------------------------------------------------------------------ */
  document.getElementById("amsRefresh")?.addEventListener("click", () => {
    showToast("Refreshing", "Updating AI analysis…");
    setTimeout(() => showToast("Updated", "AI analysis refreshed successfully"), 1500);
  });

  /* ------------------------------------------------------------------ */
  /* 09. Drawer                                                         */
  /* ------------------------------------------------------------------ */
  const drawer = document.getElementById("amsDrawer");
  const overlay = document.getElementById("amsDrawerOverlay");
  const drawerTitle = document.getElementById("amsDrawerTitle");
  const panels = drawer ? drawer.querySelectorAll(".ams-panel") : [];

  const TITLES = {
    "ai-report": "AI Full Report",
    "sentiment-details": "Sentiment Details",
    "all-signals": "All Trading Signals",
    "signal-detail": "Signal Details",
  };

  function showPanel(name) {
    panels.forEach((p) => (p.hidden = p.dataset.panel !== name));
    if (drawerTitle) drawerTitle.textContent = TITLES[name] || "Details";
  }
  function openDrawer(name) {
    showPanel(name);
    overlay?.classList.add("active");
    drawer?.classList.add("active");
    refreshIcons();
  }
  function closeDrawer() {
    overlay?.classList.remove("active");
    drawer?.classList.remove("active");
  }

  document.querySelectorAll(".ams-open-drawer").forEach((btn) => {
    btn.addEventListener("click", () => openDrawer(btn.dataset.drawer));
  });
  drawer?.querySelectorAll(".ams-drawer-close").forEach((b) => b.addEventListener("click", closeDrawer));
  overlay?.addEventListener("click", closeDrawer);
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") closeDrawer();
  });

  // Signal-detail (opened from table eye icon or an "all signals" card) — DOM updates only
  const SIGNALS = {
    "EUR/USD": { conf: "94%", entry: "1.0885", target: "1.0950", stop: "1.0850", analysis: "Strong bullish momentum with price breaking above key resistance at 1.0880. RSI shows bullish divergence on the 4H timeframe; MACD crossover confirms the uptrend. Risk/Reward ratio: 1:1.8" },
    "BTC/USD": { conf: "91%", entry: "67,200", target: "72,000", stop: "65,500", analysis: "Accumulation phase detected near the $67,000 support zone with rising on-chain volume. Momentum favours continuation toward the $72,000 target." },
    "GBP/JPY": { conf: "87%", entry: "190.45", target: "188.80", stop: "191.20", analysis: "Head and Shoulders pattern forming on the 4H chart. Bearish reversal probability 82%; a break below the neckline targets 188.80." },
    "ETH/USD": { conf: "89%", entry: "3,380", target: "3,650", stop: "3,280", analysis: "Higher-low structure intact with strengthening relative momentum versus BTC. Target 3,650 on continuation." },
    "USD/CHF": { conf: "85%", entry: "0.8920", target: "0.8850", stop: "0.8960", analysis: "Rejection at descending resistance; bias favours a move toward 0.8850 support." },
    "XAU/USD": { conf: "88%", entry: "2,340", target: "2,400", stop: "2,310", analysis: "Safe-haven demand and a weaker dollar support continuation toward 2,400." },
  };

  function openSignalDetail(pair) {
    const s = SIGNALS[pair] || SIGNALS["EUR/USD"];
    const set = (id, val) => {
      const el = document.getElementById(id);
      if (el) el.textContent = val;
    };
    set("amsSdPair", pair);
    set("amsSdConfidence", `${s.conf} Confidence`);
    set("amsSdEntry", s.entry);
    set("amsSdTarget", s.target);
    set("amsSdStop", s.stop);
    set("amsSdAnalysis", s.analysis);
    openDrawer("signal-detail");
  }

  document.querySelectorAll(".ams-signal-detail").forEach((btn) => {
    btn.addEventListener("click", () => openSignalDetail(btn.dataset.pair));
  });
  document.querySelectorAll(".ams-all-signal").forEach((card) => {
    card.addEventListener("click", () => openSignalDetail(card.dataset.pair));
  });

  // Search within the all-signals drawer
  const allSearch = document.getElementById("amsAllSignalsSearch");
  const allEmpty = document.querySelector(".ams-all-empty");
  allSearch?.addEventListener("input", () => {
    const term = allSearch.value.toLowerCase();
    let visible = 0;
    document.querySelectorAll(".ams-all-signal").forEach((card) => {
      const show = (card.dataset.pair || "").toLowerCase().includes(term);
      card.style.display = show ? "" : "none";
      if (show) visible++;
    });
    allEmpty?.classList.toggle("hidden", visible !== 0);
  });

  /* ------------------------------------------------------------------ */
  /* 10. Charts                                                         */
  /* ------------------------------------------------------------------ */
  const charts = {};

  function baseScales(extra) {
    return Object.assign(
      {
        y: { grid: { color: gridColor() }, ticks: { color: tickColor() } },
        x: { grid: { display: false }, ticks: { color: tickColor() } },
      },
      extra || {}
    );
  }

  function initOverviewCharts() {
    if (typeof Chart === "undefined") return;

    const trend = document.getElementById("amsTrendChart");
    if (trend && !charts.trend) {
      charts.trend = new Chart(trend.getContext("2d"), {
        type: "line",
        data: {
          labels: ["00:00", "04:00", "08:00", "12:00", "16:00", "20:00", "24:00"],
          datasets: [
            { label: "Forex Index", data: [100, 102, 101, 105, 108, 107, 110], borderColor: "#10b981", backgroundColor: "rgba(16,185,129,0.1)", fill: true, tension: 0.4, borderWidth: 2, pointRadius: 0 },
            { label: "Crypto Index", data: [100, 98, 103, 99, 106, 112, 115], borderColor: "#6366f1", backgroundColor: "rgba(99,102,241,0.1)", fill: true, tension: 0.4, borderWidth: 2, pointRadius: 0 },
          ],
        },
        options: { responsive: true, maintainAspectRatio: false, interaction: { intersect: false, mode: "index" }, plugins: { legend: { position: "top", align: "end", labels: { color: tickColor(), usePointStyle: true, pointStyle: "circle", padding: 16 } } }, scales: baseScales() },
      });
    }

    const sentiment = document.getElementById("amsSentimentChart");
    if (sentiment && !charts.sentiment) {
      charts.sentiment = new Chart(sentiment.getContext("2d"), {
        type: "doughnut",
        data: { labels: ["Bullish", "Bearish", "Neutral"], datasets: [{ data: [68, 22, 10], backgroundColor: ["#10b981", "#ef4444", "#64748b"], borderWidth: 0, hoverOffset: 10 }] },
        options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { position: "right", labels: { color: tickColor(), usePointStyle: true, pointStyle: "circle", padding: 16 } } }, cutout: "70%" },
      });
    }

    const perf = document.getElementById("amsPerformanceChart");
    if (perf && !charts.perf) {
      charts.perf = new Chart(perf.getContext("2d"), {
        type: "bar",
        data: { labels: ["Forex", "Crypto", "Commodities", "Indices"], datasets: [{ label: "Accuracy %", data: [87, 82, 79, 84], backgroundColor: ["rgba(16,185,129,0.85)", "rgba(99,102,241,0.85)", "rgba(245,158,11,0.85)", "rgba(14,165,233,0.85)"], borderRadius: 8 }] },
        options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { display: false } }, scales: baseScales({ y: { beginAtZero: true, max: 100, grid: { color: gridColor() }, ticks: { color: tickColor(), callback: (v) => v + "%" } } }) },
      });
    }
  }

  function initTabCharts(tabId) {
    if (typeof Chart === "undefined") return;

    if (tabId === "forex") {
      const forex = document.getElementById("amsForexChart");
      if (forex && !charts.forex) {
        charts.forex = new Chart(forex.getContext("2d"), {
          type: "line",
          data: {
            labels: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
            datasets: [
              { label: "EUR/USD", data: [1.085, 1.087, 1.089, 1.086, 1.088, 1.09, 1.089], borderColor: "#10b981", borderWidth: 2, tension: 0.4, pointRadius: 0 },
              { label: "GBP/USD", data: [1.268, 1.27, 1.269, 1.272, 1.271, 1.273, 1.271], borderColor: "#6366f1", borderWidth: 2, tension: 0.4, pointRadius: 0 },
              { label: "USD/JPY", data: [149.2, 149.5, 149.3, 149.8, 149.6, 149.9, 149.8], borderColor: "#f59e0b", borderWidth: 2, tension: 0.4, pointRadius: 0, yAxisID: "y1" },
            ],
          },
          options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { position: "top", labels: { color: tickColor() } } }, scales: baseScales({ y1: { position: "right", grid: { display: false }, ticks: { color: tickColor() } } }) },
        });
      }
    }

    if (tabId === "crypto") {
      const crypto = document.getElementById("amsCryptoChart");
      if (crypto && !charts.crypto) {
        charts.crypto = new Chart(crypto.getContext("2d"), {
          type: "line",
          data: {
            labels: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"],
            datasets: [
              { label: "BTC", data: [65000, 66200, 65800, 67000, 67500, 67200, 67480], borderColor: "#f59e0b", borderWidth: 2, tension: 0.4, pointRadius: 0 },
              { label: "ETH", data: [3200, 3280, 3250, 3350, 3400, 3380, 3421], borderColor: "#6366f1", borderWidth: 2, tension: 0.4, pointRadius: 0, yAxisID: "y1" },
            ],
          },
          options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { position: "top", labels: { color: tickColor() } } }, scales: baseScales({ y1: { position: "right", grid: { display: false }, ticks: { color: tickColor() } } }) },
        });
      }
      const dom = document.getElementById("amsDominanceChart");
      if (dom && !charts.dom) {
        charts.dom = new Chart(dom.getContext("2d"), {
          type: "doughnut",
          data: { labels: ["Bitcoin", "Ethereum", "Others"], datasets: [{ data: [52, 18, 30], backgroundColor: ["#f59e0b", "#6366f1", "#64748b"], borderWidth: 0 }] },
          options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { position: "bottom", labels: { color: tickColor() } } }, cutout: "60%" },
        });
      }
    }

    if (tabId === "sentiment") {
      const social = document.getElementById("amsSocialChart");
      if (social && !charts.social) {
        charts.social = new Chart(social.getContext("2d"), {
          type: "radar",
          data: { labels: ["Twitter", "Reddit", "News", "Forums", "Telegram"], datasets: [{ label: "Bullish Score", data: [78, 65, 72, 58, 82], backgroundColor: "rgba(16,185,129,0.2)", borderColor: "#10b981", borderWidth: 2, pointBackgroundColor: "#10b981" }] },
          options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { display: false } }, scales: { r: { beginAtZero: true, max: 100, grid: { color: gridColor() }, angleLines: { color: gridColor() }, pointLabels: { color: tickColor() }, ticks: { display: false } } } },
        });
      }
    }

    if (tabId === "predictions") {
      const pred = document.getElementById("amsPredictionChart");
      if (pred && !charts.pred) {
        charts.pred = new Chart(pred.getContext("2d"), {
          type: "line",
          data: {
            labels: ["Now", "+1H", "+4H", "+1D", "+3D", "+1W"],
            datasets: [
              { label: "BTC Prediction", data: [67480, 67800, 68200, 69500, 71000, 72500], borderColor: "#f59e0b", borderWidth: 2, tension: 0.4, borderDash: [5, 5], pointRadius: 0 },
              { label: "ETH Prediction", data: [3421, 3450, 3500, 3580, 3620, 3700], borderColor: "#6366f1", borderWidth: 2, tension: 0.4, borderDash: [5, 5], pointRadius: 0, yAxisID: "y1" },
            ],
          },
          options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { position: "top", labels: { color: tickColor() } } }, scales: baseScales({ y1: { position: "right", grid: { display: false }, ticks: { color: tickColor() } } }) },
        });
      }
      const acc = document.getElementById("amsAccuracyChart");
      if (acc && !charts.acc) {
        charts.acc = new Chart(acc.getContext("2d"), {
          type: "bar",
          data: { labels: ["Week 1", "Week 2", "Week 3", "Week 4"], datasets: [{ label: "Correct", data: [82, 85, 78, 88], backgroundColor: "rgba(16,185,129,0.85)", borderRadius: 6 }, { label: "Incorrect", data: [18, 15, 22, 12], backgroundColor: "rgba(239,68,68,0.85)", borderRadius: 6 }] },
          options: { responsive: true, maintainAspectRatio: false, plugins: { legend: { position: "top", labels: { color: tickColor() } } }, scales: { y: { stacked: true, grid: { color: gridColor() }, ticks: { color: tickColor() } }, x: { stacked: true, grid: { display: false }, ticks: { color: tickColor() } } } },
        });
      }
    }
  }

  function recolorCharts() {
    Object.values(charts).forEach((c) => {
      if (!c) return;
      const s = c.options.scales || {};
      if (s.y) { if (s.y.grid) s.y.grid.color = gridColor(); if (s.y.ticks) s.y.ticks.color = tickColor(); }
      if (s.y1 && s.y1.ticks) s.y1.ticks.color = tickColor();
      if (s.x && s.x.ticks) s.x.ticks.color = tickColor();
      if (s.r) { if (s.r.grid) s.r.grid.color = gridColor(); if (s.r.angleLines) s.r.angleLines.color = gridColor(); if (s.r.pointLabels) s.r.pointLabels.color = tickColor(); }
      if (c.options.plugins?.legend?.labels) c.options.plugins.legend.labels.color = tickColor();
      c.update();
    });
  }
  document.getElementById("themeToggle")?.addEventListener("click", () => setTimeout(recolorCharts, 0));

  // Charts depend on the CDN Chart.js (deferred) — init after load.
  if (document.readyState === "complete") initOverviewCharts();
  else window.addEventListener("load", initOverviewCharts);
})();
