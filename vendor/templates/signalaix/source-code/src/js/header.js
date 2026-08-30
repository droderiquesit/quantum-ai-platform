/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: Global Header
 * description: SignalAIX - Global Header Functionality
 * author: SignalAIX

    -------------------------------------------------
     01. Initialization
     -------------------------------------------------
     02. Positioning Logic
     -------------------------------------------------
     03. Dropdown Handlers
     -------------------------------------------------
     04. Toggle Logic
     -------------------------------------------------
    ================================================== */

// Initialize header functionality
/**
 * ======================================
 * 01. Initialization
 * ======================================
 */
document.addEventListener("DOMContentLoaded", () => {
  // Dropdown Toggle
  document.querySelectorAll(".js-dropdown-toggle").forEach((btn) => {
    btn.addEventListener("click", handleDropdownToggle);
  });

  // Export/Sort/Filter buttons - only handle if they have dropdown menus
  // This prevents conflicts with modal-opening buttons in common.js
  document.querySelectorAll(".js-export-btn, .js-sort-btn, .js-filter-btn, .js-export-dropdown-toggle").forEach((btn) => {
    const dropdownMenu = btn.nextElementSibling || btn.parentElement.querySelector(".dropdown-menu");
    if (dropdownMenu && dropdownMenu.classList.contains("dropdown-menu")) {
      btn.addEventListener("click", handleUniversalDropdown);
    }
  });

  // Global Search
  const globalSearch = document.getElementById("globalSearch");
  if (globalSearch) {
    document.addEventListener("keydown", (e) => {
      if ((e.ctrlKey || e.metaKey) && e.key === "k") {
        e.preventDefault();
        globalSearch.focus();
      }
    });
  }
});

// Smart Positioning Function (reusable for all dropdowns)
/**
 * ======================================
 * 02. Positioning Logic
 * ======================================
 */
function applySmartPositioning(dropdownMenu) {
  // 1. Reset to CSS defaults (usually right: 0)
  dropdownMenu.style.left = "";
  dropdownMenu.style.right = "";

  // 2. Wait for next frame to get accurate measurements
  requestAnimationFrame(() => {
    const rect = dropdownMenu.getBoundingClientRect();
    const viewportWidth = window.innerWidth;

    // 3. Check for left overflow
    if (rect.left < 10) {
      dropdownMenu.style.right = "auto";
      dropdownMenu.style.left = "0";
    }
    // 4. Check for right overflow
    else if (rect.right > viewportWidth - 10) {
      dropdownMenu.style.left = "auto";
      dropdownMenu.style.right = "0";
    }
  });
}

// Dropdown Toggle Handler
/**
 * ======================================
 * 03. Dropdown Handlers
 * ======================================
 */
function handleDropdownToggle(e) {
  const element = e.currentTarget;
  const dropdownMenu = element.nextElementSibling || element.parentElement.querySelector(".dropdown-menu");

  toggleDropdown(dropdownMenu, e);
}

// Universal Handler for Export/Sort/Filter buttons with dropdowns
function handleUniversalDropdown(e) {
  e.preventDefault();
  e.stopPropagation();

  const element = e.currentTarget;
  const dropdownMenu = element.nextElementSibling || element.parentElement.querySelector(".dropdown-menu");

  toggleDropdown(dropdownMenu, e);
}

// Universal Dropdown Toggle Function
/**
 * ======================================
 * 04. Toggle Logic
 * ======================================
 */
function toggleDropdown(dropdownMenu, e) {
  if (!dropdownMenu) return;

  // Close other dropdowns
  document.querySelectorAll(".dropdown-menu.active").forEach((menu) => {
    if (menu !== dropdownMenu) {
      menu.classList.remove("active");
    }
  });

  const isActive = dropdownMenu.classList.toggle("active");
  if (e) e.stopPropagation();

  if (isActive) {
    applySmartPositioning(dropdownMenu);
  }
}
