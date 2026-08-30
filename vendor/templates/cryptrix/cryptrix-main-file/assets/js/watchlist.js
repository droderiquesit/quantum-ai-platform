document.addEventListener("DOMContentLoaded", () => {
  const d = window.AppData;
  if (!d) return;

  // Render main watchlist
  const el = document.getElementById("watchlist-items");
  if (el) {
    el.innerHTML = d.watchlist
      .map((w, i) => {
        const coin = d.getCoin(w.coin);
        if (!coin) return "";

        let shortName = coin.symbol.substring(0, 3);
        if (coin.symbol === "AVAX") shortName = "AVA";
        if (coin.symbol === "LINK") shortName = "LIN";

        const formatVal = (val) =>
          "$" +
          val.toLocaleString("en-US", {
            minimumFractionDigits: 2,
            maximumFractionDigits: 2,
          });

        return (
          '<div class="bg-surface border border-white/5 rounded-2xl p-4 mb-3">' +
          '<div class="flex items-center gap-3">' +
          '<a href="coin-detail.html?id=' +
          coin.id +
          '" class="w-11 h-11 rounded-full flex items-center justify-center text-[11px] font-black flex-shrink-0" style="background:' +
          coin.color +
          "22;color:" +
          coin.color +
          '">' +
          shortName +
          "</a>" +
          '<a href="coin-detail.html?id=' +
          coin.id +
          '" class="flex-[1.5] min-w-0">' +
          '<div class="text-[15px] font-bold mb-0.5">' +
          coin.name +
          "</div>" +
          '<div class="text-[12px] text-white/50">' +
          coin.symbol +
          " · #" +
          coin.rank +
          "</div>" +
          "</a>" +
          '<div class="flex-1 flex items-center justify-center overflow-visible">' +
          '<canvas id="wl-' +
          i +
          '" width="64" height="28"></canvas>' +
          "</div>" +
          '<div class="text-right flex-shrink-0 pl-2">' +
          '<div class="text-[15px] font-bold mb-0.5">' +
          formatVal(coin.price).replace("$", "$") +
          "</div>" +
          '<div class="text-[12px] font-bold ' +
          (coin.change24h >= 0 ? "text-success" : "text-error") +
          '">' +
          d.formatChange(coin.change24h) +
          "</div>" +
          "</div>" +
          "</div>" +
          (w.alert
            ? '<div class="flex flex-wrap items-center gap-2 mt-4">' +
              (w.alert.above
                ? '<div class="bg-success/10 text-success rounded-md px-2 py-1.5 flex items-center gap-1.5 text-[11px] font-bold"><svg width="8" height="8" viewBox="0 0 24 24" fill="currentColor" stroke="none"><path d="M12 2L2 22h20L12 2z"/></svg>Alert: ' +
                  formatVal(w.alert.above) +
                  "</div>"
                : "") +
              (w.alert.below
                ? '<div class="bg-error/10 text-error rounded-md px-2 py-1.5 flex items-center gap-1.5 text-[11px] font-bold"><svg width="8" height="8" viewBox="0 0 24 24" fill="currentColor" stroke="none"><path d="M12 22L22 2H2L12 22z"/></svg>Alert: ' +
                  formatVal(w.alert.below) +
                  "</div>"
                : "") +
              "</div>"
            : "") +
          "</div>"
        );
      })
      .join("");

    d.watchlist.forEach((w, i) => {
      const coin = d.getCoin(w.coin);
      if (coin)
        setTimeout(
          () =>
            window.Charts.miniLine(
              "wl-" + i,
              coin.history7d.slice(-12),
              coin.change24h >= 0,
            ),
          100 + i * 30,
        );
    });
  }

  // Render Suggested
  const suggEl = document.getElementById("suggested-items");
  if (suggEl) {
    const suggestedCoins = ["ATOM", "NEAR", "ALGO", "XLM"]
      .map((symbol) => d.getCoin(symbol))
      .filter((c) => c);
    suggEl.innerHTML = suggestedCoins
      .map((coin) => {
        let shortName = coin.symbol.substring(0, 3);
        if (coin.symbol === "NEAR") shortName = "NEA";
        if (coin.symbol === "ALGO") shortName = "ALG";

        return (
          '<a href="coin-detail.html?id=' +
          coin.id +
          '" class="w-[100px] flex-shrink-0 bg-surface border border-white/5 rounded-2xl p-4 flex flex-col items-center justify-center transition-opacity hover:opacity-80">' +
          '<div class="w-10 h-10 rounded-full flex items-center justify-center text-[11px] font-black mb-3" style="background:' +
          coin.color +
          "22;color:" +
          coin.color +
          '">' +
          shortName +
          "</div>" +
          '<div class="text-[13px] font-bold mb-1">' +
          coin.symbol +
          "</div>" +
          '<div class="text-[12px] font-bold ' +
          (coin.change24h >= 0 ? "text-success" : "text-error") +
          '">' +
          d.formatChange(coin.change24h) +
          "</div>" +
          "</a>"
        );
      })
      .join("");
  }
});
