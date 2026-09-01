/*
 * 下注贡献圆环只负责“分类汇总数据 -> SVG 圆环和图例”。
 *
 * Rust 已经在每笔真实结算时分别累计 gross_profit 与 gross_loss，因此本模块
 * 不会从分页表格重新计算输赢，也不会把同一玩法的盈利和亏损先做净额抵消。
 */
const SVG_NAMESPACE = "http://www.w3.org/2000/svg";
const DONUT_RADIUS = 78;
const DONUT_CIRCUMFERENCE = 2 * Math.PI * DONUT_RADIUS;
const MAX_VISIBLE_CATEGORIES = 5;

// 两张图使用相同、固定的分类色序。颜色只帮助定位，图例同时提供名称、金额和
// 百分比，避免仅依赖颜色传递数据。
const categoryColors = [
  "#087a55",
  "#b98a32",
  "#3e7189",
  "#b66c4a",
  "#7a638e",
  "#89918c",
];

const moneyFormatter = new Intl.NumberFormat("zh-CN", {
  minimumFractionDigits: 2,
  maximumFractionDigits: 2,
});

function money(value) {
  return `¥${moneyFormatter.format(value)}`;
}

function percent(value) {
  return `${(value * 100).toFixed(1)}%`;
}

function finitePositive(value) {
  const numericValue = Number(value);
  return Number.isFinite(numericValue) && numericValue > 0 ? numericValue : 0;
}

/**
 * 把 14 种玩法压缩为适合圆环阅读的“前五类 + 其他”。
 *
 * 圆环用于快速观察集中度，不适合塞入大量极细扇区。完整的逐玩法金额仍由页面
 * 下方“各下注类型投注与盈亏”卡片展示，所以这里合并长尾不会损失精确查询能力。
 */
export function buildContributionSeries(
  breakdown = {},
  field,
  labels = {},
  maximumVisible = MAX_VISIBLE_CATEGORIES,
) {
  const entries = Object.entries(breakdown)
    .map(([key, metrics]) => ({
      key,
      label: labels[key] ?? key,
      value: finitePositive(metrics?.[field]),
    }))
    .filter((item) => item.value > 0)
    .sort((left, right) => right.value - left.value || left.label.localeCompare(right.label, "zh-CN"));

  const total = entries.reduce((sum, item) => sum + item.value, 0);
  if (total <= 0) return { total: 0, items: [] };

  const safeMaximum = Math.max(1, Math.floor(maximumVisible));
  const visible = entries.slice(0, safeMaximum);
  const remainder = entries.slice(safeMaximum);
  if (remainder.length > 0) {
    visible.push({
      key: "other",
      label: `其他 ${remainder.length} 类`,
      value: remainder.reduce((sum, item) => sum + item.value, 0),
    });
  }

  return {
    total,
    items: visible.map((item, index) => ({
      ...item,
      color: categoryColors[index % categoryColors.length],
      share: item.value / total,
    })),
  };
}

function createSvgCircle(className) {
  const circle = document.createElementNS(SVG_NAMESPACE, "circle");
  circle.setAttribute("class", className);
  circle.setAttribute("cx", "110");
  circle.setAttribute("cy", "110");
  circle.setAttribute("r", String(DONUT_RADIUS));
  return circle;
}

function createDonutController(card) {
  const plot = card.querySelector(".contribution-donut-plot");
  const svg = card.querySelector("svg");
  const segmentGroup = card.querySelector(".contribution-segments");
  const totalElement = card.querySelector(".contribution-total");
  const legend = card.querySelector(".contribution-legend");
  const tooltip = card.querySelector(".contribution-tooltip");
  const tooltipLabel = tooltip.querySelector("strong");
  const tooltipValue = tooltip.querySelector("span");
  const centerLabel = card.dataset.kind === "profit" ? "累计毛盈利" : "累计毛亏损";

  function hideTooltip() {
    tooltip.hidden = true;
    card.querySelectorAll(".is-active").forEach((element) => element.classList.remove("is-active"));
  }

  function showTooltip(item, segment, legendItem, pointerEvent) {
    card.querySelectorAll(".is-active").forEach((element) => element.classList.remove("is-active"));
    segment.classList.add("is-active");
    legendItem.classList.add("is-active");
    tooltipLabel.textContent = item.label;
    tooltipValue.textContent = `${money(item.value)} · ${percent(item.share)}`;
    tooltip.hidden = false;

    const bounds = plot.getBoundingClientRect();
    const pointerX = pointerEvent ? pointerEvent.clientX - bounds.left : bounds.width / 2;
    const pointerY = pointerEvent ? pointerEvent.clientY - bounds.top : 18;
    const tooltipWidth = tooltip.offsetWidth;
    const left = Math.max(6, Math.min(pointerX - tooltipWidth / 2, bounds.width - tooltipWidth - 6));
    const top = Math.max(6, Math.min(pointerY - tooltip.offsetHeight - 10, bounds.height - 52));
    tooltip.style.left = `${left}px`;
    tooltip.style.top = `${top}px`;
  }

  function reset() {
    segmentGroup.replaceChildren();
    legend.replaceChildren();
    totalElement.textContent = "—";
    plot.classList.add("is-empty");
    hideTooltip();
  }

  function render(series) {
    segmentGroup.replaceChildren();
    legend.replaceChildren();
    totalElement.textContent = money(series.total);
    plot.classList.toggle("is-empty", series.items.length === 0);
    hideTooltip();

    if (series.items.length === 0) {
      const emptyItem = document.createElement("li");
      emptyItem.className = "contribution-legend-empty";
      emptyItem.textContent = card.dataset.kind === "profit"
        ? "本次回放没有产生正收益下注"
        : "本次回放没有产生负收益下注";
      legend.append(emptyItem);
      svg.setAttribute("aria-label", `${centerLabel}为 0`);
      return;
    }

    let offset = 0;
    for (const [index, item] of series.items.entries()) {
      const completeLength = item.share * DONUT_CIRCUMFERENCE;
      // 圆环之间留出固定视觉间隔，但数值占比仍使用未扣间隔前的真实 share。
      const gap = series.items.length > 1 ? Math.min(3.2, completeLength * 0.18) : 0;
      const visibleLength = Math.max(0.75, completeLength - gap);
      const segment = createSvgCircle("contribution-segment");
      segment.setAttribute("pathLength", String(DONUT_CIRCUMFERENCE));
      segment.setAttribute("stroke", item.color);
      segment.setAttribute(
        "stroke-dasharray",
        `${visibleLength} ${DONUT_CIRCUMFERENCE - visibleLength}`,
      );
      segment.setAttribute("stroke-dashoffset", String(-offset));
      segment.setAttribute("tabindex", "0");
      segment.setAttribute(
        "aria-label",
        `${item.label}，${money(item.value)}，占 ${percent(item.share)}`,
      );
      segment.style.setProperty("--segment-length", String(visibleLength));
      segment.style.setProperty("--segment-rest", String(DONUT_CIRCUMFERENCE - visibleLength));
      segment.style.setProperty("--segment-delay", `${index * 65}ms`);
      segmentGroup.append(segment);

      const legendItem = document.createElement("li");
      legendItem.tabIndex = 0;
      legendItem.innerHTML = `
        <i style="--legend-color: ${item.color}" aria-hidden="true"></i>
        <span>${item.label}</span>
        <strong>${money(item.value)}</strong>
        <small>${percent(item.share)}</small>
      `;
      legend.append(legendItem);

      const activateFromPointer = (event) => showTooltip(item, segment, legendItem, event);
      const activateFromKeyboard = () => showTooltip(item, segment, legendItem);
      segment.addEventListener("pointerenter", activateFromPointer);
      segment.addEventListener("pointermove", activateFromPointer);
      segment.addEventListener("pointerleave", hideTooltip);
      segment.addEventListener("focus", activateFromKeyboard);
      segment.addEventListener("blur", hideTooltip);
      legendItem.addEventListener("pointerenter", activateFromKeyboard);
      legendItem.addEventListener("pointerleave", hideTooltip);
      legendItem.addEventListener("focus", activateFromKeyboard);
      legendItem.addEventListener("blur", hideTooltip);

      offset += completeLength;
    }

    svg.setAttribute(
      "aria-label",
      `${centerLabel}${money(series.total)}，共 ${series.items.length} 个可见分类`,
    );
  }

  return { render, reset };
}

/** 创建盈利与亏损两个共享数据口径的圆环控制器。 */
export function createBetContributionCharts({ section, labels = {} }) {
  if (!section) return { render() {}, reset() {} };

  const profitCard = section.querySelector('[data-kind="profit"]');
  const lossCard = section.querySelector('[data-kind="loss"]');
  const profitDonut = createDonutController(profitCard);
  const lossDonut = createDonutController(lossCard);

  return {
    reset() {
      section.hidden = true;
      profitDonut.reset();
      lossDonut.reset();
    },
    render(report) {
      const breakdown = report?.summary?.bet_breakdown ?? {};
      const profitSeries = buildContributionSeries(breakdown, "gross_profit", labels);
      const lossSeries = buildContributionSeries(breakdown, "gross_loss", labels);
      profitDonut.render(profitSeries);
      lossDonut.render(lossSeries);
      section.hidden = profitSeries.total <= 0 && lossSeries.total <= 0;
    },
  };
}
