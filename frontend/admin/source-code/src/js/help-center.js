/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: help-center.js
 * description: Self-contained controller for the Help Center page
 *              (#help-center). DOM-only — all markup is static in the HTML;
 *              JS only toggles classes / .hidden, updates textContent, and
 *              patches the view-ticket drawer from the clicked ticket's data-*.
 *              No HTML is generated in JS.
 * author: SignalAIX

    -------------------------------------------------
    Table of Contents
    -------------------------------------------------
    01. Init & guard
    02. Toast
    03. Tabs (Knowledge / FAQ / Tutorials / Tickets / Contact)
    04. FAQ accordion + category chip filter
    05. Tutorials level chip filter
    06. Tickets status chip filter + search
    07. Header filter dropdown / export / hero search / topics
    08. Drawers (new ticket, view ticket, live chat, email)
    09. Generic toast buttons + category/article/video toasts
    ================================================== */

(function () {
  if (!document.getElementById("help-center")) return;

  const refreshIcons = () => window.lucide && lucide.createIcons();

  /* 02. Toast */
  const toast = document.getElementById("hcToast");
  let toastTimer;
  function showToast(title, message) {
    if (!toast) return;
    if (title) document.getElementById("hcToastTitle").textContent = title;
    if (message) document.getElementById("hcToastMessage").textContent = message;
    toast.classList.add("active");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => toast.classList.remove("active"), 3000);
  }
  document.getElementById("hcToastClose")?.addEventListener("click", () => toast.classList.remove("active"));

  /* 03. Tabs */
  document.querySelectorAll(".hc-tab").forEach((tab) => {
    tab.addEventListener("click", () => {
      document.querySelectorAll(".hc-tab").forEach((t) => t.classList.remove("active"));
      document.querySelectorAll(".hc-pane").forEach((p) => p.classList.add("hidden"));
      tab.classList.add("active");
      document.getElementById("hc-" + tab.dataset.tab)?.classList.remove("hidden");
      refreshIcons();
    });
  });

  /* Chip active-style helpers (no common.css for these — toggle Tailwind classes) */
  const CHIP_ON = ["bg-accent/10", "border-accent", "text-accent"];
  function setChipActive(group, btn) {
    group.forEach((c) => {
      c.classList.remove("active", ...CHIP_ON);
      c.classList.add("text-muted");
    });
    btn.classList.add("active", ...CHIP_ON);
    btn.classList.remove("text-muted");
  }

  /* 04. FAQ accordion (single-open) + category filter */
  document.querySelectorAll(".hc-faq").forEach((item) => {
    const q = item.querySelector(".hc-faq-q");
    const a = item.querySelector(".hc-faq-a");
    const icon = item.querySelector(".hc-faq-icon");
    q?.addEventListener("click", () => {
      const willOpen = a.classList.contains("hidden");
      document.querySelectorAll(".hc-faq").forEach((other) => {
        other.querySelector(".hc-faq-a")?.classList.add("hidden");
        other.querySelector(".hc-faq-icon")?.classList.remove("-rotate-180");
        other.classList.remove("border-accent/40");
      });
      if (willOpen) {
        a.classList.remove("hidden");
        icon?.classList.add("-rotate-180");
        item.classList.add("border-accent/40");
      }
    });
  });

  const faqChips = Array.from(document.querySelectorAll(".hc-faq-chip"));
  faqChips.forEach((chip) => {
    chip.addEventListener("click", () => {
      setChipActive(faqChips, chip);
      const f = chip.dataset.filter;
      document.querySelectorAll(".hc-faq").forEach((item) => {
        item.style.display = f === "all" || item.dataset.category === f ? "" : "none";
      });
    });
  });

  /* 05. Tutorials level filter */
  const vidChips = Array.from(document.querySelectorAll(".hc-vid-chip"));
  vidChips.forEach((chip) => {
    chip.addEventListener("click", () => {
      setChipActive(vidChips, chip);
      const f = chip.dataset.filter;
      document.querySelectorAll(".hc-video").forEach((v) => {
        v.style.display = f === "all" || v.dataset.level === f ? "" : "none";
      });
    });
  });

  /* 06. Tickets status filter + search */
  const tickets = Array.from(document.querySelectorAll(".hc-ticket"));
  const ticketChips = Array.from(document.querySelectorAll(".hc-ticket-chip"));
  let ticketStatus = "all";
  const ticketSearch = document.getElementById("hcTicketSearch");
  function applyTicketFilters() {
    const q = (ticketSearch?.value || "").toLowerCase();
    tickets.forEach((t) => {
      const okStatus = ticketStatus === "all" || t.dataset.status === ticketStatus;
      const okSearch = !q || t.textContent.toLowerCase().includes(q);
      t.style.display = okStatus && okSearch ? "" : "none";
    });
  }
  ticketChips.forEach((chip) => {
    chip.addEventListener("click", () => {
      setChipActive(ticketChips, chip);
      ticketStatus = chip.dataset.filter;
      applyTicketFilters();
    });
  });
  ticketSearch?.addEventListener("input", applyTicketFilters);

  /* 07. Header filter dropdown / export / hero search / topics */
  document.querySelectorAll(".hc-filter").forEach((b) => {
    b.addEventListener("click", () => {
      const f = b.dataset.filter;
      showToast("Filter Applied", `Showing ${f === "all" ? "all categories" : f} articles`);
    });
  });
  document.getElementById("hcExport")?.addEventListener("click", () => showToast("Export Started", "Your help center data is being exported..."));

  const heroSearch = document.getElementById("hcHeroSearch");
  function runSearch(topic) {
    if (topic && heroSearch) heroSearch.value = topic;
    const q = (topic || heroSearch?.value || "").trim();
    if (q) showToast("Searching", `Finding articles about "${q}"...`);
  }
  document.getElementById("hcHeroSearchBtn")?.addEventListener("click", () => runSearch());
  heroSearch?.addEventListener("keydown", (e) => { if (e.key === "Enter") runSearch(); });
  document.querySelectorAll(".hc-topic").forEach((b) => b.addEventListener("click", () => runSearch(b.dataset.topic)));

  /* Category cards / article cards -> informational toasts */
  document.querySelectorAll(".hc-category").forEach((c) => c.addEventListener("click", () => showToast("Loading", `Opening ${c.dataset.category.replace(/-/g, " ")} articles...`)));
  document.querySelectorAll(".hc-article").forEach((c) => c.addEventListener("click", () => showToast("Loading", "Opening article...")));

  /* Video cards -> open the player drawer with a real <video> player.
     Each card carries data-video; the source is mapped here (sample clips —
     swap for real tutorial URLs in production). Title / description / views /
     duration are read from the card's own markup so the drawer stays in sync
     with the card — no markup generated in JS. */
  const hcVideoEl = document.getElementById("hcVideoFrame");
  const SAMPLE = "https://storage.googleapis.com/gtv-videos-bucket/sample/";
  const VIDEO_SRC = {
    intro: SAMPLE + "BigBuckBunny.mp4",
    "signals-setup": SAMPLE + "ElephantsDream.mp4",
    "ai-tools": SAMPLE + "ForBiggerBlazes.mp4",
    "webhook-setup": SAMPLE + "ForBiggerEscapes.mp4",
    "api-integration": SAMPLE + "ForBiggerFun.mp4",
    "bot-trading": SAMPLE + "ForBiggerJoyrides.mp4",
  };
  const vTitle = document.getElementById("hcVideoTitle");
  const vMeta = document.getElementById("hcVideoMeta");
  const vDesc = document.getElementById("hcVideoDesc");
  const vViews = document.getElementById("hcVideoViews");
  const vDuration = document.getElementById("hcVideoDuration");
  document.querySelectorAll(".hc-video").forEach((card) => {
    card.addEventListener("click", () => {
      const key = card.dataset.video;
      const title = card.querySelector("h4")?.textContent?.trim() || "Tutorial";
      const desc = card.querySelector("p")?.textContent?.trim() || "";
      const metaSpans = card.querySelectorAll(".text-xs span");
      const views = metaSpans[0]?.textContent?.trim() || "";
      const durationBadge = card.querySelector(".absolute")?.textContent?.trim() || "";
      const level = (card.dataset.level || "").replace(/^\w/, (c) => c.toUpperCase());
      if (vTitle) vTitle.textContent = title;
      if (vMeta) vMeta.textContent = [level, durationBadge].filter(Boolean).join(" · ");
      if (vDesc) vDesc.textContent = desc;
      if (vViews) vViews.textContent = views;
      if (vDuration) vDuration.textContent = durationBadge;
      const src = VIDEO_SRC[key] || VIDEO_SRC.intro;
      if (hcVideoEl) {
        hcVideoEl.src = src;
        hcVideoEl.load();
      }
      openDrawer("video");
      // Start playback once the drawer is visible.
      hcVideoEl?.play?.().catch(() => {});
    });
  });

  /* 08. Drawers */
  const overlay = document.getElementById("hcDrawerOverlay");
  const drawers = {
    newTicket: document.getElementById("hcNewTicketDrawer"),
    viewTicket: document.getElementById("hcViewTicketDrawer"),
    liveChat: document.getElementById("hcLiveChatDrawer"),
    email: document.getElementById("hcEmailDrawer"),
    video: document.getElementById("hcVideoDrawer"),
  };
  const videoFrame = document.getElementById("hcVideoFrame");
  function closeDrawers() {
    Object.values(drawers).forEach((d) => d?.classList.remove("active"));
    overlay?.classList.remove("active");
    // Stop video playback when any drawer closes.
    if (videoFrame) {
      videoFrame.pause?.();
      videoFrame.removeAttribute("src");
      videoFrame.load?.();
    }
  }
  function openDrawer(name) {
    closeDrawers();
    drawers[name]?.classList.add("active");
    overlay?.classList.add("active");
    refreshIcons();
  }
  overlay?.addEventListener("click", closeDrawers);
  document.querySelectorAll(".hc-drawer-close").forEach((b) => b.addEventListener("click", closeDrawers));
  document.addEventListener("keydown", (e) => { if (e.key === "Escape") closeDrawers(); });

  document.getElementById("hcNewTicket")?.addEventListener("click", () => openDrawer("newTicket"));
  document.getElementById("hcLiveChat")?.addEventListener("click", () => openDrawer("liveChat"));
  document.getElementById("hcEmailSupport")?.addEventListener("click", () => openDrawer("email"));

  /* View-ticket drawer: patch from clicked ticket's data-* (DOM updates only) */
  const STATUS_STYLE = {
    open: { badge: ["bg-emerald-500/15", "text-emerald-500"], text: "text-emerald-500", label: "Open" },
    pending: { badge: ["bg-amber-500/15", "text-amber-500"], text: "text-amber-500", label: "Pending" },
    closed: { badge: ["bg-slate-500/15", "text-slate-400"], text: "text-slate-400", label: "Closed" },
  };
  const PRIORITY_COLOR = { Low: "text-emerald-500", Medium: "text-amber-500", High: "text-red-500" };
  const vtBadge = document.getElementById("hcVtStatusBadge");
  function setText(id, v) { const el = document.getElementById(id); if (el) el.textContent = v; }
  tickets.forEach((t) => {
    t.addEventListener("click", () => {
      const d = t.dataset;
      const s = STATUS_STYLE[d.status] || STATUS_STYLE.open;
      setText("hcVtId", d.ticket);
      setText("hcVtSubject", d.subject);
      setText("hcVtStatus", s.label);
      setText("hcVtPriority", d.priority);
      setText("hcVtCategory", d.category);
      if (vtBadge) {
        vtBadge.className = "px-2 py-0.5 rounded-md text-[11px] font-semibold shrink-0 " + s.badge.join(" ");
        vtBadge.textContent = s.label;
      }
      const statusEl = document.getElementById("hcVtStatus");
      if (statusEl) statusEl.className = "font-semibold " + s.text;
      const prioEl = document.getElementById("hcVtPriority");
      if (prioEl) prioEl.className = "font-semibold " + (PRIORITY_COLOR[d.priority] || "text-text");
      openDrawer("viewTicket");
    });
  });

  /* Drawer actions */
  document.querySelector(".hc-submit-ticket")?.addEventListener("click", () => { closeDrawers(); showToast("Ticket Created", "Your support ticket has been submitted successfully."); });
  document.querySelector(".hc-send-reply")?.addEventListener("click", () => showToast("Reply Sent", "Your message has been sent to the support team."));
  document.querySelector(".hc-send-email")?.addEventListener("click", () => { closeDrawers(); showToast("Email Sent", "Your email has been sent. We'll respond within 24 hours."); });
  document.querySelector(".hc-send-chat")?.addEventListener("click", () => {
    const input = document.getElementById("hcChatInput");
    if (input && input.value.trim()) { showToast("Message Sent", "Your message has been delivered."); input.value = ""; }
  });
  document.getElementById("hcChatInput")?.addEventListener("keydown", (e) => {
    if (e.key === "Enter") document.querySelector(".hc-send-chat")?.click();
  });

  /* 09. Generic toast buttons (data-toast-title / data-toast-msg, optional data-close) */
  document.querySelectorAll(".hc-toast-btn").forEach((b) => {
    b.addEventListener("click", () => {
      if (b.hasAttribute("data-close")) closeDrawers();
      showToast(b.dataset.toastTitle, b.dataset.toastMsg);
    });
  });
})();
