import "../css/index.css";

if ("serviceWorker" in navigator) {
  window.addEventListener("load", () => {
    navigator.serviceWorker
      .register("assets/js/service-worker.js")
      .then((reg) => {
        console.log("SW Registered:", reg.scope);
      })
      .catch((err) => {
        console.warn("SW Registration failed:", err);
      });
  });
}

// bottom nav
const currentPath = window.location.pathname.split("/").pop() || "index.html";
const navMap = {
  "home.html": "nav-home",
  "market.html": "nav-market",
  "buy-crypto.html": "nav-trade",
  "portfolio-overview.html": "nav-portfolio",
  "profile.html": "nav-profile",
  "app-settings.html": "nav-profile"
};
// Add primary highlight based on current page
const activeId = navMap[currentPath];
if (activeId) {
  const activeEl = document.getElementById(activeId);
  if (activeEl) {
    activeEl.classList.add("text-primary");
    activeEl.classList.remove("text-white/40", "text-black");
  }
}

document.addEventListener("DOMContentLoaded", () => {
  const d = window.AppData;
  if (!d) return;
  // Check for chart el before init
  const chartEl = document.getElementById("home-chart");
  if (chartEl)
    Charts.portfolioLine("home-chart", d.portfolio.performanceHistory7d);
  // Coin list
  const coinsEl = document.getElementById("home-coins");
  if (coinsEl) {
    coinsEl.innerHTML = d.coins
      .slice(0, 8)
      .map(
        (c, i) => `
            <a href="coin-detail.html?id=${c.id}" class="flex items-center gap-3 py-3 border-b border-white/5 cursor-pointer">
              <div class="w-11 h-11 rounded-[14px] flex items-center justify-center text-[11px] font-black flex-shrink-0" style="background:${c.color}22;color:${c.color}">${c.symbol.substring(0, 3)}</div>
              <div class="flex-1 min-w-0">
                <div class="text-sm font-bold truncate">${c.name}</div>
                <div class="text-xs text-white/40">${c.symbol} · #${c.rank}</div>
              </div>
              <canvas id="hc-${i}" width="72" height="32" class="flex-shrink-0"></canvas>
              <div class="text-right flex-shrink-0">
                <div class="text-sm font-bold">${d.formatPrice(c.price)}</div>
                <div class="text-xs ${c.change24h >= 0 ? "text-success" : "text-error"}">${d.formatChange(c.change24h)}</div>
              </div>
            </a>`,
      )
      .join("");
    d.coins
      .slice(0, 8)
      .forEach((c, i) =>
        setTimeout(
          () =>
            Charts.miniLine(
              `hc-${i}`,
              c.history7d.slice(-15),
              c.change24h >= 0,
            ),
          150 + i * 30,
        ),
      );
  }
  // Holdings
  const holdEl = document.getElementById("home-holdings");
  if (holdEl) {
    holdEl.innerHTML = d.holdings
      .slice(0, 4)
      .map((h) => {
        const coin = d.getCoin(h.coin);
        return `<div class="flex items-center gap-3 py-3 border-b border-white/5 last:border-b-0">
              <div class="w-10 h-10 rounded-full flex items-center justify-center text-[10px] font-black flex-shrink-0" style="background:${coin?.color || "#888"}22;color:${coin?.color || "#888"}">${h.coin.substring(0, 3)}</div>
              <div class="w-[85px] flex-shrink-0 text-start">
                <div class="text-sm font-bold leading-tight">${h.coin}</div>
                <div class="text-xs text-white/40 mt-0.5">${h.amount} units</div>
              </div>
              <div class="flex-1 px-2 text-start">
                <div class="h-1.5 w-full bg-white/10 rounded-full overflow-hidden flex">
                  <div class="h-full rounded-full" style="width: ${h.weight}%; background-color: ${coin?.color || "#888"}"></div>
                </div>
                <div class="text-[11px] text-white/40 mt-1">${h.weight}%</div>
              </div>
              <div class="text-end flex-shrink-0">
                <div class="text-sm font-bold leading-tight">$${h.valueUSD.toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 })}</div>
                <div class="text-xs ${h.pnlPct >= 0 ? "text-success" : "text-error"} mt-0.5">${h.pnlPct >= 0 ? "+" : ""}${h.pnlPct}%</div>
              </div>
            </div>`;
      })
      .join("");
  }

  // ── AI Signals Mini (Advisor Page) ──
  function renderAISignalCard(rec) {
    const isBuy = rec.type === 'buy';
    const isSell = rec.type === 'sell';
    const typeColor = isBuy ? 'text-success' : (isSell ? 'text-error' : 'text-warning');
    const typeBg = isBuy ? 'bg-success/15' : (isSell ? 'bg-error/15' : 'bg-warning/15');
    
    return `
      <div class="bg-surface border border-white/5 rounded-[20px] p-4 mb-3 animate-fade-in shadow-sm hover:border-white/10 transition-colors">
        <div class="flex items-start justify-between mb-3">
          <div class="flex items-center gap-3">
            <div class="w-10 h-10 rounded-xl ${typeBg} flex items-center justify-center flex-shrink-0">
              <span class="${typeColor} font-bold text-sm uppercase">${rec.type}</span>
            </div>
            <div>
              <div class="text-[15px] font-bold">${rec.coin}</div>
              <div class="text-[11px] text-white/40 uppercase tracking-wider mt-0.5">${rec.action}</div>
            </div>
          </div>
          <div class="text-right">
            <div class="text-[15px] font-bold text-primary">${rec.confidence}%</div>
            <div class="text-[10px] text-white/40">Confidence</div>
          </div>
        </div>
        
        <div class="text-[13px] text-white/70 leading-relaxed mb-4">
          ${rec.reason}
        </div>
        
        <div class="grid grid-cols-3 gap-2 bg-surface-2 rounded-xl p-3 mb-4">
          <div class="text-center">
            <div class="text-[10px] text-white/40 uppercase mb-1">Target</div>
            <div class="text-[13px] font-bold">$${rec.targetPrice}</div>
          </div>
          <div class="text-center border-x border-white/5">
            <div class="text-[10px] text-white/40 uppercase mb-1">Stop Loss</div>
            <div class="text-[13px] font-bold">$${rec.stopLoss}</div>
          </div>
          <div class="text-center">
            <div class="text-[10px] text-white/40 uppercase mb-1">Timeframe</div>
            <div class="text-[13px] font-bold">${rec.timeframe}</div>
          </div>
        </div>
        
        <a href="trade.html?coin=${rec.coin}&action=${rec.type}" class="w-full h-11 bg-primary text-bg-dark rounded-btn font-bold text-[14px] flex items-center justify-center cursor-pointer hover:opacity-90 transition-opacity">
          ${isBuy ? 'Buy' : (isSell ? 'Sell' : 'Trade')} ${rec.coin}
        </a>
      </div>
    `;
  }

  const aiSignals = document.getElementById("ai-signals");
  if (aiSignals && d.aiRecommendations) {
    aiSignals.innerHTML = d.aiRecommendations.slice(0, 3).map(rec => renderAISignalCard(rec)).join('');
  }

  // ── AI Suggestions List & Filters ──
  const signalsList = document.getElementById("signals-list");
  const signalFilters = document.querySelectorAll('.signal-filter-btn');
  
  if (signalsList && d.aiRecommendations) {
    const renderFiltered = (filter) => {
      const filtered = filter === 'all' 
        ? d.aiRecommendations 
        : d.aiRecommendations.filter(r => r.type === filter);
        
      signalsList.innerHTML = filtered.length > 0 
        ? filtered.map(renderAISignalCard).join('')
        : '<div class="text-center py-10 text-white/40 text-sm">No signals found for this category</div>';
      
      signalFilters.forEach(btn => {
        if (btn.dataset.filter === filter) {
          btn.className = "signal-filter-btn px-4 py-1.5 text-xs font-semibold rounded-pill cursor-pointer whitespace-nowrap border bg-primary text-bg-dark border-primary transition-colors";
        } else {
          btn.className = "signal-filter-btn px-4 py-1.5 text-xs font-semibold rounded-pill cursor-pointer whitespace-nowrap border bg-surface-2 text-white/60 border-white/10 transition-colors";
        }
      });
    };
    
    if (signalFilters.length > 0) {
      signalFilters.forEach(btn => {
        btn.addEventListener('click', () => {
          renderFiltered(btn.dataset.filter);
        });
      });
    }
    renderFiltered('all');
  }

  // ── Asset Allocation Page ──
  const allocBig = document.getElementById("alloc-big");
  const allocList = document.getElementById("alloc-list");
  if (allocBig || allocList) {
    const colors = [
      "#00D4AA",
      "#7B61FF",
      "#9945FF",
      "#F3BA2F",
      "#E84142",
      "#2A5ADA",
      "#FF007A",
    ];

    if (allocBig && d.holdings && window.Charts) {
      Charts.allocationDonut("alloc-big", d.holdings);
    }
    
    if (allocList && d.holdings) {
      allocList.innerHTML = d.holdings
        .map((h, i) => {
          const coin = d.getCoin(h.coin);
          return `<div class="flex items-center gap-3 px-4 py-3.5 border-b border-white/5 last:border-0 hover:bg-white/5 transition-colors cursor-pointer">
            <div class="w-3 h-3 rounded-full flex-shrink-0" style="background:${colors[i] || "#888"}"></div>
            <div class="w-8 h-8 rounded-xl flex items-center justify-center text-[9px] font-black flex-shrink-0" style="background:${coin?.color || "#888"}22;color:${coin?.color || "#888"}">${h.coin.substring(0, 3)}</div>
            <div class="flex-1 min-w-0">
              <div class="text-[14px] font-bold text-white">${coin?.name || h.coin}</div>
              <div class="text-[12px] text-white/40">${h.amount} ${h.coin}</div>
            </div>
            <div class="text-center w-14">
              <div class="text-[14px] font-extrabold" style="color:${colors[i] || "#888"}">${h.weight}%</div>
              <div class="mt-1.5 h-1 rounded-full bg-white/10 overflow-hidden"><div class="h-full rounded-full" style="width:${Math.min(h.weight * 2.5, 100)}%;background:${colors[i] || "#888"}"></div></div>
            </div>
            <div class="text-right flex-shrink-0 min-w-[70px]">
              <div class="text-[13px] font-bold text-white">$${h.valueUSD.toLocaleString("en-US", { maximumFractionDigits: 0 })}</div>
            </div>
          </div>`;
        })
        .join("");
    }
  }

  // ── Asset News & Filters ──
  const newsList = document.getElementById("news-list");
  const newsFilters = document.querySelectorAll(".news-filter-btn");
  
  if (newsList && d.assetNews) {
    const renderNews = (filter) => {
      const filtered = filter === "all" 
        ? d.assetNews 
        : d.assetNews.filter(n => n.sentiment === filter);
        
      newsList.innerHTML = filtered.length > 0 
        ? filtered.map(news => {
            const sentimentColors = {
              positive: "text-success",
              negative: "text-error",
              neutral: "text-warning"
            };
            const sc = sentimentColors[news.sentiment] || "text-white/40";
            return `
              <a href="${news.url}" class="block bg-surface border border-white/5 rounded-[20px] p-4 hover:border-white/10 transition-colors animate-fade-in shadow-sm">
                <div class="flex items-start justify-between mb-2">
                  <div class="text-[11px] font-semibold text-white/50">${news.source}</div>
                  <div class="text-[10px] text-white/30">${news.time}</div>
                </div>
                <div class="text-[14px] font-bold leading-snug mb-3 text-white">${news.title}</div>
                <div class="flex items-center justify-between">
                  <div class="text-[11px] font-semibold ${sc} uppercase tracking-wider">${news.sentiment}</div>
                  <div class="w-6 h-6 rounded-full bg-white/5 flex items-center justify-center">
                    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" class="text-white/40"><line x1="5" y1="12" x2="19" y2="12"></line><polyline points="12 5 19 12 12 19"></polyline></svg>
                  </div>
                </div>
              </a>
            `;
          }).join('')
        : '<div class="text-center py-10 text-white/40 text-sm">No news found for this category</div>';
      
      newsFilters.forEach(btn => {
        if (btn.dataset.filter === filter) {
          btn.className = "news-filter-btn px-4 py-1.5 text-xs font-semibold rounded-pill cursor-pointer whitespace-nowrap border bg-primary text-bg-dark border-primary transition-colors";
        } else {
          btn.className = "news-filter-btn px-4 py-1.5 text-xs font-semibold rounded-pill cursor-pointer whitespace-nowrap border bg-surface-2 text-white/60 border-white/10 transition-colors";
        }
      });
    };
    
    if (newsFilters.length > 0) {
      newsFilters.forEach(btn => {
        btn.addEventListener('click', () => {
          renderNews(btn.dataset.filter);
        });
      });
    }
    renderNews('all');
  }

  // ── Balance Page ──
  const balanceChart = document.getElementById("balance-chart");
  if (balanceChart && d.portfolio && window.Charts) {
    Charts.portfolioLine(
      "balance-chart",
      d.portfolio.performanceHistory30d,
      "30d"
    );
  }
  const balanceHoldings = document.getElementById("balance-holdings");
  if (balanceHoldings && d.holdings) {
    balanceHoldings.innerHTML = d.holdings
      .map((h) => {
        const coin = d.getCoin(h.coin);
        return `<div class="flex items-center gap-3 py-3.5 border-b border-white/5 last:border-0 hover:bg-white/5 transition-colors cursor-pointer rounded-lg px-2 -mx-2">
      <div class="w-11 h-11 rounded-[14px] flex items-center justify-center text-[11px] font-black flex-shrink-0" style="background:${coin?.color || "#888"}22;color:${coin?.color || "#888"}">${h.coin.substring(0, 3)}</div>
      <div class="flex-1 min-w-0"><div class="text-sm font-bold text-white">${coin?.name || h.coin}</div><div class="text-xs text-white/40">${h.amount} ${h.coin}</div></div>
      <div class="text-right"><div class="text-sm font-bold text-white">$${h.valueUSD.toLocaleString("en-US", { maximumFractionDigits: 2 })}</div><div class="text-xs ${h.pnlPct >= 0 ? "text-success" : "text-error"}">${h.pnlPct >= 0 ? "+" : ""}${h.pnlPct}%</div></div>
    </div>`;
      })
      .join("");
  }

  // ── Biometric Auth Page ──
  const bioBtnFaceId = document.getElementById("bio-btn-faceid");
  const bioBtnFp = document.getElementById("bio-btn-fp");
  const bioTitle = document.getElementById("bio-title");
  const bioSubtitle = document.getElementById("bio-subtitle");
  const bioCircle = document.getElementById("bio-circle");
  const bioBtn = document.getElementById("bio-btn");
  const bioStatus = document.getElementById("bio-status");

  if (bioBtnFaceId && bioBtnFp && bioCircle && bioBtn) {
    let scanning = false;
    
    const setBioMethod = (method) => {
      const isface = method === "faceid";
      bioBtnFaceId.className = "px-5 h-10 rounded-[10px] text-sm font-bold transition-colors " +
        (isface ? "bg-primary text-bg-dark" : "text-white/50");
      bioBtnFp.className = "px-5 h-10 rounded-[10px] text-sm font-semibold transition-colors " +
        (!isface ? "bg-primary text-bg-dark" : "text-white/50");
      
      bioTitle.textContent = isface ? "Face ID" : "Fingerprint";
      bioSubtitle.textContent = isface 
        ? "Look at your camera to authenticate securely" 
        : "Place your finger on the sensor to unlock";
    };

    const triggerBiometric = () => {
      if (scanning) return;
      scanning = true;
      
      if (bioStatus) {
        bioStatus.innerHTML = '<div class="w-2 h-2 rounded-full bg-primary animate-pulse"></div> <span class="text-white">Scanning...</span>';
      }
      
      bioBtn.textContent = "Scanning...";
      bioBtn.style.opacity = "0.7";
      
      setTimeout(() => {
        if (bioStatus) {
          bioStatus.innerHTML = '<div class="w-2 h-2 rounded-full bg-success"></div><span class="text-success font-bold">Authenticated!</span>';
        }
        if (window.showToast) window.showToast("✓ Biometric authentication successful", "success");
        setTimeout(() => {
          window.location.href = "home.html";
        }, 800);
      }, 1800);
    };

    bioBtnFaceId.addEventListener('click', () => setBioMethod('faceid'));
    bioBtnFp.addEventListener('click', () => setBioMethod('fingerprint'));
      bioCircle.addEventListener('click', triggerBiometric);
    bioBtn.addEventListener('click', triggerBiometric);
  }

  // ── Claim Rewards Page ──
  const claimBtn = document.getElementById("claim-btn");
  const autoCompoundBtn = document.getElementById("auto-compound-btn");
  if (claimBtn) {
    claimBtn.addEventListener("click", () => {
      if (window.showToast) window.showToast('Rewards claimed successfully!', 'success');
      setTimeout(() => {
        window.location.href = 'staking-dashboard.html';
      }, 1500);
    });
  }
  if (autoCompoundBtn) {
    autoCompoundBtn.addEventListener("click", () => {
      if (window.showToast) window.showToast('Auto-compound enabled!', 'success');
    });
  }

  // ── Coin Detail Page ──
  const coinHeaderName = document.getElementById("coin-header-name");
  if (coinHeaderName && d.coins) {
    const params = new URLSearchParams(window.location.search);
    const id = params.get("id") || "btc";
    const coin = d.getCoinById(id) || d.coins[0];
    
    if (coin) {
      const pos = coin.change24h >= 0;
      
      const hIcon = document.getElementById("coin-header-icon");
      if (hIcon) {
        hIcon.textContent = coin.symbol.substring(0, 3);
        hIcon.style.background = coin.color + "22";
        hIcon.style.color = coin.color;
        hIcon.classList.remove('animate-pulse');
      }
      
      coinHeaderName.textContent = coin.name;
      
      const hSub = document.getElementById("coin-header-sub");
      if (hSub) hSub.textContent = coin.symbol + " · Rank #" + coin.rank;
      
      const hPrice = document.getElementById("coin-price");
      if (hPrice) hPrice.textContent = d.formatPrice(coin.price);
      
      const badge = document.getElementById("coin-badge");
      if (badge) {
        badge.className = `${pos ? "bg-success/15 text-success" : "bg-error/15 text-error"} text-xs font-bold px-2 py-0.5 rounded-full`;
        badge.textContent = (pos ? "▲ " : "▼ ") + Math.abs(coin.change24h).toFixed(2) + "% (24h)";
      }
      
      const coinHl = document.getElementById("coin-hl");
      if (coinHl) coinHl.textContent = "H: " + d.formatPrice(coin.high24h) + " · L: " + d.formatPrice(coin.low24h);
      
      const coinMcap = document.getElementById("coin-mcap");
      if (coinMcap) coinMcap.textContent = d.formatLargeNum(coin.marketCap);
      
      const coinVol = document.getElementById("coin-vol");
      if (coinVol) coinVol.textContent = d.formatLargeNum(coin.volume24h);
      
      const coinAth = document.getElementById("coin-ath");
      if (coinAth) coinAth.textContent = d.formatPrice(coin.ath);
      
      const coinSupply = document.getElementById("coin-supply");
      if (coinSupply) coinSupply.textContent = d.formatLargeNum(coin.supply);
      
      const coinDesc = document.getElementById("coin-desc");
      if (coinDesc) coinDesc.textContent = coin.description;
      
      setTimeout(() => {
        if (window.Charts && document.getElementById("coin-chart")) {
          Charts.coinLine("coin-chart", coin.history30d, coin.color, pos);
        }
      }, 100);
    }
    
    // Period buttons
    const periodBtns = document.querySelectorAll(".period-btn");
    if (periodBtns.length > 0) {
      periodBtns.forEach(btn => {
        btn.addEventListener("click", function() {
          this.parentElement.querySelectorAll(".period-btn").forEach(b => {
            b.className = "period-btn px-2.5 py-1.5 text-xs font-semibold rounded-lg text-white/40 transition-colors";
          });
          this.className = "period-btn px-2.5 py-1.5 text-xs font-semibold rounded-lg bg-primary text-bg-dark transition-colors";
        });
      });
    }

    // Watchlist toast
    const wlBtn = document.getElementById("wl-btn");
    if (wlBtn) {
      wlBtn.addEventListener("click", () => {
        if (window.showToast) window.showToast('Added to watchlist', 'success');
      });
    }
  }

  // ── Contact Page ──
  const btnLiveChat = document.getElementById("btn-live-chat");
  const btnEmail = document.getElementById("btn-email");
  const btnAttach = document.getElementById("btn-attach");
  const btnSend = document.getElementById("btn-send");

  if (btnLiveChat) {
    btnLiveChat.addEventListener("click", () => {
      if (window.showToast) window.showToast('Chat started', 'success');
    });
  }
  if (btnEmail) {
    btnEmail.addEventListener("click", () => {
      if (window.showToast) window.showToast('Email opened', 'info');
    });
  }
  if (btnAttach) {
    btnAttach.addEventListener("click", () => {
      if (window.showToast) window.showToast('File selected', 'success');
    });
  }
  if (btnSend) {
    btnSend.addEventListener("click", () => {
      if (window.showToast) window.showToast("Message sent! We'll respond within 24 hours.", 'success');
    });
  }

  // ── Deposit Page ──
  const btnCopyAddress = document.getElementById("btn-copy-address");
  if (btnCopyAddress) {
    btnCopyAddress.addEventListener("click", function() {
      const address = this.previousElementSibling.textContent.trim();
      navigator.clipboard.writeText(address).then(() => {
        if (window.showToast) window.showToast('Address copied to clipboard', 'success');
      });
    });
  }

  // ── Edit Profile Page ──
  const btnChangePhoto = document.getElementById("btn-change-photo");
  const btnChangePhotoText = document.getElementById("btn-change-photo-text");
  const btnSaveProfile = document.getElementById("btn-save-profile");

  const simulatePhotoUpload = () => {
    if (window.showToast) window.showToast('Photo uploaded', 'success');
  };

  if (btnChangePhoto) btnChangePhoto.addEventListener("click", simulatePhotoUpload);
  if (btnChangePhotoText) btnChangePhotoText.addEventListener("click", simulatePhotoUpload);

  if (btnSaveProfile) {
    btnSaveProfile.addEventListener("click", () => {
      if (window.showToast) window.showToast('Profile updated!', 'success');
    });
  }

  // ── Error State Page ──
  const btnTryAgain = document.getElementById("btn-try-again");
  if (btnTryAgain) {
    btnTryAgain.addEventListener("click", () => {
      window.history.back();
    });
  }

  // ── FAQ Page ──
  const faqList = document.getElementById("faq-list");
  if (faqList && d.faq) {
    faqList.innerHTML = d.faq.map((item, i) => `
      <div class="mb-3">
        <button class="w-full bg-surface-2 border border-white/10 rounded-xl p-4 flex items-center justify-between text-left transition-colors hover:bg-white/5 active:scale-[0.99] faq-toggle" data-index="${i}">
          <span class="text-sm font-semibold pr-4 text-white">${item.q}</span>
          <svg class="flex-shrink-0 transition-transform duration-200 faq-icon-${i} text-white/50" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
            <polyline points="6 9 12 15 18 9" />
          </svg>
        </button>
        <div class="overflow-hidden transition-all duration-300 max-h-0 faq-content-${i}">
          <div class="p-4 text-sm text-white/50 leading-relaxed border-x border-b border-white/5 rounded-b-xl bg-surface">
            ${item.a}
          </div>
        </div>
      </div>
    `).join('');

    document.querySelectorAll(".faq-toggle").forEach(btn => {
      btn.addEventListener("click", function() {
        const i = this.getAttribute("data-index");
        const content = document.querySelector(`.faq-content-${i}`);
        const icon = document.querySelector(`.faq-icon-${i}`);
        const isOpen = content.style.maxHeight && content.style.maxHeight !== '0px';
        
        // Optional: Accordion behavior (Close others)
        document.querySelectorAll("[class*='faq-content-']").forEach(c => {
          if (c !== content) c.style.maxHeight = '0px';
        });
        document.querySelectorAll("[class*='faq-icon-']").forEach(ic => {
          if (ic !== icon) ic.style.transform = 'rotate(0deg)';
        });
        
        if (!isOpen) {
          content.style.maxHeight = content.scrollHeight + "px";
          icon.style.transform = 'rotate(180deg)';
        } else {
          content.style.maxHeight = '0px';
          icon.style.transform = 'rotate(0deg)';
        }
      });
    });
  }

  // ── Forgot Password Page ──
  const btnBackForgot = document.getElementById("btn-back-forgot");
  if (btnBackForgot) {
    btnBackForgot.addEventListener("click", () => {
      window.history.back();
    });
  }

});
