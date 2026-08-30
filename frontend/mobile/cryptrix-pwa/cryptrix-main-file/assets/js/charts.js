// ── Charts Module ────────────────────────────────────────────────
"use strict";

const Charts = {
  instances: {},

  defaults: {
    responsive: true,
    maintainAspectRatio: false,
    plugins: { legend: { display: false }, tooltip: { enabled: true } },
    animation: { duration: 600, easing: "easeInOutQuart" },
  },

  destroy(id) {
    if (this.instances[id]) {
      this.instances[id].destroy();
      delete this.instances[id];
    }
  },

  destroyAll() {
    Object.keys(this.instances).forEach((id) => this.destroy(id));
  },

  portfolioLine(canvasId, data, period = "7d") {
    this.destroy(canvasId);
    const canvas = document.getElementById(canvasId);
    if (!canvas) return;
    const ctx = canvas.getContext("2d");

    const gradient = ctx.createLinearGradient(
      0,
      0,
      0,
      canvas.offsetHeight || 200,
    );
    gradient.addColorStop(0, "rgba(0, 212, 170, 0.3)");
    gradient.addColorStop(1, "rgba(0, 212, 170, 0)");

    const labels = data.map((_, i) => {
      if (period === "7d")
        return ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"][i % 7] || i;
      if (period === "30d") return i % 5 === 0 ? `Day ${i + 1}` : "";
      return i % 30 === 0 ? `M${Math.floor(i / 30) + 1}` : "";
    });

    this.instances[canvasId] = new Chart(ctx, {
      type: "line",
      data: {
        labels,
        datasets: [
          {
            data,
            borderColor: "#00D4AA",
            borderWidth: 2.5,
            backgroundColor: gradient,
            fill: true,
            tension: 0.4,
            pointRadius: 0,
            pointHoverRadius: 6,
            pointHoverBackgroundColor: "#00D4AA",
            pointHoverBorderColor: "#fff",
            pointHoverBorderWidth: 2,
          },
        ],
      },
      options: {
        ...this.defaults,
        scales: {
          x: { display: false },
          y: { display: false },
        },
        interaction: { mode: "index", intersect: false },
        plugins: {
          ...this.defaults.plugins,
          tooltip: {
            enabled: true,
            backgroundColor: "rgba(18,20,26,0.95)",
            borderColor: "rgba(0,212,170,0.3)",
            borderWidth: 1,
            titleColor: "#8B91A8",
            bodyColor: "#F0F2F8",
            bodyFont: { size: 14, weight: "700", family: "Inter" },
            callbacks: {
              label: (ctx) =>
                " $" +
                ctx.raw.toLocaleString("en-US", {
                  minimumFractionDigits: 2,
                  maximumFractionDigits: 2,
                }),
            },
          },
        },
      },
    });
    return this.instances[canvasId];
  },

  coinLine(canvasId, data, color = "#00D4AA", positive = true) {
    this.destroy(canvasId);
    const canvas = document.getElementById(canvasId);
    if (!canvas) return;
    const ctx = canvas.getContext("2d");

    const lineColor = positive ? "#00C896" : "#FF4757";
    const gradient = ctx.createLinearGradient(
      0,
      0,
      0,
      canvas.offsetHeight || 200,
    );
    gradient.addColorStop(
      0,
      positive ? "rgba(0,200,150,0.25)" : "rgba(255,71,87,0.25)",
    );
    gradient.addColorStop(1, "rgba(0,0,0,0)");

    this.instances[canvasId] = new Chart(ctx, {
      type: "line",
      data: {
        labels: data.map((_, i) => i),
        datasets: [
          {
            data,
            borderColor: lineColor,
            borderWidth: 2,
            backgroundColor: gradient,
            fill: true,
            tension: 0.4,
            pointRadius: 0,
          },
        ],
      },
      options: {
        ...this.defaults,
        scales: { x: { display: false }, y: { display: false } },
        plugins: { ...this.defaults.plugins, tooltip: { enabled: false } },
      },
    });
    return this.instances[canvasId];
  },

  miniLine(canvasId, data, positive = true) {
    this.destroy(canvasId);
    const canvas = document.getElementById(canvasId);
    if (!canvas) return;
    canvas.width = 72;
    canvas.height = 32;
    const ctx = canvas.getContext("2d");
    const color = positive ? "#00C896" : "#FF4757";

    this.instances[canvasId] = new Chart(ctx, {
      type: "line",
      data: {
        labels: data.map((_, i) => i),
        datasets: [
          {
            data,
            borderColor: color,
            borderWidth: 1.5,
            fill: false,
            tension: 0.4,
            pointRadius: 0,
          },
        ],
      },
      options: {
        responsive: false,
        maintainAspectRatio: false,
        scales: { x: { display: false }, y: { display: false } },
        plugins: { legend: { display: false }, tooltip: { enabled: false } },
        animation: { duration: 400 },
      },
    });
    return this.instances[canvasId];
  },

  allocationDonut(canvasId, holdings) {
    this.destroy(canvasId);
    const canvas = document.getElementById(canvasId);
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    const colors = [
      "#00D4AA",
      "#7B61FF",
      "#9945FF",
      "#F3BA2F",
      "#E84142",
      "#2A5ADA",
      "#FF007A",
      "#28A0F0",
    ];

    this.instances[canvasId] = new Chart(ctx, {
      type: "doughnut",
      data: {
        labels: holdings.map((h) => h.coin),
        datasets: [
          {
            data: holdings.map((h) => h.weight),
            backgroundColor: colors.slice(0, holdings.length),
            borderColor: "transparent",
            borderWidth: 0,
            hoverOffset: 8,
          },
        ],
      },
      options: {
        ...this.defaults,
        cutout: "68%",
        plugins: {
          ...this.defaults.plugins,
          tooltip: {
            enabled: true,
            backgroundColor: "rgba(18,20,26,0.95)",
            borderColor: "rgba(255,255,255,0.1)",
            borderWidth: 1,
            titleColor: "#8B91A8",
            bodyColor: "#F0F2F8",
            callbacks: { label: (ctx) => ` ${ctx.label}: ${ctx.raw}%` },
          },
        },
      },
    });
    return this.instances[canvasId];
  },

  pnlBar(canvasId) {
    this.destroy(canvasId);
    const canvas = document.getElementById(canvasId);
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    const months = [
      "Jan",
      "Feb",
      "Mar",
      "Apr",
      "May",
      "Jun",
      "Jul",
      "Aug",
      "Sep",
      "Oct",
      "Nov",
      "Dec",
    ];
    const gains = [
      820, 1240, -340, 2100, 890, -120, 3400, 1560, 890, 2210, -450, 1830,
    ];

    this.instances[canvasId] = new Chart(ctx, {
      type: "bar",
      data: {
        labels: months,
        datasets: [
          {
            data: gains,
            backgroundColor: gains.map((v) =>
              v >= 0 ? "rgba(0,200,150,0.8)" : "rgba(255,71,87,0.8)",
            ),
            borderRadius: 6,
            borderSkipped: false,
          },
        ],
      },
      options: {
        ...this.defaults,
        scales: {
          x: {
            grid: { display: false },
            ticks: { color: "#8B91A8", font: { size: 11, family: "Inter" } },
          },
          y: {
            grid: { color: "rgba(255,255,255,0.04)" },
            ticks: {
              color: "#8B91A8",
              font: { size: 11, family: "Inter" },
              callback: (v) => "$" + (v / 1000).toFixed(1) + "k",
            },
          },
        },
        plugins: {
          ...this.defaults.plugins,
          tooltip: {
            enabled: true,
            backgroundColor: "rgba(18,20,26,0.95)",
            callbacks: {
              label: (ctx) =>
                (ctx.raw >= 0 ? "+" : "") + "$" + ctx.raw.toLocaleString(),
            },
          },
        },
      },
    });
    return this.instances[canvasId];
  },

  riskGauge(canvasId, score = 65) {
    this.destroy(canvasId);
    const canvas = document.getElementById(canvasId);
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    const remaining = 100 - score;
    const colors =
      score < 33
        ? ["#00C896", "rgba(255,255,255,0.08)"]
        : score < 66
          ? ["#FFB800", "rgba(255,255,255,0.08)"]
          : ["#FF4757", "rgba(255,255,255,0.08)"];

    this.instances[canvasId] = new Chart(ctx, {
      type: "doughnut",
      data: {
        datasets: [
          {
            data: [score, remaining],
            backgroundColor: colors,
            borderColor: "transparent",
            borderWidth: 0,
          },
        ],
      },
      options: {
        ...this.defaults,
        cutout: "72%",
        rotation: -90,
        circumference: 180,
        plugins: { ...this.defaults.plugins, tooltip: { enabled: false } },
      },
    });
    return this.instances[canvasId];
  },

  volumeBar(canvasId, data) {
    this.destroy(canvasId);
    const canvas = document.getElementById(canvasId);
    if (!canvas) return;
    const ctx = canvas.getContext("2d");

    this.instances[canvasId] = new Chart(ctx, {
      type: "bar",
      data: {
        labels: data.map((_, i) => i),
        datasets: [
          {
            data,
            backgroundColor: "rgba(0,212,170,0.3)",
            borderColor: "rgba(0,212,170,0.6)",
            borderWidth: 1,
            borderRadius: 3,
            borderSkipped: false,
          },
        ],
      },
      options: {
        ...this.defaults,
        scales: { x: { display: false }, y: { display: false } },
        plugins: { ...this.defaults.plugins, tooltip: { enabled: false } },
      },
    });
    return this.instances[canvasId];
  },
};

window.Charts = Charts;
