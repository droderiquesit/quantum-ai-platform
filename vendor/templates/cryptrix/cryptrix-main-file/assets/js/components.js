// ── Reusable JS UI Components ────────────────────────────────────
"use strict";

// ── Bottom Sheet ─────────────────────────────────────────────────
const BottomSheet = {
  show(contentHTML, title = "") {
    // Remove existing
    const existing = document.getElementById("bottom-sheet-overlay");
    if (existing) existing.remove();

    const isDark = document.documentElement.classList.contains("dark");
    const overlay = document.createElement("div");
    overlay.id = "bottom-sheet-overlay";
    overlay.className =
      "fixed inset-0 bg-black/40 backdrop-blur-sm z-[100] flex items-end justify-center opacity-0 transition-opacity duration-300";
    overlay.innerHTML = `
      <div class="w-full max-w-[430px] bg-white rounded-t-[32px] z-[101] overflow-hidden translate-y-full transition-transform duration-300 shadow-2xl shadow-black/20" id="bottom-sheet-panel">
        <div class="flex items-center justify-center pt-4 pb-2">
          <div class="w-12 h-1.5 bg-gray-200 rounded-full"></div>
        </div>
        <div class="flex items-center justify-between px-6 py-4 border-b border-gray-50">
           <h3 class="text-lg font-black text-[#0f172a]">${title}</h3>
           <button onclick="BottomSheet.hide()" class="w-8 h-8 rounded-full bg-gray-50 flex items-center justify-center">
             <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="#64748b" stroke-width="3" stroke-linecap="round"><path d="M18 6L6 18M6 6l12 12"/></svg>
           </button>
        </div>
        <div class="px-6 pb-12 pt-5 max-h-[75vh] overflow-y-auto no-scrollbar">${contentHTML}</div>
      </div>
    `;
    overlay.addEventListener("click", (e) => {
      if (e.target === overlay) BottomSheet.hide();
    });
    document.body.appendChild(overlay);
  },

  hide() {
    const overlay = document.getElementById("bottom-sheet-overlay");
    const panel = document.getElementById("bottom-sheet-panel");
    if (!overlay || !panel) return;
    panel.classList.add("translate-y-full");
    overlay.classList.add("opacity-0");
    setTimeout(() => overlay.remove(), 300);
  },
};

// Auto-animate entrance
const _originalShow = BottomSheet.show;
BottomSheet.show = function () {
  _originalShow.apply(this, arguments);
  const overlay = document.getElementById("bottom-sheet-overlay");
  const panel = document.getElementById("bottom-sheet-panel");
  setTimeout(() => {
    overlay.classList.remove("opacity-0");
    panel.classList.remove("translate-y-full");
  }, 10);
};

window.BottomSheet = BottomSheet;

// ── Modal ────────────────────────────────────────────────────────
const Modal = {
  show(contentHTML) {
    const existing = document.getElementById("modal-overlay");
    if (existing) existing.remove();

    const overlay = document.createElement("div");
    overlay.id = "modal-overlay";
    overlay.className =
      "fixed inset-0 bg-black/50 backdrop-blur-md z-[100] flex items-center justify-center p-6 opacity-0 transition-opacity duration-300";
    overlay.innerHTML = `
      <div class="w-full max-w-[360px] bg-white rounded-[28px] p-8 shadow-2xl scale-95 transition-transform duration-300" id="modal-panel">
        ${contentHTML}
      </div>
    `;
    overlay.addEventListener("click", (e) => {
      if (e.target === overlay) Modal.hide();
    });
    document.body.appendChild(overlay);
  },

  hide() {
    const overlay = document.getElementById("modal-overlay");
    if (overlay) overlay.remove();
  },
};

window.Modal = Modal;

// ── Confirm Dialog ───────────────────────────────────────────────
function confirmAction(message, onConfirm) {
  window.lastModalConfirmAction = onConfirm;
  Modal.show(`
    <div class="text-center">
      <div class="w-16 h-16 rounded-full bg-error/15 flex items-center justify-center mx-auto mb-4">
        <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="#FF4757" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <triangle points="10.29 3.86L1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z"/>
          <line x1="12" y1="9" x2="12" y2="13"/>
          <line x1="12" y1="17" x2="12.01" y2="17"/>
        </svg>
      </div>
      <p class="text-sm text-white/70 leading-relaxed mb-6">${message}</p>
      <div class="flex gap-3">
        <button onclick="Modal.hide()" class="flex-1 bg-surface-2 border border-white/8 rounded-btn h-11 text-sm font-semibold">Cancel</button>
        <button onclick="window.lastModalConfirmAction(); Modal.hide();" class="flex-1 bg-error/15 text-error border border-error/20 rounded-btn h-11 text-sm font-bold">Confirm</button>
      </div>
    </div>
  `);
}

window.confirmAction = confirmAction;

// ── Coin picker helper ───────────────────────────────────────────
function showCoinPickerSheet(onSelect) {
  const coins = window.AppData ? window.AppData.coins : [];

  const content = `
    <div class="relative mb-6">
      <svg class="absolute left-4 top-1/2 -translate-y-1/2 text-gray-400" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/></svg>
      <input type="text" id="coin-picker-search" placeholder="Search asset name or symbol" class="w-full bg-gray-50 border-none rounded-2xl h-14 pl-12 pr-4 text-sm font-bold focus:ring-2 focus:ring-[#00D4AA]/20 transition-all outline-none" oninput="window.filterCoinPickerListing(this.value)">
    </div>
    <div id="coin-picker-list" class="flex flex-col gap-1">
      ${renderCoinPickerItems(coins, onSelect)}
    </div>
  `;

  window.lastCoinSelectCallback = onSelect;
  BottomSheet.show(content, "Select Asset");
}

function renderCoinPickerItems(coins, onSelect) {
  return coins
    .map(
      (c) => `
    <div class="flex items-center gap-4 p-4 rounded-2xl hover:bg-gray-50 active:bg-gray-100 transition-colors cursor-pointer group" onclick="window.lastCoinSelectCallback('${c.id}'); BottomSheet.hide();">
       <div class="w-11 h-11 rounded-[16px] flex items-center justify-center text-[10px] font-black shadow-sm" style="background:${c.color}15; color:${c.color}">${c.symbol.substring(0, 3)}</div>
       <div class="flex-1 min-w-0">
         <div class="text-[15px] font-black text-[#0f172a] group-hover:text-[#00D4AA] transition-colors">${c.name}</div>
         <div class="text-[11px] text-[#94a3b8] font-bold">${c.symbol}</div>
       </div>
       <div class="text-right">
         <div class="text-sm font-black text-[#0f172a]">${window.AppData.formatPrice(c.price)}</div>
         <div class="text-[11px] ${c.change24h >= 0 ? "text-[#00C896]" : "text-[#FF4757]"} font-bold">${window.AppData.formatChange(c.change24h)}</div>
       </div>
    </div>
  `,
    )
    .join("");
}

window.filterCoinPickerListing = function (filter) {
  const coins = window.AppData ? window.AppData.coins : [];
  const filtered = coins.filter(
    (c) =>
      c.name.toLowerCase().includes(filter.toLowerCase()) ||
      c.symbol.toLowerCase().includes(filter.toLowerCase()),
  );
  const container = document.getElementById("coin-picker-list");
  if (container) {
    container.innerHTML = renderCoinPickerItems(
      filtered,
      window.lastCoinSelectCallback,
    );
  }
};

window.showCoinPickerSheet = showCoinPickerSheet;

// ── Global Switch Toggle ─────────────────────────────────────────
document.addEventListener('DOMContentLoaded', () => {
  const switches = document.querySelectorAll('.w-12.h-6.rounded-pill.cursor-pointer');
  switches.forEach(sw => {
    // Add transition classes for smooth animation
    sw.classList.add('transition-colors', 'duration-200');
    const knob = sw.querySelector('.w-5.h-5');
    if (knob) {
      knob.classList.add('transition-all', 'duration-200');
    }

    sw.addEventListener('click', () => {
      const knob = sw.querySelector('.w-5.h-5');
      if (!knob) return;
      
      const isOn = sw.classList.contains('bg-primary');
      if (isOn) {
        // Turn off
        sw.classList.remove('bg-primary');
        sw.classList.add('bg-white/20');
        knob.classList.remove('right-0.5');
        knob.classList.add('left-0.5');
      } else {
        // Turn on
        sw.classList.remove('bg-white/20');
        sw.classList.add('bg-primary');
        knob.classList.remove('left-0.5');
        knob.classList.add('right-0.5');
      }
    });
  });
});
