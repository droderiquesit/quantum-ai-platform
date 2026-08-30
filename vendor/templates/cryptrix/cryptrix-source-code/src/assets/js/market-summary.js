document.addEventListener("DOMContentLoaded", () => {
  if (!window.AppData || !AppData.coins) return;
  const gList = [...AppData.coins]
    .sort((a, b) => b.change24h - a.change24h)
    .slice(0, 4);
  const lList = [...AppData.coins]
    .sort((a, b) => a.change24h - b.change24h)
    .slice(0, 4);

  const renderItem = (c, isGainer) => `
          <a href="coin-detail.html?id=${c.id}" class="flex items-center justify-between py-3 border-b border-white/5 last:border-0 hover:bg-white/[0.02] cursor-pointer">
            <div class="flex items-center gap-3">
              <div class="w-8 h-8 rounded-lg flex items-center justify-center text-[10px] font-black" style="background:${c.color}22;color:${c.color}">${c.symbol.substring(0, 3)}</div>
              <div>
                <div class="text-[13px] font-bold">${c.symbol.toUpperCase()}</div>
                <div class="text-[11px] text-white/40">${AppData.formatPrice(c.price)}</div>
              </div>
            </div>
            <div class="text-[12px] font-bold px-2 py-0.5 rounded ${isGainer ? "bg-success/15 text-success" : "bg-error/15 text-error"}">
              ${isGainer ? "▲" : "▼"} ${Math.abs(c.change24h).toFixed(1)}%
            </div>
          </a>`;

  const gainersEl = document.getElementById("gainers");
  const losersEl = document.getElementById("losers");
  if (gainersEl)
    gainersEl.innerHTML = gList.map((c) => renderItem(c, true)).join("");
  if (losersEl)
    losersEl.innerHTML = lList.map((c) => renderItem(c, false)).join("");
});
