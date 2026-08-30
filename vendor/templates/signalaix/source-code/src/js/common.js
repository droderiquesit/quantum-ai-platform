/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: All Pages
 * description: SignalAIX - Common Utilities & Logic
 * author: SignalAIX

    -------------------------------------------------
     01. Startup
     -------------------------------------------------
     02. Theme Management
     -------------------------------------------------
     03. Global Elements
     -------------------------------------------------
     04. Utilities
     -------------------------------------------------
     05. Drawer System
     -------------------------------------------------
     06. Modal System
     -------------------------------------------------
     07. Dropdown System
     -------------------------------------------------
     08. Navigation Logic
     -------------------------------------------------
     09. Notifications
     -------------------------------------------------
     10. Filtering & Search
     -------------------------------------------------
     11. UI Components
     -------------------------------------------------
     12. Data Export
     -------------------------------------------------
     13. Keyboard Shortcuts
     -------------------------------------------------
     14. Global Initialization
     -------------------------------------------------
     15. Quick Trade Feature
     -------------------------------------------------
    ================================================== */
import { createIcons, icons } from "lucide";

/**
 * ======================================
 * 01. Startup
 * ======================================
 */
document.addEventListener("DOMContentLoaded", initCommon);

// Loader Logic
window.addEventListener("load", () => {
  const loader = document.getElementById("loader");
  if (loader) {
    // Small delay to ensure smooth transition even on fast loads
    setTimeout(() => {
      loader.classList.add("hidden");
    }, 500);
  }
});

// Expose lucide to window for compatibility

/**
 * ======================================
 * 02. Theme Management
 * ======================================
 */
/**
 * Toggles the theme between dark and light.
 */
function toggleTheme() {
  html.classList.toggle("dark");
  localStorage.setItem("theme", html.classList.contains("dark") ? "dark" : "light");

  if (window.lucide) lucide.createIcons();
  document.dispatchEvent(new Event("themeChanged"));

  document.documentElement.style.colorScheme = html.classList.contains("dark") ? "dark" : "light";
}
window.toggleTheme = toggleTheme;
window.lucide = {
  createIcons: (args) => {
    if (!args || !args.icons) {
      return createIcons({ icons, ...args });
    }
    return createIcons(args);
  },
  icons,
};

/**
 * ======================================
 * 03. Global Elements
 * ======================================
 */
const html = document.documentElement;
const drawerOverlay = document.getElementById("drawerOverlay");

/**
 * ======================================
 * 04. Utilities
 * ======================================
 */

/**
 * Helper to get Chart.js colors based on the current theme.
 * @returns {object} { gridColor, textColor }
 */
function getChartColors() {
  const isDark = html.classList.contains("dark");
  return {
    isDark,
    gridColor: isDark ? "rgba(255, 255, 255, 0.05)" : "rgba(0, 0, 0, 0.05)",
    textColor: isDark ? "#94A3B8" : "#64748B",
    tooltipBg: isDark ? "#1E293B" : "#FFFFFF",
    tooltipTitle: isDark ? "#F1F5F9" : "#0F172A",
    tooltipBody: isDark ? "#94A3B8" : "#64748B",
    tooltipBorder: isDark ? "#334155" : "#E2E8F0",
  };
}

// Expose to window for other scripts
window.getChartColors = getChartColors;

/**
 * ======================================
 * 05. Drawer System
 * ======================================
 */
/**
 * Opens a drawer with content and optional title
 * This is the main function all pages should use to open drawers
 * @param {string} content - HTML content to display in drawer
 * @param {string} title - Optional title for the drawer
 */
function openDrawer(content, title = "") {
  const drawer = document.getElementById("drawer");
  const drawerContent = document.getElementById("drawerContent");
  const drawerTitle = document.getElementById("drawerTitle");
  const drawerOverlay = document.getElementById("drawerOverlay");

  if (!drawer || !drawerOverlay) {
    console.error("Drawer elements not found");
    return;
  }

  // Set content
  if (content && drawerContent) {
    drawerContent.innerHTML = content;
  }

  // Set title
  if (title && drawerTitle) {
    drawerTitle.textContent = title;
  }

  // Show drawer
  drawerOverlay.classList.add("active");
  drawer.classList.add("active");
  document.body.style.overflow = "hidden";

  // Attach close button listeners and reinitialize icons (for dynamically added content)
  setTimeout(() => {
    attachDrawerCloseListeners();
    if (window.lucide) lucide.createIcons();
  }, 50);
}
window.openDrawer = openDrawer;

/**
 * Legacy function name for backward compatibility
 * @deprecated Use openDrawer() instead
 */
function openDrawerUI(content, title) {
  openDrawer(content, title);
}
window.openDrawerUI = openDrawerUI;

/**
 * Closes all drawers
 */
function closeAllDrawers() {
  const drawerOverlay = document.getElementById("drawerOverlay");
  const drawerContent = document.getElementById("drawerContent");

  // Stop video playback by removing iframe src
  if (drawerContent) {
    const iframe = drawerContent.querySelector("iframe");
    if (iframe) {
      iframe.src = "";
    }
  }

  if (drawerOverlay) drawerOverlay.classList.remove("active");

  document.querySelectorAll(".drawer.active").forEach((d) => d.classList.remove("active"));
  document.body.style.overflow = "";

  // Clear drawer content after animation
  setTimeout(() => {
    if (drawerContent) {
      drawerContent.innerHTML = "";
    }
  }, 300);

  lucide.createIcons();
}
window.closeAllDrawers = closeAllDrawers;

/**
 * Attach event listeners to close buttons within drawer content
 * Called automatically by openDrawer(), but can be called manually if needed
 */
function attachDrawerCloseListeners() {
  document.querySelectorAll(".js-close-drawer").forEach((btn) => {
    // Remove old listeners by cloning
    const newBtn = btn.cloneNode(true);
    btn.parentNode?.replaceChild(newBtn, btn);

    // Add fresh listener
    newBtn.addEventListener("click", closeAllDrawers);
  });
}

window.attachDrawerCloseListeners = attachDrawerCloseListeners;

/**
 * ======================================
 * 06. Modal System
 * ======================================
 */
/**
 * Closes any open modals.
 */
function closeAllModals() {
  document.getElementById("searchModal")?.classList.remove("active");
  document.getElementById("exportModal")?.classList.remove("active");
  document.getElementById("filterModal")?.classList.remove("active");
  document.querySelectorAll(".modal-overlay.active").forEach((modal) => modal.classList.remove("active"));
  document.querySelectorAll(".modal.active").forEach((modal) => modal.classList.remove("active"));
}
window.closeAllModals = closeAllModals;

/**
 * Opens a modal by ID.
 * @param {string} modalId - The ID of the modal to open
 */
function openModal(modalId) {
  const modal = document.getElementById(modalId);
  if (modal) modal.classList.add("active");
}
window.openModal = openModal;

/**
 * ======================================
 * 07. Dropdown System
 * ======================================
 */
/**
 * Closes all open dropdown menus.
 */
function closeAllDropdowns() {
  document.querySelectorAll(".dropdown-menu.active").forEach((menu) => {
    menu.classList.remove("active");
  });
}
window.closeAllDropdowns = closeAllDropdowns;

/**
 * ======================================
 * 08. Navigation Logic
 * ======================================
 */
/**
 * Sets the active tab visually and triggers tab-specific logic.
 * @param {string} tabId The ID of the tab to switch to (e.g., 'holdings', 'bots').
 */
function switchTab(tabId) {
  // Update tab buttons
  document.querySelectorAll(".js-tab-button, .tab-button").forEach((btn) => {
    const target = btn.dataset.tab || btn.dataset.tabId;
    btn.classList.toggle("active", target === tabId);
  });

  // Update tab content
  document.querySelectorAll(".tab-content").forEach((content) => {
    content.classList.remove("active");
  });

  // Try ID formats: "tab-{id}" or "{id}-content"
  document.getElementById("tab-" + tabId)?.classList.add("active");
  document.getElementById(tabId + "-content")?.classList.add("active");

  window.dispatchEvent(new Event("resize"));
}
window.switchTab = switchTab;

/**
 * ======================================
 * 09. Notifications
 * ======================================
 */
/**
 * Shows a notification toast message.
 * @param {string} message - The message to display
 * @param {string} type - Type of notification (success, error, info, warning)
 * @param {number} duration - Duration in milliseconds
 */
function showToast(message, type = "info", duration = 3000) {
  // Try to use existing toast if available
  const existingToast = document.getElementById("toast");
  const existingToastMessage = document.getElementById("toastMessage");

  if (existingToast && existingToastMessage) {
    existingToastMessage.textContent = message;

    // Update styling based on type
    const typeClasses = {
      success: "toast-success",
      error: "toast-error",
      warning: "toast-warning",
      info: "toast-info",
    };

    existingToast.className = `toast ${typeClasses[type] || ""} active`;

    setTimeout(() => {
      existingToast.classList.remove("active");
    }, duration);
    return;
  }

  // Create a new toast if none exists
  const toast = document.createElement("div");
  const colors = {
    success: "bg-emerald-500",
    error: "bg-red-500",
    warning: "bg-amber-500",
    info: "bg-gray-800",
  };

  toast.className = `fixed bottom-24 left-1/2 -translate-x-1/2 px-6 py-3 rounded-xl text-white font-medium z-[400] transition-all transform translate-y-10 opacity-0 ${colors[type] || colors.info}`;
  toast.textContent = message;
  document.body.appendChild(toast);

  setTimeout(() => {
    toast.classList.remove("translate-y-10", "opacity-0");
  }, 10);

  setTimeout(() => {
    toast.classList.add("translate-y-10", "opacity-0");
    setTimeout(() => toast.remove(), 300);
  }, duration);
}
window.showToast = showToast;

/**
 * ======================================
 * 10. Filtering & Search
 * ======================================
 */
/**
 * Generic search function for filtering tables/cards.
 * @param {string} query - Search query
 * @param {string} selector - CSS selector for items to filter
 * @param {string} textSelector - Optional child selector for text content
 */
function filterBySearch(query, selector, textSelector = null) {
  const items = document.querySelectorAll(selector);
  const lowerQuery = query.toLowerCase();

  items.forEach((item) => {
    const text = textSelector ? item.querySelector(textSelector)?.textContent.toLowerCase() : item.textContent.toLowerCase();

    if (text.includes(lowerQuery) || lowerQuery === "") {
      item.style.display = "";
    } else {
      item.style.display = "none";
    }
  });
}
window.filterBySearch = filterBySearch;

/**
 * Generic filter function for filtering by data attribute.
 * @param {string} filter - Filter value
 * @param {string} selector - CSS selector for items to filter
 * @param {string} attribute - Data attribute name
 */
function filterByAttribute(filter, selector, attribute = "filter") {
  const items = document.querySelectorAll(selector);

  items.forEach((item) => {
    if (filter === "all" || item.dataset[attribute] === filter) {
      item.style.display = "";
    } else {
      item.style.display = "none";
    }
  });
}
window.filterByAttribute = filterByAttribute;

/**
 * Updates active state for filter chips/buttons.
 * @param {HTMLElement} element - The clicked element
 * @param {string} selector - CSS selector for sibling elements
 */
function updateActiveFilter(element, selector = ".filter-chip") {
  const parent = element.parentElement;
  parent.querySelectorAll(selector).forEach((chip) => {
    chip.classList.remove("active");
  });
  element.classList.add("active");
}
window.updateActiveFilter = updateActiveFilter;

/**
 * ======================================
 * 11. UI Components
 * ======================================
 */
/**
 * Initialize all toggle switches on the page.
 * Safe to call multiple times - will skip already initialized switches.
 */
function initToggleSwitches() {
  document.querySelectorAll(".js-toggle-switch").forEach((switchEl) => {
    // Skip if already initialized
    if (switchEl.dataset.toggleInitialized) return;

    switchEl.dataset.toggleInitialized = "true";
    switchEl.addEventListener("click", () => {
      switchEl.classList.toggle("active");
    });
  });
}
window.initToggleSwitches = initToggleSwitches;

/**
 * ======================================
 * 12. Data Export
 * ======================================
 */
/**
 * Export data in various formats.
 * @param {Array} data - Data to export
 * @param {string} filename - Base filename
 * @param {string} format - Export format (csv, json, pdf)
 */
function exportData(data, filename, format = "csv") {
  let content, fullFilename, type;

  if (format === "csv") {
    const headers = Object.keys(data[0]).join(",");
    const rows = data.map((item) => Object.values(item).join(",")).join("\n");
    content = headers + "\n" + rows;
    fullFilename = `${filename}.csv`;
    type = "text/csv";
  } else if (format === "json") {
    content = JSON.stringify(data, null, 2);
    fullFilename = `${filename}.json`;
    type = "application/json";
  } else {
    alert("PDF export would be generated here");
    return;
  }

  const blob = new Blob([content], { type });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = fullFilename;
  a.click();
  URL.revokeObjectURL(url);
}
window.exportData = exportData;

/**
 * ======================================
 * 13. Keyboard Shortcuts
 * ======================================
 */
/**
 * Setup global keyboard shortcuts
 */
function setupKeyboardShortcuts() {
  document.addEventListener("keydown", (e) => {
    // Ctrl/Cmd + K for search
    if ((e.ctrlKey || e.metaKey) && e.key === "k") {
      e.preventDefault();
      const searchInput = document.querySelector(".js-search-input, .search-input, #globalSearch, #protocolSearch");
      if (searchInput) searchInput.focus();
    }

    // Escape to close drawers/modals/dropdowns
    if (e.key === "Escape") {
      closeAllDrawers();
      closeAllModals();
      closeAllDropdowns();
    }
  });
}

/**
 * ======================================
 * 14. Global Initialization
 * ======================================
 */

/**
 * Initializes common UI elements and listeners on page load.
 */
function initCommon() {
  // 1. Initialize Lucide Icons
  if (window.lucide) {
    lucide.createIcons();
  }

  // 2. Load saved theme
  const savedTheme = localStorage.getItem("theme");
  if (savedTheme === "light") {
    html.classList.remove("dark");
  } else {
    html.classList.add("dark");
  }

  // 3. Setup Global Event Listeners
  setupGlobalListeners();

  // 4. Paint the filled portion of every range slider (the `.range` class).
  initRangeFills();
}

/**
 * Paints the filled track of every `<input type="range">` by setting a
 * `--range-fill` percentage the CSS uses for the track background. Runs once on
 * load and updates live on input — covers all sliders on every page with no
 * per-page wiring. (Native range inputs only color the thumb via accent-color;
 * the fill is otherwise invisible.)
 */
function initRangeFills() {
  const paint = (el) => {
    const min = parseFloat(el.min) || 0;
    const max = parseFloat(el.max) || 100;
    const val = parseFloat(el.value);
    const pct = max > min ? ((val - min) / (max - min)) * 100 : 0;
    el.style.setProperty("--range-fill", `${pct}%`);
  };
  document.querySelectorAll('input[type="range"]').forEach((el) => {
    paint(el);
    el.addEventListener("input", () => paint(el));
  });
}

/**
 * Sets up listeners for common interactions (theme, tabs, close buttons).
 */
function setupGlobalListeners() {
  // Theme Toggles
  document.querySelectorAll(".js-theme-toggle").forEach((btn) => {
    btn.addEventListener("click", toggleTheme);
  });

  // Tab Buttons
  document.querySelectorAll(".js-tab-button").forEach((btn) => {
    btn.addEventListener("click", () => {
      const tabId = btn.dataset.tab || btn.dataset.tabId;
      switchTab(tabId);
    });
  });

  // Close Drawers/Modals on Overlay Click
  if (drawerOverlay) {
    drawerOverlay.addEventListener("click", closeAllDrawers);
  }

  // Close Modals when clicking on overlay (outside modal content)
  document.querySelectorAll(".modal-overlay").forEach((overlay) => {
    overlay.addEventListener("click", (e) => {
      // Only close if clicking directly on overlay, not on modal content
      if (e.target === overlay) {
        closeAllModals();
      }
    });
  });

  // Close Buttons for Generic Drawer/Modals
  document.querySelectorAll(".js-close-drawer, .js-close-modal").forEach((btn) => {
    btn.addEventListener("click", () => {
      if (btn.classList.contains("js-close-drawer")) {
        closeAllDrawers();
      } else if (btn.classList.contains("js-close-modal")) {
        closeAllModals();
      }
    });
  });

  // Modal Open Buttons (Filter and Export)
  // Only add modal handlers to buttons WITHOUT dropdown menus
  document.querySelectorAll(".js-filter-btn").forEach((btn) => {
    const hasDropdown = btn.nextElementSibling?.classList.contains("dropdown-menu") || btn.parentElement.querySelector(".dropdown-menu");
    if (!hasDropdown) {
      btn.addEventListener("click", () => openModal("filterModal"));
    }
  });

  document.querySelectorAll(".js-export-btn").forEach((btn) => {
    const hasDropdown = btn.nextElementSibling?.classList.contains("dropdown-menu") || btn.parentElement.querySelector(".dropdown-menu");
    if (!hasDropdown) {
      btn.addEventListener("click", () => openModal("exportModal"));
    }
  });

  // Drawer Open Buttons
  document.querySelectorAll(".js-open-drawer").forEach((btn) => {
    btn.addEventListener("click", () => {
      lucide.createIcons();
      const drawerId = btn.dataset.drawer;
      const drawerType = btn.dataset.drawerType;

      if (drawerId) {
        // Handle static drawers with IDs
        const drawer = document.getElementById(`drawer-${drawerId}`);
        const overlay = document.getElementById("drawerOverlay");
        if (drawer && overlay) {
          overlay.classList.add("active");
          drawer.classList.add("active");
        }
      } else if (drawerType) {
        // Handle dynamic drawers with types
        if (drawerType === "quickTrade") {
          handleQuickTradeDrawer();
        } else if (window.openDynamicDrawer) {
          window.openDynamicDrawer(drawerType);
        } else {
          console.error("openDynamicDrawer function not found");
        }
      }
    });
  });

  // Setup keyboard shortcuts
  setupKeyboardShortcuts();

  // Close Dropdowns on Outside Click
  document.addEventListener("click", (e) => {
    const isDropdownToggle = e.target.closest(".js-sort-btn") || e.target.closest(".js-export-btn") || e.target.closest(".js-filter-btn") || e.target.closest(".js-dropdown-toggle");
    const isDropdownMenu = e.target.closest(".dropdown-menu");

    if (!isDropdownToggle && !isDropdownMenu) {
      closeAllDropdowns();
    }
  });

  // Dropdown Toggles (Delegated)
  document.addEventListener("click", (e) => {
    const toggleBtn = e.target.closest(".js-dropdown-toggle");
    if (toggleBtn) {
      e.stopPropagation();
      const dropdownId = toggleBtn.dataset.dropdown;
      const dropdown = document.getElementById(dropdownId);
      if (dropdown) {
        const isActive = dropdown.classList.contains("active");
        closeAllDropdowns();
        if (!isActive) {
          dropdown.classList.add("active");
        }
      }
    }
  });

  // Export Actions (Delegated)
  document.addEventListener("click", (e) => {
    const exportItem = e.target.closest(".js-export-action");
    if (exportItem) {
      e.stopPropagation();
      const format = exportItem.dataset.format || "csv";
      alert(`Exporting data as ${format.toUpperCase()}...`);
      closeAllDropdowns();
    }
  });

  // Initialize toggle switches
  initToggleSwitches();
}

/**
 * ======================================
 * 15. Quick Trade Feature
 * ======================================
 */

function handleQuickTradeDrawer() {
  const drawerTitle = document.getElementById("drawerTitle");
  let content = getQuickTradeContent();

  if (drawerTitle) {
    // Standard Page: Use generic openDrawer which sets title in existing header
    openDrawer(content, "Quick Trade");
  } else {
    // Non-Standard Page (e.g. Execution Logs): Inject full header + padded content
    const fullContent = `
        <div class="p-6 border-b border-border flex items-center justify-between">
            <h3 class="text-lg font-semibold">Quick Trade</h3>
            <button class="p-2 rounded-xl hover:bg-bg transition-all js-close-drawer" aria-label="Close Drawer">
                <i data-lucide="x" class="w-5 h-5"></i>
            </button>
        </div>
        <div class="p-6">
            ${content}
        </div>
    `;
    // Pass full content to openDrawer.
    // openDrawer checks for drawerTitle internally to set Title, but we don't pass a title here
    // because we injected our own header.
    openDrawer(fullContent);
  }

  // Initialize icons
  if (window.lucide) window.lucide.createIcons();

  // Add event listeners specific to this drawer
  setTimeout(() => {
    document.querySelectorAll(".js-trade-type-toggle").forEach((btn) => {
      btn.addEventListener("click", () => {
        const type = btn.dataset.tradeType;
        setTradeTypeInDrawer(btn, type);
      });
    });
  }, 100);
}

function getQuickTradeContent() {
  return `
        <div class="space-y-6">
            <div class="flex gap-2">
                <button class="flex-1 py-3 rounded-xl font-semibold text-white gradient-success js-trade-type-toggle" data-trade-type="buy">Buy</button>
                <button class="flex-1 py-3 rounded-xl font-semibold text-muted bg-border/50 js-trade-type-toggle" data-trade-type="sell">Sell</button>
            </div>
            <div>
                <label class="block text-sm font-medium text-text mb-2">Trading Pair</label>
                <select class="form-control">
                    <option>BTC/USDT</option>
                    <option>ETH/USDT</option>
                    <option>SOL/USDT</option>
                    <option>LINK/USDT</option>
                </select>
            </div>
            <div>
                <label class="block text-sm font-medium text-text mb-2">Amount</label>
                <div class="relative">
                    <span class="absolute left-4 top-1/2 -translate-y-1/2! text-muted">$</span>
                    <input type="number" class="form-control pl-8!" placeholder="100.00" />
                </div>
                <div class="flex gap-2 mt-2">
                    <button class="filter-chip">25%</button>
                    <button class="filter-chip">50%</button>
                    <button class="filter-chip">Max</button>
                </div>
            </div>
            <div class="p-4 rounded-xl bg-border/30 space-y-3">
                <div class="flex justify-between text-sm">
                    <span class="text-muted">Price</span>
                    <span class="font-medium text-text">$64,230.50</span>
                </div>
                <div class="flex justify-between text-sm">
                    <span class="text-muted">Fee (0.1%)</span>
                    <span class="font-medium text-text">$0.10</span>
                </div>
                <div class="border-t border-border pt-3 flex justify-between font-bold">
                    <span class="text-text">Total</span>
                    <span class="text-text">$100.10</span>
                </div>
            </div>
            <button class="btn btn-primary w-full py-4 text-lg">
                Buy BTC
            </button>
        </div>
    `;
}

function setTradeTypeInDrawer(element, type) {
  const buttons = element.parentElement.querySelectorAll("button");
  buttons.forEach((btn) => {
    btn.className = "flex-1 py-3 rounded-xl font-semibold text-muted bg-border/50 js-trade-type-toggle";
  });

  if (type === "buy") {
    element.className = "flex-1 py-3 rounded-xl font-semibold text-white gradient-success js-trade-type-toggle";
    // Update button text/color usually happens here too
    const submitBtn = element.closest(".space-y-6").querySelector(".btn-primary");
    if (submitBtn) {
      submitBtn.textContent = "Buy BTC";
      submitBtn.classList.remove("gradient-danger");
      submitBtn.classList.add("gradient-success"); // Assuming btn-primary has gradient-success default or similar, otherwise hard to toggle safely without full class list knowledge. But for now let's stick to the visual toggle.
    }
  } else {
    element.className = "flex-1 py-3 rounded-xl font-semibold text-white gradient-danger js-trade-type-toggle";
    const submitBtn = element.closest(".space-y-6").querySelector(".btn-primary");
    if (submitBtn) {
      submitBtn.textContent = "Sell BTC";
      // In a real app we might want to swap classes for color.
    }
  }
}
