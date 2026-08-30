/*  ========= js documentation ==============

 * theme name: SignalAIX
 * version: 1.0
 * Page: Global Sidebar
 * description: SignalAIX - Sidebar & Navigation Logic
 * author: SignalAIX

    -------------------------------------------------
     01. Initialization
     -------------------------------------------------
     02. Navigation Logic
     -------------------------------------------------
     03. Sidebar Interactions
     -------------------------------------------------
    ================================================== */
// DOM Elements for sidebar
const sidebar = document.getElementById("sidebar");
const mainContent = document.getElementById("mainContent");

// Initialize sidebar functionality
/**
 * ======================================
 * 01. Initialization
 * ======================================
 */
document.addEventListener("DOMContentLoaded", () => {
  // Set active navigation based on current page
  setActiveNavigation();

  // Sidebar Toggle
  document.querySelectorAll(".js-sidebar-toggle").forEach((btn) => {
    btn.addEventListener("click", toggleSidebar);
  });

  // Mobile Menu Toggle
  document.querySelectorAll(".js-mobile-menu-toggle").forEach((btn) => {
    btn.addEventListener("click", toggleMobileSidebar);
  });

  // Submenu Toggle
  document.querySelectorAll(".js-submenu-toggle").forEach((item) => {
    item.addEventListener("click", handleSubmenuToggle);
  });

  // Submenu Item Click
  document.querySelectorAll(".js-submenu-item").forEach((item) => {
    item.addEventListener("click", handleSubmenuItemClick);
  });

  // Nav Item Click
  document.querySelectorAll(".nav-item").forEach((item) => {
    if (item.tagName === "A") {
      item.addEventListener("click", handleNavItemClick);
    }
  });

  // Close sidebar on mobile when clicking outside
  // Close sidebar on mobile when clicking outside or on overlay
  const overlay = document.getElementById("sidebarOverlay");

  if (overlay) {
    overlay.addEventListener("click", closeMobileSidebar);
  }

  document.addEventListener("click", (e) => {
    if (window.innerWidth < 1024) {
      const mobileMenuBtn = document.getElementById("mobileMenuBtn");
      // If we have an overlay, the overlay click handles closing.
      // But if overlay is missing for some reason, or if we want to be doubly sure:
      if (sidebar && mobileMenuBtn && !sidebar.contains(e.target) && !mobileMenuBtn.contains(e.target) && sidebar.classList.contains("mobile-open")) {
        closeMobileSidebar();
      }
    }
  });
});

// Set Active Navigation Based on Current Page
/**
 * ======================================
 * 02. Navigation Logic
 * ======================================
 */
function setActiveNavigation() {
  // Get current page filename
  const currentPage = window.location.pathname.split("/").pop() || "index.html";

  // Remove all active classes first
  document.querySelectorAll(".nav-item, .submenu-item").forEach((item) => {
    item.classList.remove("active");
  });

  // Check submenu items first (more specific)
  let activeFound = false;
  document.querySelectorAll(".submenu-item").forEach((item) => {
    const href = item.getAttribute("href");
    if (href && (href === currentPage || href.endsWith("/" + currentPage) || href === "./" + currentPage)) {
      item.classList.add("active");
      activeFound = true;

      // Expand parent submenu
      const submenu = item.closest(".submenu");
      if (submenu) {
        submenu.classList.add("open");

        // Expand and activate parent nav-item
        const parentNavItem = submenu.previousElementSibling;
        if (parentNavItem && parentNavItem.classList.contains("js-submenu-toggle")) {
          parentNavItem.classList.add("expanded");
          parentNavItem.classList.add("active");
        }
      }
    }
  });

  // If no submenu item matched, check main nav items
  if (!activeFound) {
    document.querySelectorAll("a.nav-item").forEach((item) => {
      const href = item.getAttribute("href");
      if (href && (href === currentPage || href.endsWith("/" + currentPage) || href === "./" + currentPage)) {
        item.classList.add("active");
      }
    });
  }
}

// Sidebar Functions
/**
 * ======================================
 * 03. Sidebar Interactions
 * ======================================
 */
function toggleSidebar() {
  if (sidebar) sidebar.classList.toggle("collapsed");
  if (mainContent) mainContent.classList.toggle("expanded");
}

const sidebarOverlay = document.getElementById("sidebarOverlay");

function toggleMobileSidebar() {
  const isOpen = sidebar && sidebar.classList.contains("mobile-open");
  if (isOpen) {
    closeMobileSidebar();
  } else {
    openMobileSidebar();
  }
}

function openMobileSidebar() {
  if (sidebar) sidebar.classList.add("mobile-open");
  if (sidebarOverlay) {
    sidebarOverlay.classList.remove("hidden");
    // Force reflow to enable transition
    void sidebarOverlay.offsetWidth;
    sidebarOverlay.classList.remove("opacity-0");
  }
}

function closeMobileSidebar() {
  if (sidebar) sidebar.classList.remove("mobile-open");
  if (sidebarOverlay) {
    sidebarOverlay.classList.add("opacity-0");
    setTimeout(() => {
      if (!sidebar || !sidebar.classList.contains("mobile-open")) {
        sidebarOverlay.classList.add("hidden");
      }
    }, 300);
  }
}

function handleSubmenuToggle(e) {
  if (sidebar && sidebar.classList.contains("collapsed")) return;

  const element = e.currentTarget;
  element.classList.toggle("expanded");
  const submenuId = element.dataset.submenuId;
  const submenu = document.getElementById(submenuId);

  if (submenu && submenu.classList.contains("submenu")) {
    submenu.classList.toggle("open");
  }
}

function handleNavItemClick(e) {
  // Remove active class from all nav items
  document.querySelectorAll(".nav-item").forEach((item) => {
    item.classList.remove("active");
  });

  // Add active class to clicked item
  e.currentTarget.classList.add("active");
}

function handleSubmenuItemClick(e) {
  // Remove active class from all submenu items
  document.querySelectorAll(".submenu-item").forEach((item) => {
    item.classList.remove("active");
  });

  // Add active class to clicked item
  e.currentTarget.classList.add("active");
}
