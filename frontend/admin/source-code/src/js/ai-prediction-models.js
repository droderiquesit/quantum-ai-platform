/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: ai-prediction-models.js
 * description: Self-contained controller for the AI Prediction Models page
 *              (#ai-prediction-models). DOM-only — no HTML is generated in JS;
 *              the drawer's three bodies are static templates in the HTML the JS
 *              only shows/hides and patches via textContent.
 * author: SignalAIX

    -------------------------------------------------
    Table of Contents
    -------------------------------------------------
    01. Init & guard
    02. Toast
    03. Category tabs + model-type filter chips (intersecting filters)
    04. Predictions table search
    05. Export buttons
    06. Drawer (model / view / new) + in-drawer pill groups
    07. Run-prediction buttons
    ================================================== */

(function () {
  /* ------------------------------------------------------------------ */
  /* 01. Init & guard                                                   */
  /* ------------------------------------------------------------------ */
  if (!document.getElementById("ai-prediction-models")) return;
  const refreshIcons = () => window.lucide && lucide.createIcons();

  /* ------------------------------------------------------------------ */
  /* 02. Toast                                                          */
  /* ------------------------------------------------------------------ */
  const toast = document.getElementById("apmToast");
  const toastTitle = document.getElementById("apmToastTitle");
  const toastMsg = document.getElementById("apmToastMessage");
  let toastTimer;
  function showToast(title, message) {
    if (!toast) return;
    if (title) toastTitle.textContent = title;
    if (message) toastMsg.textContent = message;
    toast.classList.add("active");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => toast.classList.remove("active"), 3000);
  }
  document.getElementById("apmToastClose")?.addEventListener("click", () => toast.classList.remove("active"));
  document.querySelectorAll(".apm-toast-btn").forEach((btn) => {
    btn.addEventListener("click", () => {
      showToast(btn.dataset.toastTitle, btn.dataset.toastMsg);
      if (btn.hasAttribute("data-close")) closeDrawer();
    });
  });

  /* ------------------------------------------------------------------ */
  /* 03. Category tabs + model-type chips (intersecting)                */
  /* ------------------------------------------------------------------ */
  const catTabs = document.querySelectorAll(".apm-cat");
  const modelChips = document.querySelectorAll(".apm-model");
  const cards = document.querySelectorAll(".apm-model-card");
  const modelsEmpty = document.querySelector(".apm-models-empty");
  let activeCat = "all";
  let activeModel = "all";

  function applyModelFilters() {
    let visible = 0;
    cards.forEach((card) => {
      const okCat = activeCat === "all" || card.dataset.category === activeCat;
      const okModel = activeModel === "all" || card.dataset.model === activeModel;
      const show = okCat && okModel;
      card.style.display = show ? "" : "none";
      if (show) visible++;
    });
    modelsEmpty?.classList.toggle("hidden", visible !== 0);
  }

  // category tabs use the shared .tab-button.active styling
  catTabs.forEach((tab) => {
    tab.addEventListener("click", () => {
      activeCat = tab.dataset.cat;
      catTabs.forEach((t) => t.classList.remove("active"));
      tab.classList.add("active");
      applyModelFilters();
    });
  });

  function setChipActive(btn, on) {
    btn.classList.toggle("active", on);
    btn.classList.toggle("bg-accent/10", on);
    btn.classList.toggle("border-accent", on);
    btn.classList.toggle("text-accent", on);
    btn.classList.toggle("text-muted", !on);
  }
  modelChips.forEach((chip) => {
    setChipActive(chip, chip.dataset.model === activeModel);
    chip.addEventListener("click", () => {
      activeModel = chip.dataset.model;
      modelChips.forEach((c) => setChipActive(c, c === chip));
      applyModelFilters();
    });
  });

  /* ------------------------------------------------------------------ */
  /* 04. Predictions table search                                       */
  /* ------------------------------------------------------------------ */
  const tableSearch = document.getElementById("apmTableSearch");
  const tableEmpty = document.querySelector(".apm-table-empty");
  tableSearch?.addEventListener("input", () => {
    const term = tableSearch.value.toLowerCase();
    let visible = 0;
    document.querySelectorAll("#apmPredictionsBody tr").forEach((row) => {
      const show = row.textContent.toLowerCase().includes(term);
      row.style.display = show ? "" : "none";
      if (show) visible++;
    });
    tableEmpty?.classList.toggle("hidden", visible !== 0);
  });

  /* ------------------------------------------------------------------ */
  /* 05. Export buttons                                                 */
  /* ------------------------------------------------------------------ */
  document.getElementById("apmExport")?.addEventListener("click", () => showToast("Export Started", "Generating prediction report…"));
  document.getElementById("apmTableExport")?.addEventListener("click", () => showToast("Export Started", "Downloading predictions as CSV…"));

  /* ------------------------------------------------------------------ */
  /* 06. Drawer                                                         */
  /* ------------------------------------------------------------------ */
  const drawer = document.getElementById("apmDrawer");
  const overlay = document.getElementById("apmDrawerOverlay");
  const drawerTitle = document.getElementById("apmDrawerTitle");
  const drawerSubtitle = document.getElementById("apmDrawerSubtitle");
  const panels = drawer ? drawer.querySelectorAll(".apm-panel") : [];

  const META = {
    model: { title: "Model Configuration", subtitle: "Configure model settings" },
    view: { title: "Prediction Details", subtitle: "Latest AI-generated prediction" },
    new: { title: "New Prediction", subtitle: "Generate AI-powered market prediction" },
  };

  function showPanel(name) {
    panels.forEach((p) => (p.hidden = p.dataset.panel !== name));
    if (drawerTitle) drawerTitle.textContent = META[name]?.title || "Details";
    if (drawerSubtitle) drawerSubtitle.textContent = META[name]?.subtitle || "";
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

  // Model config — patch name/pair/accuracy from a data map
  const MODELS = {
    "lstm-eurusd": { name: "LSTM-Pro v3.2", pair: "EUR/USD Prediction Model", acc: "91.2%" },
    "transformer-btc": { name: "Transformer-X", pair: "BTC/USD Prediction Model", acc: "88.7%" },
    "ensemble-gbpjpy": { name: "Ensemble-Alpha", pair: "GBP/JPY Prediction Model", acc: "94.5%" },
    "lstm-eth": { name: "LSTM-Crypto v2.1", pair: "ETH/USD Prediction Model", acc: "86.3%" },
    "transformer-gold": { name: "Gold-Predictor X", pair: "XAU/USD Prediction Model", acc: "89.8%" },
    "ensemble-spx": { name: "Index-Master Pro", pair: "S&P 500 Prediction Model", acc: "85.4%" },
  };

  function openModelDrawer(modelId) {
    const m = MODELS[modelId] || MODELS["lstm-eurusd"];
    const set = (id, val) => {
      const el = document.getElementById(id);
      if (el) el.textContent = val;
    };
    set("apmModelName", m.name);
    set("apmModelPair", m.pair);
    set("apmModelAcc", m.acc);
    if (drawerSubtitle) drawerSubtitle.textContent = `Model ID: ${modelId}`;
    openDrawer("model");
  }

  // Prediction details — patch the asset/model/values from the clicked row
  const PREDICTIONS = {
    "EUR/USD": { badge: "€/$", model: "LSTM-Pro v3.2", status: "Hit Target", statusCls: "bg-emerald-500/15 text-emerald-500", dir: "LONG", up: true, prob: "85%", entry: "1.0845", target: "1.0892" },
    "BTC/USD": { badge: "₿", model: "Transformer-X", status: "Active", statusCls: "bg-amber-500/15 text-amber-500", dir: "SHORT", up: false, prob: "72%", entry: "$96,450", target: "$94,250" },
    "GBP/JPY": { badge: "£/¥", model: "Ensemble-Alpha", status: "Hit Target", statusCls: "bg-emerald-500/15 text-emerald-500", dir: "LONG", up: true, prob: "91%", entry: "192.150", target: "192.850" },
    "XAU/USD": { badge: "XAU", model: "Gold-Predictor X", status: "Active", statusCls: "bg-amber-500/15 text-amber-500", dir: "LONG", up: true, prob: "88%", entry: "$2,648", target: "$2,685" },
    "ETH/USD": { badge: "ETH", model: "LSTM-Crypto v2.1", status: "Stopped", statusCls: "bg-red-500/15 text-red-500", dir: "SHORT", up: false, prob: "45%", entry: "$3,485", target: "$3,320" },
    "S&P 500": { badge: "SPX", model: "Index-Master Pro", status: "Active", statusCls: "bg-amber-500/15 text-amber-500", dir: "LONG", up: true, prob: "79%", entry: "6,045", target: "6,150" },
  };

  function openViewDrawer(id) {
    const p = PREDICTIONS[id] || PREDICTIONS["EUR/USD"];
    const set = (elId, val) => {
      const el = document.getElementById(elId);
      if (el) el.textContent = val;
    };
    set("apmViewBadge", p.badge);
    set("apmViewPair", id);
    set("apmViewModel", p.model);
    set("apmViewProb", p.prob);
    set("apmViewEntry", p.entry);
    set("apmViewTarget", p.target);
    const status = document.getElementById("apmViewStatus");
    if (status) {
      status.textContent = p.status;
      status.className = "inline-flex px-2.5 py-1 rounded-full text-xs font-semibold " + p.statusCls;
    }
    const color = p.up ? "text-emerald-500" : "text-red-500";
    const dirIcon = document.getElementById("apmViewDirIcon");
    if (dirIcon) {
      dirIcon.setAttribute("data-lucide", p.up ? "trending-up" : "trending-down");
      dirIcon.className = "w-5 h-5 " + color;
    }
    const dirText = document.getElementById("apmViewDirText");
    if (dirText) {
      dirText.textContent = p.dir;
      dirText.className = "font-bold " + color;
    }
    // recolor the Target value to match direction
    const tgt = document.getElementById("apmViewTarget");
    if (tgt) tgt.className = "font-bold font-mono " + (p.up ? "text-emerald-500" : "text-red-500");
    openDrawer("view");
  }

  document.querySelectorAll(".apm-open-drawer").forEach((btn) => {
    btn.addEventListener("click", () => {
      const d = btn.dataset.drawer;
      if (d === "model") openModelDrawer(btn.dataset.modelId);
      else openDrawer(d);
    });
  });
  document.querySelectorAll(".apm-view").forEach((btn) => {
    btn.addEventListener("click", () => openViewDrawer(btn.dataset.id));
  });
  drawer?.querySelectorAll(".apm-drawer-close").forEach((b) => b.addEventListener("click", closeDrawer));
  overlay?.addEventListener("click", closeDrawer);
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") closeDrawer();
  });

  // In-drawer pill groups (risk / timeframe / horizon)
  function wirePillGroup(selector) {
    document.querySelectorAll(selector).forEach((pill) => {
      pill.addEventListener("click", () => {
        pill.parentElement.querySelectorAll(selector).forEach((p) => {
          p.classList.remove("active", "bg-accent/10", "border-accent", "text-accent");
          p.classList.add("text-muted");
        });
        pill.classList.add("active", "bg-accent/10", "border-accent", "text-accent");
        pill.classList.remove("text-muted");
      });
    });
    // initialise preselected
    document.querySelectorAll(`${selector}.active`).forEach((p) => {
      p.classList.add("bg-accent/10", "border-accent", "text-accent");
      p.classList.remove("text-muted");
    });
  }
  wirePillGroup(".apm-risk");
  wirePillGroup(".apm-tf");
  wirePillGroup(".apm-hz");

  /* ------------------------------------------------------------------ */
  /* 07. Run-prediction buttons                                         */
  /* ------------------------------------------------------------------ */
  document.querySelectorAll(".apm-run").forEach((btn) => {
    btn.addEventListener("click", () => {
      const m = MODELS[btn.dataset.modelId];
      showToast("Prediction Started", `Running ${m ? m.name : "model"}…`);
    });
  });
})();
