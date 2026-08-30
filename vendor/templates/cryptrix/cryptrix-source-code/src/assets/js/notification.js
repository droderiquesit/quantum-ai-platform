document.addEventListener("DOMContentLoaded", () => {
  const d = window.AppData;
  if (!d) return;
  const el = document.getElementById("notif-list");
  if (!el) return;

  const iconMap = {
    price_alert: '<polyline points="22 7 13.5 15.5 8.5 10.5 1 18"/>',
    trade: '<polyline points="20 6 9 17 4 12"/>',
    ai: '<rect x="2" y="2" width="20" height="8" rx="2"/><rect x="2" y="14" width="20" height="8" rx="2"/>',
    staking:
      '<polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2"/>',
    market: '<polyline points="22 7 13.5 15.5 8.5 10.5 1 18"/>',
    security: '<path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/>',
    kyc: '<path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2"/><circle cx="12" cy="7" r="4"/>',
    deposit:
      '<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/>',
  };
  const classMap = {
    price_alert: "bg-primary/15 text-primary",
    trade: "bg-success/15 text-success",
    ai: "bg-secondary/15 text-secondary",
    staking: "bg-warning/15 text-warning",
    market: "bg-accent/15 text-accent",
    security: "bg-error/15 text-error",
    kyc: "bg-primary/15 text-primary",
    deposit: "bg-primary/15 text-primary",
  };
  el.innerHTML = d.notifications
    .map(
      (n) =>
        '<div class="flex items-start gap-3 py-4 border-b border-white/5' +
        (n.read ? "" : " bg-primary/[0.03]") +
        ' rounded-lg px-2 mb-1 cursor-pointer">' +
        '<div class="w-11 h-11 rounded-[14px] flex items-center justify-center flex-shrink-0 mt-0.5 ' +
        (classMap[n.type] || "bg-surface-3 text-white/50") +
        '"><svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">' +
        (iconMap[n.type] || '<circle cx="12" cy="12" r="4"/>') +
        "</svg></div>" +
        '<div class="flex-1 min-w-0">' +
        '<div class="text-sm font-bold">' +
        n.title +
        "</div>" +
        '<div class="text-xs text-white/50 leading-relaxed mt-0.5">' +
        n.message +
        "</div>" +
        '<div class="text-[11px] text-white/30 mt-1">' +
        n.time +
        "</div>" +
        "</div>" +
        (n.read
          ? ""
          : '<div class="w-2 h-2 rounded-full bg-primary flex-shrink-0 mt-2"></div>') +
        "</div>",
    )
    .join("");
});
