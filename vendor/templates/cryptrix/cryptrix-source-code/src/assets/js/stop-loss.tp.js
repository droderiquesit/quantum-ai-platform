(function () {
  var CURRENT = 43900;
  var ENTRY = 41200;
  var QTY = 0.1;

  var slSlider = document.getElementById("sl-slider");
  var slPrice = document.getElementById("sl-price");
  var slInfo = document.getElementById("sl-info");
  var slPct = document.getElementById("sl-pct");

  var tpSlider = document.getElementById("tp-slider");
  var tpPrice = document.getElementById("tp-price");
  var tpInfo = document.getElementById("tp-info");
  var tpPct = document.getElementById("tp-pct");

  var sumRisk = document.getElementById("sum-risk");
  var sumReward = document.getElementById("sum-reward");
  var sumRatio = document.getElementById("sum-ratio");
  var rrBar = document.getElementById("rr-bar");

  var slToggle = document.getElementById("sl-toggle");
  var tpToggle = document.getElementById("tp-toggle");
  var slIsOn = true;
  var tpIsOn = true;

  function updateSL() {
    if (!slIsOn) return;
    var pctVal = slSlider.value / 10;
    var price = Math.round(CURRENT * (1 - pctVal / 100));
    var loss = Math.round(QTY * (CURRENT - price));
    var fillPct =
      ((slSlider.value - slSlider.min) / (slSlider.max - slSlider.min)) * 100;

    slSlider.style.setProperty("--sl-fill", fillPct + "%");
    slPrice.value = price;
    slPct.textContent = "-" + pctVal.toFixed(1) + "%";
    slInfo.textContent =
      "-" + pctVal.toFixed(1) + "% from current · Max loss: -$" + loss;
    updateSummary();
  }

  function updateTP() {
    if (!tpIsOn) return;
    var pctVal = tpSlider.value / 10;
    var price = Math.round(CURRENT * (1 + pctVal / 100));
    var gain = Math.round(QTY * (price - CURRENT));
    var fillPct =
      ((tpSlider.value - tpSlider.min) / (tpSlider.max - tpSlider.min)) * 100;

    tpSlider.style.setProperty("--tp-fill", fillPct + "%");
    tpPrice.value = price;
    tpPct.textContent = "+" + pctVal.toFixed(1) + "%";
    tpInfo.textContent =
      "+" + pctVal.toFixed(1) + "% from current · Profit: +$" + gain;
    updateSummary();
  }

  function updateSummary() {
    var slPctVal = slIsOn ? slSlider.value / 10 : 0;
    var tpPctVal = tpIsOn ? tpSlider.value / 10 : 0;
    var loss = Math.round((QTY * CURRENT * slPctVal) / 100);
    var gain = Math.round((QTY * CURRENT * tpPctVal) / 100);
    var ratio = loss > 0 ? (gain / loss).toFixed(2) : gain > 0 ? "∞" : "0.00";
    var riskPct =
      loss + gain > 0
        ? Math.round((loss / (loss + gain)) * 100)
        : slIsOn
          ? 100
          : 0;

    sumRisk.textContent = "-$" + loss;
    sumReward.textContent = "+$" + gain;
    sumRatio.textContent = ratio + "x";
    rrBar.style.width = riskPct + "%";
  }

  slToggle.addEventListener("click", function () {
    slIsOn = !slIsOn;
    var dot = this.querySelector("div");
    if (slIsOn) {
      this.classList.replace("bg-[#E2E8F0]", "bg-[#00D4AA]");
      dot.style.right = "2px";
      dot.style.left = "auto";
    } else {
      this.classList.replace("bg-[#00D4AA]", "bg-[#E2E8F0]");
      dot.style.right = "auto";
      dot.style.left = "2px";
    }
    updateSummary();
  });

  tpToggle.addEventListener("click", function () {
    tpIsOn = !tpIsOn;
    var dot = this.querySelector("div");
    if (tpIsOn) {
      this.classList.replace("bg-[#E2E8F0]", "bg-[#00D4AA]");
      dot.style.right = "2px";
      dot.style.left = "auto";
    } else {
      this.classList.replace("bg-[#00D4AA]", "bg-[#E2E8F0]");
      dot.style.right = "auto";
      dot.style.left = "2px";
    }
    updateSummary();
  });

  slSlider.addEventListener("input", updateSL);
  tpSlider.addEventListener("input", updateTP);

  slPrice.addEventListener("change", function () {
    var price = parseInt(slPrice.value) || CURRENT;
    var pct = ((CURRENT - price) / CURRENT) * 100;
    pct = Math.max(0.5, Math.min(30, pct));
    slSlider.value = Math.round(pct * 10);
    updateSL();
  });

  tpPrice.addEventListener("change", function () {
    var price = parseInt(tpPrice.value) || CURRENT;
    var pct = ((price - CURRENT) / CURRENT) * 100;
    pct = Math.max(0.5, Math.min(100, pct));
    tpSlider.value = Math.round(pct * 10);
    updateTP();
  });

  updateSL();
  updateTP();
})();
