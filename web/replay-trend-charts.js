/* 回测时序图只展示 Worker 汇总的有界曲线；完整下注仍在本地明细中。 */
const NS = "http://www.w3.org/2000/svg";
const moneyFormat = new Intl.NumberFormat("zh-CN", { maximumFractionDigits: 2 });
const countFormat = new Intl.NumberFormat("zh-CN");
const compactFormat = new Intl.NumberFormat("zh-CN", { notation: "compact", maximumFractionDigits: 1 });

const money = (value) => `${value < 0 ? "−" : ""}¥${moneyFormat.format(Math.abs(value))}`;
const shortMoney = (value) => `${value < 0 ? "−" : ""}¥${compactFormat.format(Math.abs(value))}`;
const svgNode = (tag, attributes = {}) => {
  const node = document.createElementNS(NS, tag);
  for (const [key, value] of Object.entries(attributes)) node.setAttribute(key, String(value));
  return node;
};

function createLineController(card, kind) {
  const svg = card.querySelector("svg");
  const summary = card.querySelector(".trend-summary");
  const hover = card.querySelector(".trend-hover");
  const plot = { left: 76, right: 700, top: 22, bottom: 184 };
  let viewportWidth = 720;
  let points = [];
  let marker = null;
  let cursor = 0;
  let valueToY = () => plot.bottom;
  let indexToX = () => plot.left;
  const unit = kind === "stake" ? "笔" : "个下注局";

  function selectPoint(next) {
    if (!points.length || !marker) return;
    cursor = Math.max(0, Math.min(points.length - 1, next));
    const point = points[cursor];
    marker.setAttribute("cx", String(indexToX(point.index)));
    marker.setAttribute("cy", String(valueToY(point.value)));
    marker.hidden = false;
    hover.textContent = `第 ${countFormat.format(point.index)} ${unit} · ${money(point.value)}`;
  }

  svg.addEventListener("pointermove", (event) => {
    if (!points.length) return;
    const bounds = svg.getBoundingClientRect();
    if (!bounds.width) return;
    const x = (event.clientX - bounds.left) * viewportWidth / bounds.width;
    let low = 0;
    let high = points.length - 1;
    const wanted = Math.max(0, Math.min(1, (x - plot.left) / (plot.right - plot.left))) * points.at(-1).index;
    while (low < high) {
      const middle = Math.floor((low + high) / 2);
      if (points[middle].index < wanted) low = middle + 1;
      else high = middle;
    }
    if (low > 0 && Math.abs(points[low - 1].index - wanted) < Math.abs(points[low].index - wanted)) low -= 1;
    selectPoint(low);
  });
  svg.addEventListener("keydown", (event) => {
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    event.preventDefault();
    selectPoint(cursor + (event.key === "ArrowRight" ? 1 : -1));
  });

  return {
    reset() {
      points = [];
      svg.replaceChildren();
      summary.textContent = "暂无下注";
      hover.textContent = "完成回测后显示";
    },
    render(input, total) {
      points = Array.isArray(input)
        ? input.filter((point) => Number.isFinite(Number(point.index)) && Number.isFinite(Number(point.value)))
        : [];
      svg.replaceChildren();
      if (!points.length) return;
      viewportWidth = svg.clientWidth < 540 ? 360 : 720;
      plot.left = viewportWidth === 360 ? 66 : 76;
      plot.right = viewportWidth - 20;
      svg.setAttribute("viewBox", `0 0 ${viewportWidth} 230`);
      const values = points.map((point) => Number(point.value));
      const actualMaximum = Math.max(...values);
      const min = Math.min(0, ...values);
      const max = Math.max(0, ...values);
      const range = max - min || 1;
      const padding = range * 0.08;
      const bottomValue = min - padding;
      const topValue = max + padding;
      valueToY = (value) => plot.bottom - (value - bottomValue) / (topValue - bottomValue) * (plot.bottom - plot.top);
      indexToX = (index) => plot.left + (index - 1) / Math.max(1, total - 1) * (plot.right - plot.left);

      for (const value of new Set([min, 0, max])) {
        const y = valueToY(value);
        svg.append(svgNode("line", { x1: plot.left, x2: plot.right, y1: y, y2: y, class: "trend-grid-line" }));
        const label = svgNode("text", { x: plot.left - 7, y: y + 4, "text-anchor": "end", class: "trend-axis-label" });
        label.textContent = shortMoney(value);
        svg.append(label);
      }
      const path = svgNode("polyline", {
        points: points.map((point) => `${indexToX(point.index).toFixed(2)},${valueToY(point.value).toFixed(2)}`).join(" "),
        class: kind === "stake" ? "trend-line trend-line-stake" : "trend-line trend-line-profit",
      });
      svg.append(path);
      for (const [index, anchor] of [[1, "start"], [total, "end"]]) {
        const label = svgNode("text", {
          x: indexToX(index), y: 212, "text-anchor": anchor, class: "trend-axis-label",
        });
        label.textContent = `第 ${countFormat.format(index)} ${unit}`;
        svg.append(label);
      }
      marker = svgNode("circle", { r: 5, class: "trend-marker" });
      svg.append(marker);
      selectPoint(points.length - 1);
      summary.textContent = `${countFormat.format(total)} ${unit} · 峰值 ${money(actualMaximum)}`;
      svg.setAttribute("aria-label", `${kind === "stake" ? "每笔下注额" : "逐局盈亏"}变化；共 ${total} ${unit}，最低 ${money(min)}，最高 ${money(max)}`);
    },
  };
}

function createDistributionController(card) {
  const plot = card.querySelector(".trend-distribution-bars");
  const summary = card.querySelector(".trend-summary");
  return {
    reset() {
      plot.replaceChildren();
      summary.textContent = "暂无下注";
    },
    render(distribution, bettingRounds) {
      plot.replaceChildren();
      if (!Array.isArray(distribution) || !distribution.length) return;
      const maximumRound = Math.max(...distribution.map((item) => Number(item.roundNo) || 0));
      const width = Math.max(10, Math.ceil(maximumRound / 18 / 10) * 10);
      const bins = new Map();
      for (const item of distribution) {
        const roundNo = Number(item.roundNo);
        const count = Number(item.count);
        if (!Number.isSafeInteger(roundNo) || roundNo <= 0 || !Number.isFinite(count) || count <= 0) continue;
        const start = Math.floor((roundNo - 1) / width) * width + 1;
        bins.set(start, (bins.get(start) ?? 0) + count);
      }
      const maximumCount = Math.max(1, ...bins.values());
      for (let start = 1; start <= maximumRound; start += width) {
        const count = bins.get(start) ?? 0;
        const column = document.createElement("div");
        column.className = "trend-distribution-column";
        column.title = `第 ${start}–${Math.min(start + width - 1, maximumRound)} 局：${countFormat.format(count)} 个下注局`;
        const number = document.createElement("strong");
        number.textContent = countFormat.format(count);
        const bar = document.createElement("i");
        bar.style.height = `${Math.max(count > 0 ? 4 : 0, count / maximumCount * 100)}px`;
        const label = document.createElement("small");
        label.textContent = `${start}–${Math.min(start + width - 1, maximumRound)}`;
        column.append(number, bar, label);
        plot.append(column);
      }
      summary.textContent = `${countFormat.format(bettingRounds)} 个下注局`;
      plot.setAttribute("aria-label", `实际下注子局数分布，共 ${bettingRounds} 个下注局；每组 ${width} 局`);
    },
  };
}

export function createReplayTrendCharts(section) {
  if (!section) return { reset() {}, render() {} };
  const stake = createLineController(section.querySelector('[data-trend="stake"]'), "stake");
  const profit = createLineController(section.querySelector('[data-trend="profit"]'), "profit");
  const rounds = createDistributionController(section.querySelector('[data-trend="rounds"]'));
  return {
    reset() {
      section.hidden = true;
      stake.reset();
      profit.reset();
      rounds.reset();
    },
    render(report) {
      const trends = report?.trend_charts;
      const totalBets = Number(trends?.total_bets) || 0;
      const bettingRounds = Number(trends?.betting_rounds) || 0;
      section.hidden = totalBets === 0;
      if (totalBets === 0) return;
      stake.render(trends.stake_points, totalBets);
      profit.render(trends.round_profit_points, bettingRounds);
      rounds.render(trends.round_distribution, bettingRounds);
    },
  };
}
