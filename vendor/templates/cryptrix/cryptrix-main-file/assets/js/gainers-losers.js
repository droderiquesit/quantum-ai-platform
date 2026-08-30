let activeTab = "gainers";
const gainers = [...AppData.coins]
  .sort((a, b) => b.change24h - a.change24h)
  .slice(0, 8);
const losers = [...AppData.coins]
  .sort((a, b) => a.change24h - b.change24h)
  .slice(0, 8);

function renderList(list, isGainers) {
  return list
    .map(
      (coin, i) => `
        <a href="coin-detail.html?id=${coin.id}" class="flex items-center gap-3 py-3.5 border-b border-white/5 last:border-0 cursor-pointer hover:bg-white/[0.02] transition-colors">
          <div class="text-[16px] font-black text-white/20 w-6 text-center flex-shrink-0">${i + 1}</div>
          <div class="w-11 h-11 rounded-[14px] flex items-center justify-center text-[11px] font-black flex-shrink-0" style="background:${coin.color}22;color:${coin.color}">${coin.symbol.substring(0, 3)}</div>
          <div class="flex-1 min-w-0">
            <div class="text-[14px] font-bold">${coin.name}</div>
            <div class="text-[12px] text-white/40">${AppData.formatLargeNum(coin.marketCap)}</div>
          </div>
          <div class="text-end flex-shrink-0">
            <div class="text-[14px] font-bold">${AppData.formatPrice(coin.price)}</div>
            <span class="${isGainers ? "bg-success/15 text-success" : "bg-error/15 text-error"} text-xs font-bold px-2 py-0.5 rounded-full">${isGainers ? "▲" : "▼"} ${Math.abs(coin.change24h).toFixed(2)}%</span>
          </div>
        </a>`,
    )
    .join("");
}

function switchTab(tab) {
  activeTab = tab;
  const gainBtn = document.getElementById("tab-gainers");
  const loseBtn = document.getElementById("tab-losers");
  gainBtn.className =
    tab === "gainers"
      ? "flex-1 h-10 rounded-btn bg-success text-bg-dark font-bold text-sm transition-all"
      : "flex-1 h-10 rounded-btn text-white/50 font-semibold text-sm transition-all";
  loseBtn.className =
    tab === "losers"
      ? "flex-1 h-10 rounded-btn bg-error text-white font-bold text-sm transition-all"
      : "flex-1 h-10 rounded-btn text-white/50 font-semibold text-sm transition-all";
  document.getElementById("gl-list").innerHTML = renderList(
    tab === "gainers" ? gainers : losers,
    tab === "gainers",
  );
}

document.addEventListener("DOMContentLoaded", () => switchTab("gainers"));
