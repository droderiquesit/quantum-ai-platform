// ── App Utilities ────────────────────────────────────────────────
"use strict";

// ── Toast Notifications ──────────────────────────────────────────
function showToast(message, type = "info") {
  let container = document.getElementById("toast-container");
  if (!container) {
    container = document.createElement("div");
    container.id = "toast-container";
    container.className =
      "fixed top-5 left-1/2 -translate-x-1/2 z-[9999] flex flex-col gap-2 max-w-[390px] w-full px-4 pointer-events-none";
    document.body.appendChild(container);
  }
  const colors = {
    success: "border-green-500/40 text-green-400",
    error: "border-red-500/40 text-red-400",
    warning: "border-yellow-500/40 text-yellow-400",
    info: "border-teal-500/40 text-teal-400",
  };
  const toast = document.createElement("div");
  toast.className = `bg-black/80 backdrop-blur-xl border ${colors[type] || colors.info} rounded-xl px-4 py-3 text-sm font-semibold pointer-events-auto transition-all duration-300 opacity-0 translate-y-2`;
  toast.textContent = message;
  container.appendChild(toast);
  requestAnimationFrame(() => {
    toast.classList.remove("opacity-0", "translate-y-2");
  });
  setTimeout(() => {
    toast.classList.add("opacity-0", "-translate-y-2");
    setTimeout(() => toast.remove(), 300);
  }, 3000);
}

// ── Theme Toggle ─────────────────────────────────────────────────
function toggleTheme() {
  const html = document.documentElement;
  const isDark = html.classList.toggle("dark");
  localStorage.setItem("theme", isDark ? "dark" : "light");
  // Update all theme toggle icons
  document.querySelectorAll(".theme-icon-moon").forEach((el) => {
    el.style.display = isDark ? "block" : "none";
  });
  document.querySelectorAll(".theme-icon-sun").forEach((el) => {
    el.style.display = isDark ? "none" : "block";
  });
}

// Apply saved theme on load and set icons
(function () {
  const saved = localStorage.getItem("theme") || "dark";
  const html = document.documentElement;
  if (saved === "dark") {
    html.classList.add("dark");
  } else {
    html.classList.remove("dark");
  }
})();

// Update theme icons after DOM ready
document.addEventListener("DOMContentLoaded", () => {
  const isDark = document.documentElement.classList.contains("dark");
  document
    .querySelectorAll(".theme-icon-moon")
    .forEach((el) => (el.style.display = isDark ? "block" : "none"));
  document
    .querySelectorAll(".theme-icon-sun")
    .forEach((el) => (el.style.display = isDark ? "none" : "block"));
});

// ── Copy to clipboard ─────────────────────────────────────────────
function copyToClipboard(text) {
  navigator.clipboard
    .writeText(text)
    .then(() => showToast("Copied!", "success"))
    .catch(() => showToast("Failed to copy — please try again", "error"));
}

// ── Format helpers ────────────────────────────────────────────────
function formatPrice(n) {
  if (n >= 1000)
    return (
      "$" +
      n.toLocaleString("en-US", {
        minimumFractionDigits: 2,
        maximumFractionDigits: 2,
      })
    );
  if (n >= 1) return "$" + n.toFixed(2);
  return "$" + n.toFixed(4);
}

function formatChange(n) {
  return (n >= 0 ? "+" : "") + n.toFixed(2) + "%";
}

function formatLargeNum(n) {
  if (n >= 1e12) return "$" + (n / 1e12).toFixed(2) + "T";
  if (n >= 1e9) return "$" + (n / 1e9).toFixed(2) + "B";
  if (n >= 1e6) return "$" + (n / 1e6).toFixed(2) + "M";
  return "$" + n.toLocaleString();
}

// ── Active nav highlighting ────────────────────────────────────────
(function () {
  const path = window.location.pathname.split("/").pop();
  const navMap = {
    "home.html": "nav-home",
    "balance.html": "nav-home",
    "quick-actions.html": "nav-home",
    "market-overview.html": "nav-market",
    "trending.html": "nav-market",
    "gainers-losers.html": "nav-market",
    "market-categories.html": "nav-market",
    "coin-detail.html": "nav-market",
    "asset-news.html": "nav-market",
    "trade.html": "nav-trade",
    "buy-crypto.html": "nav-trade",
    "sell-crypto.html": "nav-trade",
    "limit-order.html": "nav-trade",
    "swap.html": "nav-trade",
    "portfolio-overview.html": "nav-portfolio",
    "asset-allocation.html": "nav-portfolio",
    "portfolio-performance.html": "nav-portfolio",
    "profit-loss.html": "nav-portfolio",
    "staking-dashboard.html": "nav-portfolio",
    "profile.html": "nav-profile",
    "security.html": "nav-profile",
    "app-settings.html": "nav-profile",
    "wallet.html": "nav-profile",
  };
  const activeId = navMap[path];
  if (activeId) {
    const el = document.getElementById(activeId);
    if (el) {
      el.classList.add("text-primary");
      el.classList.remove("text-white/40");
    }
  }
})();

// ── Bottom sheet helpers ───────────────────────────────────────────
function openSheet(id) {
  const overlay = document.getElementById(id + "-overlay");
  const sheet = document.getElementById(id + "-sheet");
  if (!overlay || !sheet) return;
  overlay.classList.remove("hidden");
  sheet.classList.remove("translate-y-full");
}

function closeSheet(id) {
  const overlay = document.getElementById(id + "-overlay");
  const sheet = document.getElementById(id + "-sheet");
  if (!overlay || !sheet) return;
  sheet.classList.add("translate-y-full");
  setTimeout(() => overlay.classList.add("hidden"), 300);
}
