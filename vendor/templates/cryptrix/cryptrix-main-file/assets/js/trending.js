document.addEventListener("DOMContentLoaded", () => {
  const d = window.AppData;
  const trending = [...d.coins]
    .sort((a, b) => Math.abs(b.changePct7d) - Math.abs(a.changePct7d))
    .slice(0, 8);
  const list = document.getElementById("trending-list");
  if (list) {
    list.innerHTML = trending
      .map((coin, i) => {
        const isPos = coin.change24h >= 0;
        return `<a href="coin-detail.html?id=${coin.id}" class="flex items-center gap-3 py-3.5 border-b border-white/5 last:border-0 cursor-pointer hover:bg-white/[0.02] rounded-lg px-2 transition-colors -mx-2">
            <div class="text-[18px] font-black text-white/20 w-6 text-center flex-shrink-0">${i + 1}</div>
            <div class="w-11 h-11 rounded-[14px] flex items-center justify-center text-[11px] font-black flex-shrink-0" style="background:${coin.color}22;color:${coin.color}">${coin.symbol.substring(0, 3)}</div>
            <div class="flex-1 min-w-0">
              <div class="text-[14px] font-bold">${coin.name}</div>
              <div class="text-[12px] text-white/40">${window.AppData.formatPrice(coin.price)}</div>
            </div>
            <canvas id="trend-chart-${i}" width="64" height="28" class="flex-shrink-0"></canvas>
            <div class="text-right flex-shrink-0">
              <div class="text-[13px] font-bold ${isPos ? "text-success" : "text-error"}">${window.AppData.formatChange(coin.change24h)}</div>
              <div class="text-[11px] text-white/40">7D: <span class="${coin.changePct7d >= 0 ? "text-success" : "text-error"}">${window.AppData.formatChange(coin.changePct7d)}</span></div>
            </div>
          </a>`;
      })
      .join("");
    trending.forEach((coin, i) =>
      setTimeout(
        () =>
          window.Charts.miniLine(
            "trend-chart-" + i,
            coin.history7d.slice(-12),
            coin.change24h >= 0,
          ),
        100 + i * 30,
      ),
    );
  }
});
