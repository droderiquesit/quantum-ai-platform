/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: notification-settings.html
 * description: SignalAIX - Notification Settings Page Controller
 *              Self-contained; mirrors the reference mockup's functionality
 *              (6 tabs: Delivery Channels / Preferences / Alert Types /
 *               Schedule / Sounds / History; master enable that flips every
 *               channel toggle; per-channel enable toggles + Configure/Connect
 *               drawer; preference & session & digest toggles; priority-level
 *               picker; alert-categories reset; digest time-slot picker; sound
 *               selection + play + volume range; notification-history search +
 *               delete + clear-all; export; toast).
 *              All markup lives in notification-settings.html; this file only
 *              modifies the DOM (text/values/classes/visibility) — it never
 *              injects HTML strings. The master toggle iterates the existing
 *              channel toggle nodes and flips their .active class (no rebuild).
 * author: SignalAIX

    -------------------------------------------------
     01. Init & DOM refs (guarded by #notification-settings)
     -------------------------------------------------
     02. Toast
     -------------------------------------------------
     03. Channel drawer (add / configure / connect panels)
     -------------------------------------------------
     04. Tabs (channels / preferences / alerts / schedule / sounds / history)
     -------------------------------------------------
     05. Master toggle + per-channel toggles
     -------------------------------------------------
     06. Preference / session / digest / quiet toggles (toasts)
     -------------------------------------------------
     07. Priority levels + alert-categories reset + send test
     -------------------------------------------------
     08. Digest time-slot picker
     -------------------------------------------------
     09. Sounds: selection + play + volume range
     -------------------------------------------------
     10. History: search + delete + clear-all
     -------------------------------------------------
     11. Export + keyboard shortcuts
     -------------------------------------------------
    ================================================== */

document.addEventListener("DOMContentLoaded", () => {
  /* ======================================
   * 01. Init & DOM refs
   * ====================================== */
  const page = document.getElementById("notification-settings");
  if (!page) return; // Guard: only run on the Notification Settings page

  const refreshIcons = () => window.lucide?.createIcons?.();

  const overlay = document.getElementById("ns2DrawerOverlay");
  const channelDrawer = document.getElementById("ns2ChannelDrawer");

  const toast = document.getElementById("ns2Toast");
  const toastTitle = document.getElementById("ns2ToastTitle");
  const toastMessage = document.getElementById("ns2ToastMessage");

  // Per-channel presentation for the Connect panel
  const CONNECT_META = {
    Discord: {
      iconWrap: "w-20 h-20 rounded-2xl bg-[#5865F2]/15 flex items-center justify-center mx-auto mb-4",
      icon: "message-circle",
      iconClass: "w-10 h-10 text-[#5865F2] shrink-0",
      desc: "Connect your Discord server to receive signal notifications",
    },
    Slack: {
      iconWrap: "w-20 h-20 rounded-2xl bg-[#4A154B]/15 flex items-center justify-center mx-auto mb-4",
      icon: "hash",
      iconClass: "w-10 h-10 text-violet-400 shrink-0",
      desc: "Add SignalAIX to your Slack workspace",
    },
    "Custom Webhook": {
      iconWrap: "w-20 h-20 rounded-2xl bg-amber-500/15 flex items-center justify-center mx-auto mb-4",
      icon: "webhook",
      iconClass: "w-10 h-10 text-amber-500 shrink-0",
      desc: "Send notifications to your own custom endpoint",
    },
  };

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
  document.querySelector(".ns2-toast-close")?.addEventListener("click", hideToast);

  /* ======================================
   * 03. Channel drawer (add / configure / connect panels)
   * ====================================== */
  const panels = Array.from(document.querySelectorAll(".ns2-panel"));
  const drawerHeading = document.getElementById("ns2DrawerHeading");
  const drawerSub = document.getElementById("ns2DrawerSub");

  function showPanel(name) {
    panels.forEach((p) => p.classList.toggle("hidden", p.dataset.panel !== name));
  }
  function openDrawer() {
    channelDrawer?.classList.add("active");
    overlay?.classList.add("active");
    refreshIcons();
  }
  function closeDrawer() {
    channelDrawer?.classList.remove("active");
    overlay?.classList.remove("active");
  }
  overlay?.addEventListener("click", closeDrawer);
  document.querySelectorAll(".ns2-close-drawer").forEach((b) => b.addEventListener("click", closeDrawer));

  function openAddPanel() {
    if (drawerHeading) drawerHeading.textContent = "Add Notification Channel";
    if (drawerSub) drawerSub.textContent = "Choose a channel to receive notifications";
    showPanel("add");
    openDrawer();
  }
  function openEditPanel(channel) {
    if (drawerHeading) drawerHeading.textContent = `Configure ${channel}`;
    if (drawerSub) drawerSub.textContent = "Manage notification types for this channel";
    showPanel("edit");
    openDrawer();
  }
  function openConnectPanel(channel) {
    const meta = CONNECT_META[channel] || CONNECT_META.Discord;
    if (drawerHeading) drawerHeading.textContent = `Connect ${channel}`;
    if (drawerSub) drawerSub.textContent = "Set up a new delivery channel";

    const iconWrap = document.getElementById("ns2ConnectIconWrap");
    const icon = document.getElementById("ns2ConnectIcon");
    const name = document.getElementById("ns2ConnectName");
    const desc = document.getElementById("ns2ConnectDesc");
    const hint = document.getElementById("ns2ConnectHint");
    const btnLabel = document.getElementById("ns2ConnectBtnLabel");

    if (iconWrap) iconWrap.className = meta.iconWrap;
    if (icon) {
      icon.setAttribute("data-lucide", meta.icon);
      icon.className = meta.iconClass;
    }
    if (name) name.textContent = `Connect ${channel}`;
    if (desc) desc.textContent = meta.desc;
    if (hint) hint.textContent = `Paste your ${channel} webhook URL here`;
    if (btnLabel) btnLabel.textContent = `Connect ${channel}`;

    showPanel("connect");
    openDrawer();
  }

  // Header "Add Channel"
  document.getElementById("ns2AddChannelBtn")?.addEventListener("click", openAddPanel);

  // Add-panel options
  document.querySelectorAll(".ns2-add-opt").forEach((opt) => {
    opt.addEventListener("click", () => {
      const channel = opt.dataset.channel;
      const status = opt.dataset.status;
      if (status === "soon") {
        showToast("Coming Soon", `${channel} integration is coming soon`);
        return;
      }
      openConnectPanel(channel);
    });
  });

  // Connect / disconnect / save / test actions in the drawer
  document.getElementById("ns2ConnectBtn")?.addEventListener("click", () => {
    const name = document.getElementById("ns2ConnectName")?.textContent || "Channel";
    closeDrawer();
    showToast("Channel Connected", `${name.replace("Connect ", "")} connected successfully`);
  });
  document.getElementById("ns2TestConnBtn")?.addEventListener("click", () => {
    showToast("Test Sent", "Check your channel for the test message");
  });
  document.getElementById("ns2SaveChannelBtn")?.addEventListener("click", () => {
    closeDrawer();
    showToast("Settings Saved", "Channel settings updated successfully");
  });
  document.getElementById("ns2DisconnectBtn")?.addEventListener("click", () => {
    closeDrawer();
    showToast("Channel Disconnected", "The channel has been removed");
  });

  /* ======================================
   * 04. Tabs
   * ====================================== */
  const panes = Array.from(document.querySelectorAll(".ns2-pane"));
  document.querySelectorAll(".ns2-tab").forEach((btn) => {
    btn.addEventListener("click", () => {
      document.querySelectorAll(".ns2-tab").forEach((b) => {
        b.classList.remove("active");
        b.classList.add("text-muted");
      });
      btn.classList.add("active");
      btn.classList.remove("text-muted");
      const tab = btn.dataset.tab;
      panes.forEach((p) => p.classList.toggle("active", p.dataset.pane === tab));
      refreshIcons();
    });
  });

  /* ======================================
   * 05. Master toggle + per-channel toggles
   * ====================================== */
  const channelToggles = Array.from(document.querySelectorAll(".ns2-channel-toggle"));
  const masterToggle = document.getElementById("ns2MasterToggle");

  // Master toggle: NOT a .js-toggle-switch — owned here. Flips its own state,
  // then iterates the existing channel toggle nodes and matches them (DOM-only).
  function applyMaster(on) {
    masterToggle?.classList.toggle("active", on);
    masterToggle?.setAttribute("aria-checked", on ? "true" : "false");
    channelToggles.forEach((t) => {
      t.classList.toggle("active", on);
      t.setAttribute("aria-checked", on ? "true" : "false");
    });
  }
  masterToggle?.addEventListener("click", () => {
    const on = !masterToggle.classList.contains("active");
    applyMaster(on);
    showToast("Master Switch", on ? "All channels enabled" : "All channels paused");
  });
  masterToggle?.addEventListener("keydown", (e) => {
    if (e.key === " " || e.key === "Enter") {
      e.preventDefault();
      masterToggle.click();
    }
  });

  // Per-channel toggle: .js-toggle-switch (common.js flips .active) — we read the
  // resulting state for the toast + keep master in sync.
  function syncMaster() {
    const anyOn = channelToggles.some((t) => t.classList.contains("active"));
    masterToggle?.classList.toggle("active", anyOn);
    masterToggle?.setAttribute("aria-checked", anyOn ? "true" : "false");
  }
  channelToggles.forEach((t) => {
    t.addEventListener("click", () => {
      const on = t.classList.contains("active");
      showToast("Channel Updated", `${t.dataset.channel} ${on ? "enabled" : "disabled"}`);
      syncMaster();
    });
  });

  // Configure / Connect buttons on the channel cards
  document.querySelectorAll(".ns2-config-btn").forEach((b) => {
    b.addEventListener("click", () => openEditPanel(b.dataset.channel));
  });
  document.querySelectorAll(".ns2-connect-btn").forEach((b) => {
    b.addEventListener("click", () => openConnectPanel(b.dataset.channel));
  });

  /* ======================================
   * 06. Preference / session / digest / quiet toggles (toasts)
   * ====================================== */
  document.querySelectorAll(".ns2-pref-toggle").forEach((t) => {
    t.addEventListener("click", () => {
      const on = t.classList.contains("active");
      showToast("Setting Updated", on ? "Enabled" : "Disabled");
    });
  });

  /* ======================================
   * 07. Priority levels + alert-categories reset + send test
   * ====================================== */
  document.querySelectorAll(".ns2-priority").forEach((card) => {
    card.addEventListener("click", () => {
      document.querySelectorAll(".ns2-priority").forEach((c) => {
        c.classList.remove("active", "border-accent", "bg-accent/5");
        c.classList.add("border-border");
      });
      card.classList.add("active", "border-accent", "bg-accent/5");
      card.classList.remove("border-border");
      const p = card.dataset.priority;
      showToast("Priority Updated", `${p.charAt(0).toUpperCase() + p.slice(1)} priority selected`);
    });
  });

  document.getElementById("ns2ResetAlertsBtn")?.addEventListener("click", () => {
    showToast("Reset Complete", "Alert settings restored to defaults");
  });
  document.getElementById("ns2TestBtn")?.addEventListener("click", () => {
    showToast("Test Sent", "Test notification sent to all channels");
  });

  /* ======================================
   * 08. Digest time-slot picker (single-select per group)
   * ====================================== */
  document.querySelectorAll(".ns2-slot-group").forEach((group) => {
    group.querySelectorAll(".ns2-slot").forEach((slot) => {
      slot.addEventListener("click", () => {
        group.querySelectorAll(".ns2-slot").forEach((s) => {
          s.classList.remove("active", "border-accent", "bg-accent/15", "text-accent");
          s.classList.add("border-border", "bg-panel", "text-muted");
        });
        slot.classList.add("active", "border-accent", "bg-accent/15", "text-accent");
        slot.classList.remove("border-border", "bg-panel", "text-muted");
      });
    });
  });

  /* ======================================
   * 09. Sounds: selection + play + volume range
   * ====================================== */
  document.querySelectorAll(".ns2-sound").forEach((item) => {
    item.addEventListener("click", () => {
      document.querySelectorAll(".ns2-sound").forEach((s) => {
        s.classList.remove("selected", "border-accent", "bg-accent/5");
        s.classList.add("border-border");
      });
      item.classList.add("selected", "border-accent", "bg-accent/5");
      item.classList.remove("border-border");
      showToast("Sound Changed", `${item.dataset.sound} selected`);
    });
  });
  document.querySelectorAll(".ns2-play").forEach((btn) => {
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      showToast("Playing Sound", `Playing ${btn.dataset.sound}...`);
    });
  });

  const volume = document.getElementById("ns2Volume");
  const volumeVal = document.getElementById("ns2VolumeVal");
  volume?.addEventListener("input", () => {
    if (volumeVal) volumeVal.textContent = `${volume.value}%`;
  });

  /* ======================================
   * 10. History: search + delete + clear-all
   * ====================================== */
  const historyList = document.getElementById("ns2HistoryList");
  const historyEmpty = document.getElementById("ns2HistoryEmpty");
  const historySearch = document.getElementById("ns2HistorySearch");

  function updateHistoryEmpty() {
    const items = Array.from(document.querySelectorAll(".ns2-history"));
    const anyVisible = items.some((el) => el.style.display !== "none") && items.length > 0;
    historyEmpty?.classList.toggle("hidden", anyVisible);
  }

  historySearch?.addEventListener("input", (e) => {
    const q = e.target.value.toLowerCase();
    document.querySelectorAll(".ns2-history").forEach((el) => {
      const text = (el.dataset.text || "") + " " + el.textContent.toLowerCase();
      el.style.display = text.includes(q) ? "" : "none";
    });
    updateHistoryEmpty();
  });

  function bindDelete(btn) {
    btn.addEventListener("click", () => {
      btn.closest(".ns2-history")?.remove();
      showToast("Deleted", "Notification removed from history");
      updateHistoryEmpty();
    });
  }
  document.querySelectorAll(".ns2-history-del").forEach(bindDelete);

  document.getElementById("ns2ClearHistoryBtn")?.addEventListener("click", () => {
    document.querySelectorAll(".ns2-history").forEach((el) => el.remove());
    showToast("History Cleared", "All notification history has been removed");
    updateHistoryEmpty();
  });

  /* ======================================
   * 11. Export + keyboard shortcuts
   * ====================================== */
  document.getElementById("ns2ExportBtn")?.addEventListener("click", () => {
    showToast("Exporting", "Downloading notification settings...");
  });

  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") closeDrawer();
  });

  refreshIcons();
});
