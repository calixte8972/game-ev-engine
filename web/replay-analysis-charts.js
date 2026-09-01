/*
 * CSV 回放的四张诊断图：分类 ROI、盈亏瀑布、回撤水下图和风险收益散点图。
 *
 * 所有金额和分类统计都来自 Rust 回放报告。本模块只负责把已经汇总好的字段
 * 转成图形坐标，不重新实现百家乐结算、返水或下注策略。
 */
import { buildBankrollSeries } from "./bankroll-chart.js";

const SVG_NAMESPACE = "http://www.w3.org/2000/svg";
const moneyFormatter = new Intl.NumberFormat("zh-CN", {
  minimumFractionDigits: 2,
  maximumFractionDigits: 2,
});
const compactMoneyFormatter = new Intl.NumberFormat("zh-CN", {
  notation: "compact",
  maximumFractionDigits: 1,
});
const integerFormatter = new Intl.NumberFormat("zh-CN", { maximumFractionDigits: 0 });
const mainBetKeys = new Set(["player", "banker", "tie"]);

function finiteNumber(value, fallback = 0) {
  const number = Number(value);
  return Number.isFinite(number) ? number : fallback;
}

function money(value) {
  return `¥${moneyFormatter.format(finiteNumber(value))}`;
}

function signedMoney(value) {
  const number = finiteNumber(value);
  if (number > 0) return `+${money(number)}`;
  if (number < 0) return `-${money(Math.abs(number))}`;
  return money(0);
}

function compactMoney(value) {
  const number = finiteNumber(value);
  const absolute = `¥${compactMoneyFormatter.format(Math.abs(number))}`;
  return number < 0 ? `-${absolute}` : absolute;
}

function percent(value, digits = 2) {
  return `${(finiteNumber(value) * 100).toFixed(digits)}%`;
}

function signedPercent(value, digits = 2) {
  const number = finiteNumber(value);
  return `${number > 0 ? "+" : ""}${percent(number, digits)}`;
}

function svgElement(tag, attributes = {}) {
  const element = document.createElementNS(SVG_NAMESPACE, tag);
  for (const [name, value] of Object.entries(attributes)) {
    element.setAttribute(name, String(value));
  }
  return element;
}

function chartColor(name, fallback) {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim() || fallback;
}

/** 每种实际下注玩法的含返水净 ROI，分母是该玩法累计下注额。 */
export function buildRoiSeries(breakdown = {}, labels = {}) {
  return Object.entries(breakdown)
    .map(([key, metrics]) => {
      const stake = Math.max(0, finiteNumber(metrics?.total_stake));
      const profit = finiteNumber(metrics?.total_profit);
      return {
        key,
        label: labels[key] ?? key,
        count: Math.max(0, Math.trunc(finiteNumber(metrics?.count))),
        stake,
        profit,
        roi: stake > 0 ? profit / stake : 0,
      };
    })
    .filter((item) => item.stake > 0)
    .sort((left, right) => right.roi - left.roi || right.stake - left.stake);
}

/**
 * 构造严格可加总的瀑布：基础毛盈利 + 返水 - 基础毛亏损 = 最终净盈亏。
 * gross_profit/gross_loss 已包含返水，因此这里必须使用 base_gross_* 字段。
 */
export function buildWaterfallSeries(summary = {}, breakdown = {}) {
  const categories = Object.values(breakdown);
  const grossProfit = categories.reduce(
    (sum, metrics) => sum + Math.max(0, finiteNumber(metrics?.base_gross_profit)),
    0,
  );
  const grossLoss = categories.reduce(
    (sum, metrics) => sum + Math.max(0, finiteNumber(metrics?.base_gross_loss)),
    0,
  );
  const rebate = Math.max(0, finiteNumber(summary?.rebate_income));
  const afterProfit = grossProfit;
  const afterRebate = afterProfit + rebate;
  const calculatedNet = afterRebate - grossLoss;
  const reportedNet = finiteNumber(summary?.total_profit, calculatedNet);

  return {
    grossProfit,
    rebate,
    grossLoss,
    net: reportedNet,
    reconciliationDifference: calculatedNet - reportedNet,
    items: [
      { key: "gross-profit", label: "基础毛盈利", type: "positive", start: 0, end: afterProfit, delta: grossProfit },
      { key: "rebate", label: "返水", type: "rebate", start: afterProfit, end: afterRebate, delta: rebate },
      { key: "gross-loss", label: "基础毛亏损", type: "negative", start: afterRebate, end: calculatedNet, delta: -grossLoss },
      { key: "net", label: "最终净盈亏", type: "total", start: 0, end: reportedNet, delta: reportedNet, total: true },
    ],
  };
}

/** 将本金路径补充为回撤图需要的最大值和持续时间摘要。 */
export function buildDrawdownSeries(report) {
  const points = buildBankrollSeries(report);
  let maximumPoint = points[0] ?? null;
  let maximumDuration = 0;
  for (const point of points) {
    if (!maximumPoint || point.drawdown > maximumPoint.drawdown) maximumPoint = point;
    maximumDuration = Math.max(maximumDuration, finiteNumber(point.drawdownDuration));
  }
  return {
    points,
    maximumPoint,
    maximumDrawdown: finiteNumber(maximumPoint?.drawdown),
    maximumDrawdownRate: finiteNumber(maximumPoint?.drawdownRate),
    maximumDuration,
  };
}

/** 分类亏损率、ROI 和下注规模，用于风险收益气泡图。 */
export function buildRiskReturnSeries(breakdown = {}, labels = {}) {
  return Object.entries(breakdown)
    .map(([key, metrics]) => {
      const count = Math.max(0, Math.trunc(finiteNumber(metrics?.count)));
      const lossCount = Math.max(0, Math.trunc(finiteNumber(metrics?.loss_count)));
      const stake = Math.max(0, finiteNumber(metrics?.total_stake));
      const profit = finiteNumber(metrics?.total_profit);
      return {
        key,
        label: labels[key] ?? key,
        group: mainBetKeys.has(key) ? "main" : "side",
        count,
        lossCount,
        lossRate: count > 0 ? Math.min(1, lossCount / count) : 0,
        stake,
        profit,
        roi: stake > 0 ? profit / stake : 0,
      };
    })
    .filter((item) => item.count > 0 && item.stake > 0)
    .sort((left, right) => right.stake - left.stake);
}

function createRoiController(card, labels) {
  const list = card?.querySelector(".roi-chart-list");
  const empty = card?.querySelector(".analysis-chart-empty");
  const summary = card?.querySelector(".analysis-chart-summary");
  if (!card || !list) return { render() {}, reset() {} };

  function reset() {
    list.replaceChildren();
    list.hidden = true;
    empty.hidden = false;
    summary.textContent = "暂无实际下注";
  }

  function render(report) {
    const series = buildRoiSeries(report?.summary?.bet_breakdown, labels);
    list.replaceChildren();
    const hasData = series.length > 0;
    list.hidden = !hasData;
    empty.hidden = hasData;
    if (!hasData) {
      summary.textContent = "暂无实际下注";
      return;
    }

    const minimum = Math.min(0, ...series.map((item) => item.roi));
    const maximum = Math.max(0, ...series.map((item) => item.roi));
    const rawRange = maximum - minimum;
    const range = rawRange > 1e-12 ? rawRange : 1;
    const zero = ((0 - minimum) / range) * 100;

    for (const item of series) {
      const row = document.createElement("li");
      row.className = "roi-chart-row";
      const end = ((item.roi - minimum) / range) * 100;
      const left = Math.min(zero, end);
      const width = Math.max(0.35, Math.abs(end - zero));
      row.innerHTML = `
        <span class="roi-chart-label">${item.label}</span>
        <span class="roi-chart-track" style="--roi-zero:${zero}%; --roi-left:${left}%; --roi-width:${width}%">
          <i class="roi-zero-line" aria-hidden="true"></i>
          <i class="roi-chart-bar ${item.roi >= 0 ? "is-positive" : "is-negative"}" aria-hidden="true"></i>
        </span>
        <strong class="${item.roi > 0 ? "value-positive" : item.roi < 0 ? "value-negative" : ""}">${signedPercent(item.roi)}</strong>
        <small>${money(item.stake)} · ${integerFormatter.format(item.count)} 笔 · 净盈亏 ${signedMoney(item.profit)}</small>
      `;
      row.setAttribute(
        "aria-label",
        `${item.label}，ROI ${signedPercent(item.roi)}，累计下注 ${money(item.stake)}，${item.count} 笔`,
      );
      list.append(row);
    }
    summary.textContent = `${series.length} 种实际下注玩法 · 含返水净 ROI`;
  }

  reset();
  return { render, reset };
}

function createWaterfallController(card) {
  const svg = card?.querySelector("svg");
  const empty = card?.querySelector(".analysis-chart-empty");
  const summary = card?.querySelector(".analysis-chart-summary");
  if (!card || !svg) return { render() {}, reset() {} };

  function reset() {
    svg.replaceChildren();
    svg.hidden = true;
    empty.hidden = false;
    summary.textContent = "等待回放对账";
    summary.classList.remove("value-negative");
  }

  function render(report) {
    const data = buildWaterfallSeries(report?.summary, report?.summary?.bet_breakdown);
    const hasData = finiteNumber(report?.summary?.total_stake) > 0;
    svg.replaceChildren();
    svg.hidden = !hasData;
    empty.hidden = hasData;
    if (!hasData) {
      summary.textContent = "暂无实际下注";
      return;
    }

    const width = 700;
    const height = 300;
    const margin = { left: 68, right: 18, top: 34, bottom: 62 };
    const plotWidth = width - margin.left - margin.right;
    const plotHeight = height - margin.top - margin.bottom;
    const values = data.items.flatMap((item) => [item.start, item.end]);
    let minimum = Math.min(0, ...values);
    let maximum = Math.max(0, ...values);
    const rawRange = maximum - minimum;
    const padding = rawRange > 0 ? rawRange * 0.12 : 1;
    minimum -= padding;
    maximum += padding;
    const range = maximum - minimum;
    const y = (value) => margin.top + ((maximum - value) / range) * plotHeight;
    const slot = plotWidth / data.items.length;
    const barWidth = Math.min(92, slot * 0.58);

    for (let tick = 0; tick <= 4; tick += 1) {
      const value = maximum - (tick / 4) * range;
      const tickY = y(value);
      const line = svgElement("line", { x1: margin.left, x2: width - margin.right, y1: tickY, y2: tickY, class: "waterfall-grid" });
      const label = svgElement("text", { x: margin.left - 9, y: tickY + 4, class: "waterfall-axis-label", "text-anchor": "end" });
      label.textContent = compactMoney(value);
      svg.append(line, label);
    }

    const zeroLine = svgElement("line", { x1: margin.left, x2: width - margin.right, y1: y(0), y2: y(0), class: "waterfall-zero" });
    svg.append(zeroLine);

    data.items.forEach((item, index) => {
      const centerX = margin.left + slot * (index + 0.5);
      if (index < data.items.length - 2) {
        const connector = svgElement("line", {
          x1: centerX + barWidth / 2,
          x2: centerX + slot - barWidth / 2,
          y1: y(item.end),
          y2: y(item.end),
          class: "waterfall-connector",
        });
        svg.append(connector);
      }

      const top = Math.min(y(item.start), y(item.end));
      const barHeight = Math.max(2, Math.abs(y(item.start) - y(item.end)));
      const rect = svgElement("rect", {
        x: centerX - barWidth / 2,
        y: top,
        width: barWidth,
        height: barHeight,
        rx: 6,
        class: `waterfall-bar is-${item.type}`,
        tabindex: 0,
        role: "img",
        "aria-label": `${item.label}${signedMoney(item.delta)}`,
      });
      const title = svgElement("title");
      title.textContent = `${item.label}：${signedMoney(item.delta)}`;
      rect.append(title);

      const valueLabel = svgElement("text", {
        x: centerX,
        y: item.delta >= 0 ? top - 8 : top + barHeight + 16,
        class: "waterfall-value-label",
        "text-anchor": "middle",
      });
      valueLabel.textContent = signedMoney(item.delta);
      const categoryLabel = svgElement("text", {
        x: centerX,
        y: height - 25,
        class: "waterfall-category-label",
        "text-anchor": "middle",
      });
      categoryLabel.textContent = item.label;
      svg.append(rect, valueLabel, categoryLabel);
    });

    svg.setAttribute(
      "aria-label",
      `盈亏瀑布图：基础毛盈利${money(data.grossProfit)}，返水${money(data.rebate)}，基础毛亏损${money(data.grossLoss)}，最终净盈亏${signedMoney(data.net)}`,
    );
    const reconciled = Math.abs(data.reconciliationDifference) < 1e-7;
    summary.textContent = reconciled ? `已对账 · 最终 ${signedMoney(data.net)}` : "对账差异，请检查数据";
    summary.classList.toggle("value-negative", !reconciled);
  }

  reset();
  return { render, reset };
}

function sampleByDrawdown(points, maximumPoints) {
  const safeMaximum = Math.max(4, Math.floor(maximumPoints));
  if (points.length <= safeMaximum) return points;
  const first = points[0];
  const last = points.at(-1);
  const innerLength = points.length - 2;
  const bucketCount = Math.max(1, Math.floor((safeMaximum - 2) / 2));
  const sampled = [first];
  for (let bucket = 0; bucket < bucketCount; bucket += 1) {
    const start = 1 + Math.floor((bucket * innerLength) / bucketCount);
    const end = 1 + Math.floor(((bucket + 1) * innerLength) / bucketCount);
    if (start >= end) continue;
    let minimum = points[start];
    let maximum = points[start];
    for (let index = start + 1; index < end; index += 1) {
      if (points[index].drawdown < minimum.drawdown) minimum = points[index];
      if (points[index].drawdown > maximum.drawdown) maximum = points[index];
    }
    if (minimum.index === maximum.index) sampled.push(minimum);
    else if (minimum.index < maximum.index) sampled.push(minimum, maximum);
    else sampled.push(maximum, minimum);
  }
  if (sampled.at(-1)?.index !== last.index) sampled.push(last);
  return sampled;
}

function createDrawdownController(card) {
  const plot = card?.querySelector(".drawdown-chart-plot");
  const canvas = card?.querySelector("canvas");
  const empty = card?.querySelector(".analysis-chart-empty");
  const summary = card?.querySelector(".analysis-chart-summary");
  const tooltip = card?.querySelector(".analysis-chart-tooltip");
  if (!card || !plot || !canvas) return { render() {}, reset() {} };

  const context = canvas.getContext("2d");
  let data = { points: [] };
  let geometry = null;
  let keyboardIndex = 0;
  let resizeFrame = 0;

  function hideTooltip() {
    tooltip.hidden = true;
  }

  function showTooltip(point) {
    if (!geometry || !point) return;
    const x = geometry.xForIndex(point.index);
    const y = geometry.yForValue(point.drawdown);
    tooltip.querySelector("strong").textContent = point.index === 0 ? "模拟开始" : `第 ${integerFormatter.format(point.index)} 个下注结算局`;
    tooltip.querySelector("span").textContent = `回撤 ${money(point.drawdown)} · ${percent(point.drawdownRate)} · 已持续 ${integerFormatter.format(point.drawdownDuration)} 局`;
    tooltip.hidden = false;
    const tooltipWidth = tooltip.offsetWidth;
    let left = x + 12;
    if (left + tooltipWidth > geometry.width - 8) left = x - tooltipWidth - 12;
    tooltip.style.left = `${Math.max(8, left)}px`;
    tooltip.style.top = `${Math.max(8, Math.min(y - tooltip.offsetHeight - 9, geometry.height - tooltip.offsetHeight - 8))}px`;
  }

  function draw() {
    if (data.points.length <= 1 || plot.hidden) return;
    const bounds = canvas.getBoundingClientRect();
    const width = Math.max(260, Math.round(bounds.width));
    const height = Math.max(250, Math.round(bounds.height));
    const pixelRatio = Math.min(window.devicePixelRatio || 1, 2);
    canvas.width = Math.round(width * pixelRatio);
    canvas.height = Math.round(height * pixelRatio);
    context.setTransform(pixelRatio, 0, 0, pixelRatio, 0, 0);
    context.clearRect(0, 0, width, height);

    const compact = width < 560;
    const margin = { left: compact ? 60 : 76, right: 16, top: 24, bottom: 42 };
    const plotWidth = width - margin.left - margin.right;
    const plotHeight = height - margin.top - margin.bottom;
    const maximum = Math.max(data.maximumDrawdown * 1.12, 1);
    const finalIndex = data.points.at(-1).index;
    const xForIndex = (index) => margin.left + (index / finalIndex) * plotWidth;
    const yForValue = (value) => margin.top + (value / maximum) * plotHeight;
    geometry = { width, height, ...margin, plotWidth, plotHeight, xForIndex, yForValue };

    const line = chartColor("--line", "#d9ddd7");
    const muted = chartColor("--muted", "#66736d");
    const negative = chartColor("--negative", "#b33b32");
    const gold = chartColor("--gold", "#b98a32");
    context.font = `${compact ? 10 : 11}px Inter, system-ui, sans-serif`;
    context.textAlign = "right";
    context.fillStyle = muted;
    context.strokeStyle = line;
    context.lineWidth = 1;
    for (let tick = 0; tick <= 4; tick += 1) {
      const value = (tick / 4) * maximum;
      const tickY = yForValue(value);
      context.beginPath();
      context.moveTo(margin.left, tickY);
      context.lineTo(width - margin.right, tickY);
      context.stroke();
      context.fillText(compactMoney(-value), margin.left - 8, tickY + 4);
    }

    const drawable = sampleByDrawdown(data.points, Math.max(120, Math.floor(plotWidth * 2)));
    context.beginPath();
    context.moveTo(xForIndex(drawable[0].index), yForValue(0));
    for (const point of drawable) context.lineTo(xForIndex(point.index), yForValue(point.drawdown));
    context.lineTo(xForIndex(drawable.at(-1).index), yForValue(0));
    context.closePath();
    context.save();
    context.globalAlpha = 0.16;
    context.fillStyle = negative;
    context.fill();
    context.restore();

    context.beginPath();
    drawable.forEach((point, index) => {
      const x = xForIndex(point.index);
      const y = yForValue(point.drawdown);
      if (index === 0) context.moveTo(x, y);
      else context.lineTo(x, y);
    });
    context.strokeStyle = negative;
    context.lineWidth = compact ? 2 : 2.4;
    context.lineJoin = "round";
    context.stroke();

    const maximumPoint = data.maximumPoint;
    const markerX = xForIndex(maximumPoint.index);
    const markerY = yForValue(maximumPoint.drawdown);
    context.beginPath();
    context.arc(markerX, markerY, 4.5, 0, Math.PI * 2);
    context.fillStyle = gold;
    context.fill();
    context.fillStyle = gold;
    context.font = "700 10px Inter, system-ui, sans-serif";
    context.textAlign = markerX > width - 150 ? "right" : "left";
    context.fillText(
      `最大 ${compactMoney(-maximumPoint.drawdown)}`,
      markerX + (context.textAlign === "right" ? -8 : 8),
      Math.min(height - margin.bottom - 5, markerY + 17),
    );

    const xTickCount = Math.min(4, finalIndex);
    context.fillStyle = muted;
    context.font = `${compact ? 10 : 11}px Inter, system-ui, sans-serif`;
    for (let tick = 0; tick <= xTickCount; tick += 1) {
      const index = Math.round((finalIndex * tick) / xTickCount);
      const x = xForIndex(index);
      context.textAlign = tick === 0 ? "left" : tick === xTickCount ? "right" : "center";
      context.fillText(integerFormatter.format(index), x, height - 15);
    }
    context.textAlign = "right";
    context.fillText("下注结算局序号", width - margin.right, height - 2);
  }

  function scheduleDraw() {
    window.cancelAnimationFrame(resizeFrame);
    resizeFrame = window.requestAnimationFrame(() => {
      hideTooltip();
      draw();
    });
  }

  canvas.addEventListener("pointermove", (event) => {
    if (!geometry || data.points.length <= 1) return;
    const bounds = canvas.getBoundingClientRect();
    const x = event.clientX - bounds.left;
    const ratio = Math.max(0, Math.min(1, (x - geometry.left) / geometry.plotWidth));
    keyboardIndex = Math.round(ratio * (data.points.length - 1));
    showTooltip(data.points[keyboardIndex]);
  });
  canvas.addEventListener("pointerleave", hideTooltip);
  canvas.addEventListener("focus", () => {
    keyboardIndex = data.points.length - 1;
    showTooltip(data.points[keyboardIndex]);
  });
  canvas.addEventListener("blur", hideTooltip);
  canvas.addEventListener("keydown", (event) => {
    if (data.points.length <= 1) return;
    if (event.key === "ArrowLeft") keyboardIndex = Math.max(0, keyboardIndex - 1);
    else if (event.key === "ArrowRight") keyboardIndex = Math.min(data.points.length - 1, keyboardIndex + 1);
    else if (event.key === "Home") keyboardIndex = 0;
    else if (event.key === "End") keyboardIndex = data.points.length - 1;
    else return;
    event.preventDefault();
    showTooltip(data.points[keyboardIndex]);
  });

  if (typeof ResizeObserver === "function") new ResizeObserver(scheduleDraw).observe(plot);
  else window.addEventListener("resize", scheduleDraw);

  function reset() {
    data = { points: [] };
    geometry = null;
    plot.hidden = true;
    empty.hidden = false;
    summary.textContent = "暂无资金变化点";
    hideTooltip();
  }

  function render(report) {
    data = buildDrawdownSeries(report);
    const hasData = data.points.length > 1;
    plot.hidden = !hasData;
    empty.hidden = hasData;
    if (!hasData) {
      summary.textContent = "暂无资金变化点";
      return;
    }
    summary.textContent = `最大 ${money(data.maximumDrawdown)} · ${percent(data.maximumDrawdownRate)} · 最长持续 ${integerFormatter.format(data.maximumDuration)} 局`;
    canvas.setAttribute("aria-label", `回撤水下图，最大回撤${money(data.maximumDrawdown)}，最长持续${data.maximumDuration}个下注结算局；可使用左右方向键逐点查看。`);
    keyboardIndex = data.points.length - 1;
    scheduleDraw();
  }

  reset();
  return { render, reset };
}

function createScatterController(card, labels) {
  const svg = card?.querySelector("svg");
  const empty = card?.querySelector(".analysis-chart-empty");
  const summary = card?.querySelector(".analysis-chart-summary");
  const note = card?.querySelector(".scatter-sample-note");
  const tooltip = card?.querySelector(".analysis-chart-tooltip");
  if (!card || !svg) return { render() {}, reset() {} };

  function reset() {
    svg.replaceChildren();
    svg.hidden = true;
    empty.hidden = false;
    note.hidden = true;
    tooltip.hidden = true;
    summary.textContent = "暂无实际下注";
  }

  function render(report) {
    const series = buildRiskReturnSeries(report?.summary?.bet_breakdown, labels);
    svg.replaceChildren();
    const hasData = series.length > 0;
    svg.hidden = !hasData;
    empty.hidden = hasData;
    tooltip.hidden = true;
    if (!hasData) {
      note.hidden = true;
      summary.textContent = "暂无实际下注";
      return;
    }

    const width = 700;
    const height = 330;
    const margin = { left: 70, right: 28, top: 28, bottom: 58 };
    const plotWidth = width - margin.left - margin.right;
    const plotHeight = height - margin.top - margin.bottom;
    let minimumRoi = Math.min(0, ...series.map((item) => item.roi));
    let maximumRoi = Math.max(0, ...series.map((item) => item.roi));
    const roiRange = maximumRoi - minimumRoi;
    const roiPadding = roiRange > 0 ? roiRange * 0.12 : 0.05;
    minimumRoi -= roiPadding;
    maximumRoi += roiPadding;
    const yRange = maximumRoi - minimumRoi;
    const x = (lossRate) => margin.left + lossRate * plotWidth;
    const y = (roi) => margin.top + ((maximumRoi - roi) / yRange) * plotHeight;
    const stakes = series.map((item) => item.stake);
    const minimumStake = Math.min(...stakes);
    const maximumStake = Math.max(...stakes);
    const radius = (stake) => {
      if (maximumStake === minimumStake) return 11;
      const ratio = (Math.sqrt(stake) - Math.sqrt(minimumStake))
        / (Math.sqrt(maximumStake) - Math.sqrt(minimumStake));
      return 6 + ratio * 13;
    };

    for (let tick = 0; tick <= 4; tick += 1) {
      const lossRate = tick / 4;
      const tickX = x(lossRate);
      const line = svgElement("line", { x1: tickX, x2: tickX, y1: margin.top, y2: height - margin.bottom, class: "scatter-grid" });
      const label = svgElement("text", { x: tickX, y: height - 33, class: "scatter-axis-label", "text-anchor": "middle" });
      label.textContent = percent(lossRate, 0);
      svg.append(line, label);
    }
    for (let tick = 0; tick <= 4; tick += 1) {
      const roi = maximumRoi - (tick / 4) * yRange;
      const tickY = y(roi);
      const line = svgElement("line", { x1: margin.left, x2: width - margin.right, y1: tickY, y2: tickY, class: "scatter-grid" });
      const label = svgElement("text", { x: margin.left - 9, y: tickY + 4, class: "scatter-axis-label", "text-anchor": "end" });
      label.textContent = signedPercent(roi, 1);
      svg.append(line, label);
    }
    svg.append(svgElement("line", { x1: margin.left, x2: width - margin.right, y1: y(0), y2: y(0), class: "scatter-zero" }));

    const xTitle = svgElement("text", { x: width - margin.right, y: height - 7, class: "scatter-axis-title", "text-anchor": "end" });
    xTitle.textContent = "亏损笔数占比 →";
    const yTitle = svgElement("text", { x: 17, y: height / 2, class: "scatter-axis-title", transform: `rotate(-90 17 ${height / 2})`, "text-anchor": "middle" });
    yTitle.textContent = "含返水净 ROI →";
    svg.append(xTitle, yTitle);

    const mainColor = chartColor("--positive", "#087a55");
    const sideColor = chartColor("--gold", "#b98a32");
    const surface = chartColor("--surface-strong", "#ffffff");
    series.forEach((item, index) => {
      const pointX = x(item.lossRate);
      const pointY = y(item.roi);
      const pointRadius = radius(item.stake);
      const group = svgElement("g", { class: `scatter-point is-${item.group}`, tabindex: 0, role: "img" });
      const circle = svgElement("circle", {
        cx: pointX,
        cy: pointY,
        r: pointRadius,
        fill: item.group === "main" ? mainColor : sideColor,
        stroke: surface,
        "stroke-width": 2,
      });
      const anchor = pointX > width - margin.right - 110 ? "end" : "start";
      const label = svgElement("text", {
        x: pointX + (anchor === "end" ? -pointRadius - 5 : pointRadius + 5),
        y: pointY + (index % 2 === 0 ? -3 : 10),
        class: "scatter-point-label",
        "text-anchor": anchor,
      });
      label.textContent = item.label;
      group.setAttribute("aria-label", `${item.label}，亏损率${percent(item.lossRate)}，ROI${signedPercent(item.roi)}，累计下注${money(item.stake)}`);

      const show = (event) => {
        tooltip.querySelector("strong").textContent = item.label;
        tooltip.querySelector("span").textContent = `亏损率 ${percent(item.lossRate)} · ROI ${signedPercent(item.roi)} · ${item.count} 笔 · ${money(item.stake)}`;
        tooltip.hidden = false;
        const bounds = svg.getBoundingClientRect();
        const left = event?.clientX ? event.clientX - bounds.left : (pointX / width) * bounds.width;
        const top = event?.clientY ? event.clientY - bounds.top : (pointY / height) * bounds.height;
        tooltip.style.left = `${Math.max(8, Math.min(left + 10, bounds.width - tooltip.offsetWidth - 8))}px`;
        tooltip.style.top = `${Math.max(8, Math.min(top - tooltip.offsetHeight - 8, bounds.height - tooltip.offsetHeight - 8))}px`;
        group.classList.add("is-active");
      };
      const hide = () => {
        tooltip.hidden = true;
        group.classList.remove("is-active");
      };
      group.addEventListener("pointerenter", show);
      group.addEventListener("pointermove", show);
      group.addEventListener("pointerleave", hide);
      group.addEventListener("focus", () => show());
      group.addEventListener("blur", hide);
      group.append(circle, label);
      svg.append(group);
    });

    summary.textContent = `${series.length} 种玩法 · 气泡面积表示累计下注额`;
    note.hidden = series.length >= 8;
    if (!note.hidden) note.textContent = `本次只有 ${series.length} 种玩法实际下注，散点用于逐项比较，不解读整体相关性。`;
    svg.setAttribute("aria-label", `风险收益散点图，共${series.length}种实际下注玩法；横轴为亏损笔数占比，纵轴为含返水净ROI，气泡面积为累计下注额。`);
  }

  reset();
  return { render, reset };
}

/** 创建四张诊断图共享的生命周期控制器。 */
export function createReplayAnalysisCharts({ section, labels = {} }) {
  if (!section) return { render() {}, reset() {} };
  const controllers = [
    createRoiController(section.querySelector('[data-chart="roi"]'), labels),
    createWaterfallController(section.querySelector('[data-chart="waterfall"]')),
    createDrawdownController(section.querySelector('[data-chart="drawdown"]')),
    createScatterController(section.querySelector('[data-chart="risk-return"]'), labels),
  ];

  return {
    reset() {
      section.hidden = true;
      controllers.forEach((controller) => controller.reset());
    },
    render(report) {
      controllers.forEach((controller) => controller.render(report));
      section.hidden = finiteNumber(report?.summary?.total_stake) <= 0;
    },
  };
}
