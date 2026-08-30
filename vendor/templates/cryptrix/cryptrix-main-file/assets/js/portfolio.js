const colors = [
  "#00D4AA",
  "#7B61FF",
  "#9945FF",
  "#F3BA2F",
  "#E84142",
  "#2A5ADA",
  "#FF007A",
  "#FF6B35",
];

function setPeriod(btn, period) {
  document.querySelectorAll(".period-btn").forEach((b) => {
    b.classList.remove("bg-primary", "text-bg-dark");
    b.classList.add("text-white/50");
  });
  btn.classList.add("bg-primary", "text-bg-dark");
  btn.classList.remove("text-white/50");
}

document.addEventListener("DOMContentLoaded", () => {
  const d = AppData;
  const p = d.portfolio;

  document.getElementById("port-total").textContent =
    "$" + p.totalValue.toLocaleString("en-US", { minimumFractionDigits: 2 });
  document.getElementById("port-pnl").textContent =
    "+$" + p.totalPnL.toLocaleString("en-US", { minimumFractionDigits: 2 });
  document.getElementById("port-roi").textContent = "+" + p.totalPnLPct + "%";
  document.getElementById("port-today").textContent =
    "+" + p.dayChangePct + "%";

  // Portfolio chart
  Charts.portfolioLine("portfolio-chart", p.performanceHistory7d, "7d");

  // Allocation donut
  Charts.allocationDonut("alloc-donut", d.holdings);

  // Allocation legend
  const legend = document.getElementById("alloc-legend");
  if (legend) {
    legend.innerHTML = d.holdings
      .slice(0, 7)
      .map(
        (h, i) => `
          <div class="flex items-center gap-2">
            <div class="w-2 h-2 rounded-full flex-shrink-0" style="background:${colors[i] || "#888"}"></div>
            <div class="flex-1 min-w-0 text-[13px] font-semibold truncate">${h.coin}</div>
            <div class="text-[13px] font-bold text-white/50">${h.weight}%</div>
          </div>`,
      )
      .join("");
  }

  // Holdings list
  const holdEl = document.getElementById("holdings-list");
  if (holdEl) {
    holdEl.innerHTML = d.holdings
      .map((h, i) => {
        const coin = d.getCoin(h.coin);
        const isPos = (h.pnlPct || 0) >= 0;
        return `<a href="coin-detail.html?id=${coin?.id || h.coin.toLowerCase()}" class="flex items-center gap-3 px-4 py-3.5 border-b border-white/5 last:border-0 cursor-pointer hover:bg-surface-2 transition-colors">
            <div class="w-11 h-11 rounded-[14px] flex items-center justify-center text-[11px] font-black flex-shrink-0" style="background:${coin?.color || colors[i]}22;color:${coin?.color || colors[i]}">${h.coin.substring(0, 3)}</div>
            <div class="flex-1 min-w-0">
              <div class="text-[14px] font-bold truncate">${coin?.name || h.coin}</div>
              <div class="text-[12px] text-white/40">${h.amount} ${h.coin}</div>
            </div>
            <canvas id="holding-chart-${i}" width="56" height="28" class="flex-shrink-0"></canvas>
            <div class="text-right flex-shrink-0 min-w-[80px]">
              <div class="text-[14px] font-extrabold">$${h.valueUSD.toLocaleString("en-US", { maximumFractionDigits: 2 })}</div>
              <div class="text-[12px] font-semibold ${isPos ? "text-success" : "text-error"}">${isPos ? "+" : ""}${h.pnlPct}%</div>
            </div>
          </a>`;
      })
      .join("");
    // Render mini charts
    d.holdings.forEach((h, i) => {
      const coin = d.getCoin(h.coin);
      if (coin)
        setTimeout(
          () =>
            Charts.miniLine(
              "holding-chart-" + i,
              coin.history7d.slice(-10),
              (coin.change24h || 0) >= 0,
            ),
          100 + i * 30,
        );
    });
  }

  // Performance detail chart
  const perfChart = document.getElementById("perf-chart");
  if (perfChart) {
    Charts.portfolioLine("perf-chart", d.portfolio.performanceHistory90d, "90d");
  }
});


