document.addEventListener("DOMContentLoaded", () => {
  const d = window.AppData;
  if (!d) return;
  const el = document.getElementById("wallet-assets");
  if (!el) return;
  const all = [
    ...d.holdings,
    { coin: "USDT", amount: 12450, valueUSD: 12450, pnlPct: 0 },
  ];
  el.innerHTML = all
    .map((h) => {
      const coin = d.getCoin(h.coin);
      let shortName = h.coin.substring(0, 3);
      if (h.coin === 'AVAX') shortName = 'AVA';
      if (h.coin === 'LINK') shortName = 'LIN';
      
      return (
        '<a href="coin-detail.html?id=' +
        (coin?.id || h.coin.toLowerCase()) +
        '" class="flex items-center gap-4 py-[14px] border-b border-white/5 cursor-pointer last:border-0">' +
        '<div class="w-11 h-11 rounded-full flex items-center justify-center text-[11px] font-black flex-shrink-0" style="background:' +
        (coin?.color || "#26A17B") +
        "22;color:" +
        (coin?.color || "#26A17B") +
        '">' +
        shortName +
        "</div>" +
        '<div class="flex-1 min-w-0"><div class="text-[15px] font-bold">' +
        (coin?.name || h.coin) +
        '</div><div class="text-[12px] text-white/50">' +
        h.amount +
        " " +
        h.coin +
        "</div></div>" +
        '<div class="text-right"><div class="text-[15px] font-bold">$' +
        h.valueUSD.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 }) +
        "</div>" +
        (h.pnlPct !== 0
          ? '<div class="text-[12px] font-bold mt-0.5 ' +
            (h.pnlPct >= 0 ? "text-success" : "text-error") +
            '">' +
            ((h.pnlPct >= 0 ? "+" : "") + h.pnlPct) +
            "%</div>"
          : '<div class="text-[12px] text-white/50 mt-0.5">Stable</div>') +
        "</div></a>"
      );
    })
    .join("");
});
