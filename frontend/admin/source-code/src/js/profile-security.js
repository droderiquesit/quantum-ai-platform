/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: profile-security.html
 * description: SignalAIX - Profile & Security (Account) Page Controller
 *              Self-contained; mirrors the reference mockup's functionality
 *              (6 tabs: Profile / Security / Sessions & Devices / API Keys /
 *               Privacy / Activity Log; profile info edit-mode toggles, primary-
 *               market & risk-tolerance chips, social connect/disconnect; password
 *               change drawer with live strength bar + show/hide; 2FA enable/disable
 *               + status text, reconfigure & backup-codes drawers; sessions filter
 *               + revoke single / revoke all; API keys show/hide masked value, copy,
 *               delete; privacy/preference toggles; activity log filter chips +
 *               search; edit-profile / avatar / improve-security / create-key /
 *               delete-account drawers; toast).
 *              All markup lives in profile-security.html; this file only modifies the
 *              DOM (text/values/classes/visibility, swap data-lucide eye<->eye-off,
 *              remove nodes on revoke/delete) — it never injects HTML strings.
 * author: SignalAIX

    -------------------------------------------------
     01. Init & DOM refs (guarded by #profile-security)
     -------------------------------------------------
     02. Toast
     -------------------------------------------------
     03. Drawers + overlay + close + keyboard
     -------------------------------------------------
     04. Tabs
     -------------------------------------------------
     05. Profile: edit-mode toggles, chips, social
     -------------------------------------------------
     06. Header actions + Security drawers (password, 2FA, backup, improve, avatar)
     -------------------------------------------------
     07. Sessions: filter chips + revoke single / revoke all
     -------------------------------------------------
     08. API Keys: show/hide + copy + delete + create
     -------------------------------------------------
     09. Privacy preferences + delete account + activity filter/search
     -------------------------------------------------
    ================================================== */

document.addEventListener("DOMContentLoaded", () => {
  /* ======================================
   * 01. Init & DOM refs
   * ====================================== */
  const page = document.getElementById("profile-security");
  if (!page) return; // Guard: only run on the Profile & Security page

  const refreshIcons = () => window.lucide?.createIcons?.();

  const overlay = document.getElementById("psDrawerOverlay");
  const drawers = {
    edit: document.getElementById("psEditDrawer"),
    password: document.getElementById("psPasswordDrawer"),
    security: document.getElementById("psSecurityDrawer"),
    avatar: document.getElementById("psAvatarDrawer"),
    twofa: document.getElementById("ps2faDrawer"),
    key: document.getElementById("psKeyDrawer"),
    backup: document.getElementById("psBackupDrawer"),
    revoke: document.getElementById("psRevokeDrawer"),
    delete: document.getElementById("psDeleteDrawer"),
  };
  const allDrawers = Object.values(drawers);

  /* ======================================
   * 02. Toast
   * ====================================== */
  const toast = document.getElementById("psToast");
  const toastTitle = document.getElementById("psToastTitle");
  const toastMessage = document.getElementById("psToastMessage");
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
  document.querySelector(".ps-toast-close")?.addEventListener("click", hideToast);

  function copyText(text, msg) {
    navigator.clipboard?.writeText(text).then(
      () => showToast("Copied", msg),
      () => showToast("Copied", msg),
    );
  }

  /* ======================================
   * 03. Drawers + overlay + close + keyboard
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
  document.querySelectorAll(".ps-close-drawer").forEach((btn) => btn.addEventListener("click", closeDrawers));

  /* ======================================
   * 04. Tabs
   * ====================================== */
  const tabs = Array.from(document.querySelectorAll(".ps-tab"));
  const panes = Array.from(document.querySelectorAll(".ps-pane"));
  tabs.forEach((tab) => {
    tab.addEventListener("click", () => {
      const name = tab.dataset.tab;
      tabs.forEach((t) => {
        t.classList.toggle("active", t === tab);
        t.classList.toggle("text-muted", t !== tab);
      });
      panes.forEach((p) => p.classList.toggle("active", p.dataset.pane === name));
      refreshIcons();
    });
  });

  /* ======================================
   * 05. Profile: edit-mode toggles, chips, social
   * ====================================== */
  // Inline edit-mode: enable/disable inputs in the section's form block
  document.querySelectorAll(".ps-edit-toggle").forEach((btn) => {
    btn.addEventListener("click", () => {
      const section = btn.dataset.section;
      const form = document.querySelector(`[data-form="${section}"]`);
      if (!form) return;
      const fields = form.querySelectorAll("input, textarea, select");
      const enabling = fields.length && fields[0].disabled;
      fields.forEach((f) => (f.disabled = !enabling));
      btn.classList.toggle("text-accent", enabling);
      btn.classList.toggle("border-accent/40", enabling);
      if (!enabling) showToast("Saved", "Your changes have been saved");
    });
  });

  // Primary-market chips (multi-select)
  document.querySelectorAll(".ps-chip").forEach((chip) => {
    chip.addEventListener("click", () => {
      const on = chip.classList.toggle("active");
      if (on) {
        chip.classList.add("border-accent", "bg-accent/15", "text-accent");
        chip.classList.remove("border-border", "bg-bg", "text-muted");
      } else {
        chip.classList.remove("border-accent", "bg-accent/15", "text-accent");
        chip.classList.add("border-border", "bg-bg", "text-muted");
      }
    });
  });

  // Risk-tolerance chips (single-select)
  document.querySelectorAll(".ps-risk").forEach((chip) => {
    chip.addEventListener("click", () => {
      document.querySelectorAll(".ps-risk").forEach((c) => {
        c.classList.remove("active", "border-accent", "bg-accent/15", "text-accent");
        c.classList.add("border-border", "bg-bg", "text-muted");
      });
      chip.classList.add("active", "border-accent", "bg-accent/15", "text-accent");
      chip.classList.remove("border-border", "bg-bg", "text-muted");
    });
  });

  // Social connect / disconnect
  document.querySelectorAll(".ps-social-toggle").forEach((btn) => {
    btn.addEventListener("click", () => {
      const name = btn.dataset.name || "Service";
      const connected = btn.dataset.connected === "true";
      if (connected) {
        btn.dataset.connected = "false";
        btn.textContent = "Connect";
        btn.className =
          "ps-social-toggle px-3 py-2 rounded-xl bg-gradient-to-r from-accent to-teal-500 text-white text-sm font-semibold hover:opacity-90 transition-opacity whitespace-nowrap shrink-0";
        btn.dataset.name = name;
        showToast("Disconnected", `${name} has been disconnected`);
      } else {
        btn.dataset.connected = "true";
        btn.textContent = "Disconnect";
        btn.className =
          "ps-social-toggle px-3 py-2 rounded-xl bg-panel border border-border text-text text-sm font-semibold hover:border-red-500/40 hover:text-red-500 transition-colors whitespace-nowrap shrink-0";
        btn.dataset.name = name;
        showToast("Connected", `${name} has been connected`);
      }
    });
  });

  /* ======================================
   * 06. Header actions + Security drawers
   * ====================================== */
  document.getElementById("psEditProfileBtn")?.addEventListener("click", () => openDrawer(drawers.edit));
  document.getElementById("psChangePwBtn")?.addEventListener("click", () => openDrawer(drawers.password));
  document.querySelector(".ps-changepw-trigger")?.addEventListener("click", () => openDrawer(drawers.password));
  document.getElementById("psImproveBtn")?.addEventListener("click", () => openDrawer(drawers.security));
  document.getElementById("psAvatarBtn")?.addEventListener("click", () => openDrawer(drawers.avatar));
  document.getElementById("psSetup2faBtn")?.addEventListener("click", () => openDrawer(drawers.twofa));
  document.getElementById("psBackupCodesBtn")?.addEventListener("click", () => openDrawer(drawers.backup));
  document.getElementById("psCreateKeyBtn")?.addEventListener("click", () => openDrawer(drawers.key));

  document.getElementById("psEditSave")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Profile Updated", "Your profile has been updated successfully");
  });

  // Password show/hide (each input + its eye button)
  document.querySelectorAll(".ps-pw-eye").forEach((btn) => {
    btn.addEventListener("click", () => {
      const input = btn.parentElement?.querySelector(".ps-pw-input");
      const icon = btn.querySelector("i");
      if (!input || !icon) return;
      if (input.type === "password") {
        input.type = "text";
        icon.setAttribute("data-lucide", "eye-off");
        btn.title = "Hide";
      } else {
        input.type = "password";
        icon.setAttribute("data-lucide", "eye");
        btn.title = "Show";
      }
      refreshIcons();
    });
  });

  // Live password strength bar (DOM-only: width + color + label)
  const strengthBar = document.getElementById("psStrengthBar");
  const strengthLabel = document.getElementById("psStrengthLabel");
  function scorePassword(pw) {
    let score = 0;
    if (pw.length >= 8) score += 1;
    if (pw.length >= 12) score += 1;
    if (/[A-Z]/.test(pw) && /[a-z]/.test(pw)) score += 1;
    if (/\d/.test(pw)) score += 1;
    if (/[^A-Za-z0-9]/.test(pw)) score += 1;
    return score; // 0..5
  }
  document.getElementById("psNewPw")?.addEventListener("input", (e) => {
    const pw = e.target.value;
    const score = scorePassword(pw);
    if (!strengthBar || !strengthLabel) return;
    let width = 0;
    let color = "bg-border";
    let label = "—";
    if (pw.length === 0) {
      width = 0;
    } else if (score <= 2) {
      width = 33;
      color = "bg-red-500";
      label = "Weak";
    } else if (score <= 3) {
      width = 66;
      color = "bg-amber-500";
      label = "Medium";
    } else {
      width = 100;
      color = "bg-emerald-500";
      label = "Strong";
    }
    strengthBar.className = `h-full rounded-full transition-all ${color}`;
    strengthBar.style.width = width + "%";
    strengthLabel.textContent = label;
    strengthLabel.className =
      "text-xs font-medium " +
      (label === "Strong" ? "text-emerald-500" : label === "Medium" ? "text-amber-500" : label === "Weak" ? "text-red-500" : "text-muted");
  });

  document.getElementById("psPasswordSave")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Password Changed", "Your password has been updated successfully");
  });

  // 2FA enable/disable toggle: update status text + toast (visual flip handled globally)
  const twofaToggle = document.getElementById("ps2faToggle");
  const twofaStatus = document.getElementById("ps2faStatus");
  twofaToggle?.addEventListener("click", () => {
    const enabled = twofaToggle.classList.contains("active");
    if (twofaStatus) {
      twofaStatus.textContent = enabled ? "Enabled • Authenticator App" : "Disabled";
      twofaStatus.className = "text-sm shrink-0 " + (enabled ? "text-emerald-500" : "text-muted");
    }
    showToast("Two-Factor Authentication", enabled ? "2FA has been enabled" : "2FA has been disabled");
  });

  document.getElementById("ps2faVerify")?.addEventListener("click", () => {
    closeDrawers();
    showToast("2FA Enabled", "Two-factor authentication is now active");
  });

  // Backup codes actions
  document.getElementById("psBackupDownload")?.addEventListener("click", () =>
    showToast("Download Started", "Your backup codes are being downloaded"),
  );
  document.getElementById("psBackupCopy")?.addEventListener("click", () => {
    const codes = Array.from(document.querySelectorAll("#psBackupGrid > div"))
      .map((el) => el.textContent.trim())
      .join("\n");
    copyText(codes, "Backup codes copied to clipboard");
  });

  // Verify phone (both inline checklist link & improve-security drawer button)
  document.querySelectorAll(".ps-verify-phone").forEach((btn) =>
    btn.addEventListener("click", () => showToast("Verification Sent", "A verification code has been sent to your phone")),
  );

  // Avatar actions
  document.getElementById("psAvatarUpload")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Photo Updated", "Your profile photo has been updated");
  });
  document.getElementById("psAvatarRemove")?.addEventListener("click", () => {
    closeDrawers();
    showToast("Photo Removed", "Your profile photo has been removed");
  });

  // View all events
  document.querySelector(".ps-view-events")?.addEventListener("click", () =>
    showToast("Security Events", "Loading full security event history..."),
  );

  /* ======================================
   * 07. Sessions: filter chips + revoke single / revoke all
   * ====================================== */
  const sessions = Array.from(document.querySelectorAll(".ps-session"));
  const sessionCount = document.getElementById("psSessionCount");

  function updateSessionCount() {
    const remaining = document.querySelectorAll("#psSessionList .ps-session").length;
    if (sessionCount) sessionCount.textContent = `${remaining} device${remaining === 1 ? "" : "s"}`;
  }

  function applySessionFilter(filter) {
    sessions.forEach((s) => {
      if (!s.isConnected) return;
      let show = true;
      if (filter === "active") show = s.dataset.status === "active";
      else if (filter === "desktop") show = s.dataset.device === "desktop";
      else if (filter === "mobile") show = s.dataset.device === "mobile";
      s.style.display = show ? "" : "none";
    });
  }

  document.querySelectorAll(".ps-session-filter").forEach((chip) => {
    chip.addEventListener("click", () => {
      document.querySelectorAll(".ps-session-filter").forEach((c) => {
        c.classList.remove("active", "border-accent", "bg-accent/15", "text-accent");
        c.classList.add("border-border", "bg-panel", "text-muted");
      });
      chip.classList.add("active", "border-accent", "bg-accent/15", "text-accent");
      chip.classList.remove("border-border", "bg-panel", "text-muted");
      applySessionFilter(chip.dataset.filter);
    });
  });

  document.querySelectorAll(".ps-revoke-session").forEach((btn) => {
    btn.addEventListener("click", () => {
      const card = btn.closest(".ps-session");
      const name = btn.dataset.name || "Session";
      card?.remove();
      updateSessionCount();
      showToast("Session Revoked", `${name} session has been terminated`);
    });
  });

  document.getElementById("psRevokeAllBtn")?.addEventListener("click", () => openDrawer(drawers.revoke));
  document.getElementById("psRevokeConfirm")?.addEventListener("click", () => {
    document.querySelectorAll("#psSessionList .ps-session").forEach((s) => s.remove());
    updateSessionCount();
    closeDrawers();
    showToast("Sessions Revoked", "All other sessions have been signed out");
  });

  document.getElementById("psHistoryExportBtn")?.addEventListener("click", () =>
    showToast("Export Started", "Your login history is being prepared for download"),
  );

  /* ======================================
   * 08. API Keys: show/hide + copy + delete + create
   * ====================================== */
  document.querySelectorAll(".ps-key-eye").forEach((btn) => {
    btn.addEventListener("click", () => {
      const value = btn.parentElement?.querySelector(".ps-key-value");
      const icon = btn.querySelector("i");
      if (!value || !icon) return;
      const revealed = value.dataset.revealed === "true";
      if (revealed) {
        value.textContent = value.dataset.mask;
        value.dataset.revealed = "false";
        icon.setAttribute("data-lucide", "eye");
        btn.title = "Show";
      } else {
        value.textContent = value.dataset.full;
        value.dataset.revealed = "true";
        icon.setAttribute("data-lucide", "eye-off");
        btn.title = "Hide";
      }
      refreshIcons();
    });
  });

  document.querySelectorAll(".ps-key-copy").forEach((btn) => {
    btn.addEventListener("click", () => {
      const value = btn.closest(".ps-key-card")?.querySelector(".ps-key-value");
      if (!value) return;
      copyText(value.dataset.full || value.textContent, "API key copied to clipboard");
    });
  });

  document.querySelectorAll(".ps-key-delete").forEach((btn) => {
    btn.addEventListener("click", () => {
      btn.closest(".ps-key-card")?.remove();
      showToast("Key Deleted", "The API key has been deleted");
    });
  });

  document.getElementById("psKeyGenerate")?.addEventListener("click", () => {
    closeDrawers();
    showToast("API Key Created", "Your new API key has been generated");
  });

  /* ======================================
   * 09. Privacy preferences + delete account + activity filter/search
   * ====================================== */
  // Preference toggles (privacy + login-alert) — fire a toast on change
  document.querySelectorAll(".js-toggle-switch[data-label]").forEach((sw) => {
    sw.addEventListener("click", () => {
      const label = sw.dataset.label || "Setting";
      const on = sw.classList.contains("active");
      showToast("Setting Updated", `${label} ${on ? "enabled" : "disabled"}`);
    });
  });

  // Blocked users: unblock
  document.querySelectorAll(".ps-unblock").forEach((btn) => {
    btn.addEventListener("click", () => {
      const name = btn.dataset.name || "User";
      btn.closest(".ps-blocked")?.remove();
      showToast("User Unblocked", `${name} has been unblocked`);
    });
  });

  document.getElementById("psDownloadDataBtn")?.addEventListener("click", () =>
    showToast("Download Requested", "We'll email you a download link shortly"),
  );

  // Delete account
  document.getElementById("psDeleteAccountBtn")?.addEventListener("click", () => openDrawer(drawers.delete));
  const deleteInput = document.getElementById("psDeleteConfirmInput");
  const deleteConfirm = document.getElementById("psDeleteConfirm");
  deleteConfirm?.addEventListener("click", () => {
    if (deleteInput && deleteInput.value.trim().toUpperCase() !== "DELETE") {
      showToast("Confirmation Required", 'Type "DELETE" to confirm account deletion');
      return;
    }
    closeDrawers();
    showToast("Account Deletion", "Your account deletion request has been submitted");
  });

  // Activity log: filter chips + search
  const activityRows = Array.from(document.querySelectorAll(".ps-activity-row"));
  const activityNoResults = document.getElementById("psActivityNoResults");
  let activityFilter = "all";
  let activitySearch = "";

  function applyActivity() {
    let visible = 0;
    activityRows.forEach((row) => {
      const typeMatch = activityFilter === "all" || row.dataset.type === activityFilter;
      const text = row.dataset.text || "";
      const searchMatch = !activitySearch || text.includes(activitySearch);
      const show = typeMatch && searchMatch;
      row.style.display = show ? "" : "none";
      if (show) visible += 1;
    });
    activityNoResults?.classList.toggle("hidden", visible !== 0);
  }

  document.querySelectorAll(".ps-activity-filter").forEach((chip) => {
    chip.addEventListener("click", () => {
      document.querySelectorAll(".ps-activity-filter").forEach((c) => {
        c.classList.remove("active", "border-accent", "bg-accent/15", "text-accent");
        c.classList.add("border-border", "bg-bg", "text-muted");
      });
      chip.classList.add("active", "border-accent", "bg-accent/15", "text-accent");
      chip.classList.remove("border-border", "bg-bg", "text-muted");
      activityFilter = chip.dataset.filter;
      applyActivity();
    });
  });

  document.getElementById("psActivitySearch")?.addEventListener("input", (e) => {
    activitySearch = e.target.value.toLowerCase();
    applyActivity();
  });

  document.getElementById("psActivityExportBtn")?.addEventListener("click", () =>
    showToast("Export Started", "Your activity log is being prepared for download"),
  );

  // Keyboard: Escape closes drawers
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") closeDrawers();
  });

  // Initial render
  refreshIcons();
});
