/*
 * 本文件只负责把 Rust 回放报告中的下注明细画成可交互的本金曲线。
 *
 * 处理分三步：
 *   逐笔明细 -> buildBankrollSeries() 按牌局合并
 *             -> sampleBankrollSeries() 为有限画布保留峰谷
 *             -> Canvas 绘制 + 指针/键盘悬停
 *
 * 报告中的完整下注明细始终保留；抽样仅用于绘图，避免几万点在 Canvas 上
 * 重复画出，同时不隐藏最大本金和最低本金等关键风险位置。
 */
const compactMoneyFormatter = new Intl.NumberFormat("zh-CN", {
  notation: "compact",
  maximumFractionDigits: 1,
});

const integerFormatter = new Intl.NumberFormat("zh-CN", {
  maximumFractionDigits: 0,
});

const moneyFormatter = new Intl.NumberFormat("zh-CN", {
  minimumFractionDigits: 2,
  maximumFractionDigits: 2,
});

function finiteNumber(value, fallback = 0) {
  const number = Number(value);
  return Number.isFinite(number) ? number : fallback;
}

// 金额格式化集中在图表模块，确保坐标轴、标记和悬停提示使用相同精度。
function money(value) {
  return `¥${moneyFormatter.format(value)}`;
}

function compactMoney(value) {
  return `¥${compactMoneyFormatter.format(value)}`;
}

function roundKey(bet) {
  return [bet.table_id, bet.session_id, bet.round_no, bet.started_at].join("|");
}

/**
 * 把逐笔下注明细整理成逐局资金路径。
 *
 * 多注模式会为同一局返回多条下注明细，但每条明细中的 bankroll_after 都是
 * 该局所有下注一起结算后的余额。这里按桌台、牌靴、局号和时间合并，确保
 * 同一局只在资金曲线上出现一次，同时保留该局下注合计和净输赢供悬停查看。
 */
export function buildBankrollSeries(report) {
  // 同一局可能有多条明细（允许同局多下注）。由于 Rust 已经把该局所有
  // 下注结算后的同一个 bankroll_after 写入每条明细，所以这里只需按局键合并。
  const initialBankroll = Number(report?.summary?.initial_bankroll);
  if (!Number.isFinite(initialBankroll)) return [];

  const settlementsByRound = new Map();
  const bets = Array.isArray(report?.bets) ? report.bets : [];

  for (const bet of bets) {
    const bankrollAfter = Number(bet.bankroll_after);
    if (!Number.isFinite(bankrollAfter)) continue;

    const key = roundKey(bet);
    let settlement = settlementsByRound.get(key);
    if (!settlement) {
      settlement = {
        tableId: bet.table_id,
        sessionId: bet.session_id,
        roundNo: bet.round_no,
        startedAt: bet.started_at,
        bankroll: bankrollAfter,
        roundStake: 0,
        roundProfit: 0,
        betCount: 0,
      };
      settlementsByRound.set(key, settlement);
    }

    settlement.bankroll = bankrollAfter;
    settlement.roundStake += finiteNumber(bet.amount);
    settlement.roundProfit += finiteNumber(bet.actual_profit);
    settlement.betCount += 1;
  }

  // 第 0 个点表示模拟开始，不是一次下注结算；它让图表能显示初始本金基线，
  // 也使第一局的回撤有明确的比较起点。
  let runningPeak = initialBankroll;
  let drawdownDuration = 0;
  const points = [{
    index: 0,
    bankroll: initialBankroll,
    cumulativeProfit: 0,
    drawdown: 0,
    drawdownRate: 0,
    drawdownDuration: 0,
    peakBankroll: initialBankroll,
    roundStake: 0,
    roundProfit: 0,
    betCount: 0,
    startedAt: "",
    tableId: null,
    sessionId: null,
    roundNo: null,
  }];

  let offset = 0;
  for (const settlement of settlementsByRound.values()) {
    runningPeak = Math.max(runningPeak, settlement.bankroll);
    const drawdown = Math.max(0, runningPeak - settlement.bankroll);
    drawdownDuration = drawdown > 0 ? drawdownDuration + 1 : 0;
    points.push({
      ...settlement,
      index: offset + 1,
      cumulativeProfit: settlement.bankroll - initialBankroll,
      drawdown,
      drawdownRate: runningPeak > 0 ? drawdown / runningPeak : 0,
      drawdownDuration,
      peakBankroll: runningPeak,
    });
    offset += 1;
  }

  return points;
}

/**
 * 画布宽度有限时按区间保留最高点和最低点。
 *
 * 普通等距抽样可能正好跳过最大盈利或最大回撤位置；峰谷抽样会让资金路径的
 * 风险形状保持可见。它只减少绘制点，完整数据仍保留给悬停和明细表。
 */
export function sampleBankrollSeries(points, maximumPoints) {
  const safeMaximum = Math.max(4, Math.floor(maximumPoints));
  if (points.length <= safeMaximum) return points;

  const first = points[0];
  const last = points.at(-1);
  const innerLength = points.length - 2;
  const bucketCount = Math.max(1, Math.floor((safeMaximum - 2) / 2));
  const sampled = [first];

  // 每个区间同时保留最低点和最高点，并按时间顺序写入，避免折线因为抽样
  // 顺序错乱而出现人为回折。第一点和最后一点始终保留。
  for (let bucket = 0; bucket < bucketCount; bucket += 1) {
    const start = 1 + Math.floor((bucket * innerLength) / bucketCount);
    const end = 1 + Math.floor(((bucket + 1) * innerLength) / bucketCount);
    if (start >= end) continue;

    let minimum = points[start];
    let maximum = points[start];
    for (let index = start + 1; index < end; index += 1) {
      if (points[index].bankroll < minimum.bankroll) minimum = points[index];
      if (points[index].bankroll > maximum.bankroll) maximum = points[index];
    }

    if (minimum.index === maximum.index) {
      sampled.push(minimum);
    } else if (minimum.index < maximum.index) {
      sampled.push(minimum, maximum);
    } else {
      sampled.push(maximum, minimum);
    }
  }

  if (sampled.at(-1)?.index !== last.index) sampled.push(last);
  return sampled;
}

function signedClass(element, value) {
  element.classList.remove("value-positive", "value-negative");
  if (value > 0) element.classList.add("value-positive");
  if (value < 0) element.classList.add("value-negative");
}

function chartColor(name, fallback) {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim() || fallback;
}

/** 创建一个不依赖第三方库的响应式资金折线图控制器。 */
export function createBankrollChart({
  canvas,
  plot,
  emptyState,
  pointCount,
  tooltip,
  tooltipTitle,
  tooltipMeta,
  tooltipBankroll,
  tooltipProfit,
  tooltipRound,
  guide,
  focus,
}) {
  // 某些页面/测试可能没有图表 DOM。返回空控制器让调用方仍可无条件调用
  // render/reset，这比在每个调用处都写一份 null 判断更简单。
  if (!canvas || !plot) return { render() {}, reset() {} };

  const context = canvas.getContext("2d");
  const panel = plot.closest(".bankroll-chart-panel");
  let points = [];
  let geometry = null;
  let keyboardIndex = 0;
  let resizeFrame = 0;

  function hideTooltip() {
    tooltip.hidden = true;
    guide.hidden = true;
    focus.hidden = true;
  }

  function showTooltip(point) {
    if (!geometry || !point) return;

    const x = geometry.xForIndex(point.index);
    const y = geometry.yForValue(point.bankroll);
    guide.style.left = `${x}px`;
    guide.style.top = `${geometry.top}px`;
    guide.style.height = `${geometry.plotHeight}px`;
    guide.hidden = false;

    focus.style.left = `${x}px`;
    focus.style.top = `${y}px`;
    focus.hidden = false;

    // 初始点没有桌台/牌靴/局号；其他点才显示一次下注结算局的定位信息。
    if (point.index === 0) {
      tooltipTitle.textContent = "模拟开始";
      tooltipMeta.textContent = "初始本金";
      tooltipRound.textContent = "尚未发生下注结算";
    } else {
      tooltipTitle.textContent = `第 ${integerFormatter.format(point.index)} 个下注结算局`;
      tooltipMeta.textContent = `桌 ${point.tableId} · 牌靴 ${point.sessionId} · 第 ${point.roundNo} 局 · ${point.startedAt}`;
      tooltipRound.textContent = `${point.betCount} 笔下注 · 合计 ${money(point.roundStake)} · 本局净输赢 ${money(point.roundProfit)} · 当前回撤 ${money(point.drawdown)}`;
    }

    tooltipBankroll.textContent = `本金 ${money(point.bankroll)}`;
    tooltipProfit.textContent = `累计输赢 ${money(point.cumulativeProfit)}`;
    signedClass(tooltipProfit, point.cumulativeProfit);
    tooltip.hidden = false;

    const tooltipWidth = tooltip.offsetWidth;
    const tooltipHeight = tooltip.offsetHeight;
    let left = x + 14;
    if (left + tooltipWidth > geometry.width - 8) left = x - tooltipWidth - 14;
    left = Math.max(8, Math.min(left, geometry.width - tooltipWidth - 8));
    const top = Math.max(8, Math.min(y - tooltipHeight / 2, geometry.height - tooltipHeight - 8));
    tooltip.style.left = `${left}px`;
    tooltip.style.top = `${top}px`;
  }

  function drawMarker(point, color, label, placeBelow) {
    const x = geometry.xForIndex(point.index);
    const y = geometry.yForValue(point.bankroll);
    context.beginPath();
    context.arc(x, y, 4.5, 0, Math.PI * 2);
    context.fillStyle = color;
    context.fill();
    context.lineWidth = 2;
    context.strokeStyle = chartColor("--surface-strong", "#ffffff");
    context.stroke();

    context.fillStyle = color;
    context.font = "700 11px Inter, system-ui, sans-serif";
    context.textAlign = x > geometry.width - 150 ? "right" : "left";
    context.fillText(
      `${label} ${compactMoney(point.bankroll)}`,
      x + (context.textAlign === "right" ? -8 : 8),
      y + (placeBelow ? 17 : -10),
    );
  }

  function draw() {
    if (points.length <= 1 || plot.hidden) return;

    const bounds = canvas.getBoundingClientRect();
    const width = Math.max(240, Math.round(bounds.width));
    const height = Math.max(260, Math.round(bounds.height));
    const pixelRatio = Math.min(window.devicePixelRatio || 1, 2);
    canvas.width = Math.round(width * pixelRatio);
    canvas.height = Math.round(height * pixelRatio);
    context.setTransform(pixelRatio, 0, 0, pixelRatio, 0, 0);
    context.clearRect(0, 0, width, height);

    // Canvas 的 CSS 尺寸与真实像素尺寸分开设置，乘以 devicePixelRatio 后
    // 再缩放绘图上下文，保证高 DPI 屏幕上的文字和线条不模糊。
    const compact = width < 620;
    const left = compact ? 62 : 82;
    const right = compact ? 14 : 24;
    const top = 28;
    const bottom = compact ? 44 : 46;
    const plotWidth = width - left - right;
    const plotHeight = height - top - bottom;
    // 先从完整 points 找到坐标范围，不能用绘图抽样后的点计算范围，否则
    // 被抽掉的峰值/谷值可能导致曲线超出图表或风险幅度显示失真。
    let rawMinimum = points[0].bankroll;
    let rawMaximum = points[0].bankroll;
    for (const point of points) {
      rawMinimum = Math.min(rawMinimum, point.bankroll);
      rawMaximum = Math.max(rawMaximum, point.bankroll);
    }
    const rawRange = rawMaximum - rawMinimum;
    const padding = rawRange > 0
      ? rawRange * 0.1
      : Math.max(Math.abs(rawMaximum) * 0.02, 1);
    const minimum = rawMinimum - padding;
    const maximum = rawMaximum + padding;
    const range = maximum - minimum;
    const finalIndex = points.at(-1).index;
    // 横轴按结算点序号等距分布，纵轴按本金线性映射；鼠标和键盘也复用
    // 同一个 geometry，因此标记与悬停位置始终一致。
    const xForIndex = (index) => left + (index / finalIndex) * plotWidth;
    const yForValue = (value) => top + ((maximum - value) / range) * plotHeight;
    geometry = { width, height, left, top, plotWidth, plotHeight, xForIndex, yForValue };

    const line = chartColor("--line", "#d9ddd7");
    const muted = chartColor("--muted", "#66736d");
    const positive = chartColor("--positive", "#087a55");
    const negative = chartColor("--negative", "#b33b32");
    const gold = chartColor("--gold", "#b98a32");
    const finalProfit = points.at(-1).cumulativeProfit;
    const pathColor = finalProfit >= 0 ? positive : negative;

    context.font = `${compact ? 10 : 11}px Inter, system-ui, sans-serif`;
    context.fillStyle = muted;
    context.strokeStyle = line;
    context.lineWidth = 1;
    context.setLineDash([]);
    for (let tick = 0; tick <= 4; tick += 1) {
      const ratio = tick / 4;
      const y = top + ratio * plotHeight;
      const value = maximum - ratio * range;
      context.beginPath();
      context.moveTo(left, y);
      context.lineTo(width - right, y);
      context.stroke();
      context.textAlign = "right";
      context.fillText(compactMoney(value), left - 9, y + 4);
    }

    const xTickCount = Math.min(4, finalIndex);
    for (let tick = 0; tick <= xTickCount; tick += 1) {
      const index = Math.round((finalIndex * tick) / xTickCount);
      const x = xForIndex(index);
      context.beginPath();
      context.moveTo(x, top);
      context.lineTo(x, top + plotHeight);
      context.stroke();
      context.textAlign = tick === 0 ? "left" : tick === xTickCount ? "right" : "center";
      context.fillText(integerFormatter.format(index), x, height - 17);
    }
    context.textAlign = "right";
    context.fillText("下注结算局序号", width - right, height - 2);

    const initialY = yForValue(points[0].bankroll);
    context.beginPath();
    context.setLineDash([6, 5]);
    context.strokeStyle = gold;
    context.moveTo(left, initialY);
    context.lineTo(width - right, initialY);
    context.stroke();
    context.setLineDash([]);
    context.fillStyle = gold;
    context.textAlign = "right";
    context.font = "700 10px Inter, system-ui, sans-serif";
    context.fillText(`初始 ${compactMoney(points[0].bankroll)}`, width - right - 3, initialY - 7);

    // 屏幕能显示的有效像素有限，绘图时只抽样到约每两个像素一个点；
    // 但最高/最低点由峰谷抽样保留，视觉上不会抹掉重要风险事件。
    const drawablePoints = sampleBankrollSeries(points, Math.max(120, Math.floor(plotWidth * 2)));
    const gradient = context.createLinearGradient(0, top, 0, top + plotHeight);
    gradient.addColorStop(0, `${pathColor}29`);
    gradient.addColorStop(1, `${pathColor}03`);
    context.beginPath();
    drawablePoints.forEach((point, index) => {
      const x = xForIndex(point.index);
      const y = yForValue(point.bankroll);
      if (index === 0) context.moveTo(x, y);
      else context.lineTo(x, y);
    });
    context.lineTo(xForIndex(drawablePoints.at(-1).index), top + plotHeight);
    context.lineTo(xForIndex(drawablePoints[0].index), top + plotHeight);
    context.closePath();
    context.fillStyle = gradient;
    context.fill();

    context.beginPath();
    drawablePoints.forEach((point, index) => {
      const x = xForIndex(point.index);
      const y = yForValue(point.bankroll);
      if (index === 0) context.moveTo(x, y);
      else context.lineTo(x, y);
    });
    context.strokeStyle = pathColor;
    context.lineWidth = compact ? 2 : 2.5;
    context.lineJoin = "round";
    context.lineCap = "round";
    context.stroke();

    const highest = points.reduce((best, point) => (
      point.bankroll > best.bankroll ? point : best
    ));
    const lowest = points.reduce((best, point) => (
      point.bankroll < best.bankroll ? point : best
    ));
    drawMarker(highest, positive, "最高", false);
    if (lowest.index !== highest.index) drawMarker(lowest, negative, "最低", true);
  }

  function scheduleDraw() {
    window.cancelAnimationFrame(resizeFrame);
    resizeFrame = window.requestAnimationFrame(() => {
      hideTooltip();
      draw();
    });
  }

  canvas.addEventListener("pointermove", (event) => {
    if (!geometry || points.length <= 1) return;
    const bounds = canvas.getBoundingClientRect();
    const x = event.clientX - bounds.left;
    if (x < geometry.left || x > geometry.left + geometry.plotWidth) {
      hideTooltip();
      return;
    }
    const ratio = (x - geometry.left) / geometry.plotWidth;
    // 指针位置映射回完整 points，而不是 drawablePoints，所以即使图形绘制
    // 经过抽样，悬停仍能查看完整报告中的每个结算点。
    keyboardIndex = Math.round(ratio * (points.length - 1));
    showTooltip(points[keyboardIndex]);
  });
  canvas.addEventListener("pointerleave", hideTooltip);
  canvas.addEventListener("focus", () => {
    keyboardIndex = points.length - 1;
    showTooltip(points[keyboardIndex]);
  });
  canvas.addEventListener("blur", hideTooltip);
  canvas.addEventListener("keydown", (event) => {
    if (points.length <= 1) return;
    const largeStep = Math.max(1, Math.floor(points.length / 100));
    const step = event.shiftKey ? largeStep : 1;
    if (event.key === "ArrowLeft") keyboardIndex = Math.max(0, keyboardIndex - step);
    else if (event.key === "ArrowRight") keyboardIndex = Math.min(points.length - 1, keyboardIndex + step);
    else if (event.key === "Home") keyboardIndex = 0;
    else if (event.key === "End") keyboardIndex = points.length - 1;
    else return;
    event.preventDefault();
    showTooltip(points[keyboardIndex]);
  });

  if (typeof ResizeObserver === "function") {
    new ResizeObserver(scheduleDraw).observe(plot);
  } else {
    window.addEventListener("resize", scheduleDraw);
  }

  return {
    reset(message = "上传 CSV 并完成策略回放后，这里会显示结算后本金的变化曲线。") {
      points = [];
      geometry = null;
      keyboardIndex = 0;
      pointCount.textContent = "暂无资金变化点";
      plot.hidden = true;
      emptyState.textContent = message;
      emptyState.hidden = false;
      if (panel) delete panel.dataset.trend;
      hideTooltip();
    },
    render(report) {
      // 每次新报告到来都重新建立完整曲线；图表不会在内部累加旧报告，
      // 这样重新上传 CSV 或重新配置策略时不会串入上一次的本金数据。
      points = buildBankrollSeries(report);
      const settlementCount = Math.max(0, points.length - 1);
      pointCount.textContent = settlementCount > 0
        ? `${integerFormatter.format(settlementCount)} 个下注结算局`
        : "暂无资金变化点";
      const hasSettlements = settlementCount > 0;
      if (panel) {
        panel.dataset.trend = points.at(-1)?.cumulativeProfit < 0 ? "negative" : "positive";
      }
      plot.hidden = !hasSettlements;
      emptyState.hidden = hasSettlements;
      if (!hasSettlements) {
        emptyState.textContent = Number(report?.summary?.replayed_rounds ?? 0) > 0
          ? "本次策略没有产生真实下注，因此本金没有形成变化曲线。"
          : "本次数据没有可回放牌靴，因此本金没有形成变化曲线。";
      }
      hideTooltip();
      if (!hasSettlements) return;

      canvas.setAttribute(
        "aria-label",
        `本金变化折线图，共 ${integerFormatter.format(settlementCount)} 个下注结算局；可使用左右方向键逐点查看。`,
      );
      keyboardIndex = points.length - 1;
      scheduleDraw();
    },
  };
}
