/*
 * 浏览器入口只负责“收集输入、调用 WASM、渲染结果”。
 *
 * 数据流：
 *   HTML 表单字符串
 *       -> strategyConfig()/readNumber() 统一单位与校验
 *       -> Rust/WASM 的 analyzeBaccaratStrategy 或 analyzeBlackjack
 *       -> JSON.parse()
 *       -> renderResults()/renderReplay()/renderBlackjack()
 *
 * 概率、EV、凯利比例、真实回放结算和风险指标都由 Rust 计算；这里不复制
 * 任何业务公式。这样页面的职责是交互和展示，换成 Python 或其他前端时仍然
 * 可以复用同一份核心结果。
 */
import init, { analyzeBaccaratStrategy, analyzeBlackjack } from "./pkg/game_ev_engine.js";
import { createBankrollChart } from "./bankroll-chart.js";
import { createBetContributionCharts } from "./bet-contribution-charts.js";
import { createReplayAnalysisCharts } from "./replay-analysis-charts.js";
import { createReplayTrendCharts } from "./replay-trend-charts.js";
import { deleteReplayStore, openReplayStore, readReplayDetailPage, readReplayDetails } from "./replay-storage.js";
import { createBetDetailExportEncoder, EXPORT_FORMATS } from "./replay-export.mjs";

const form = document.querySelector("#analysis-form");
const analyzeButton = document.querySelector("#analyze-button");
const sampleButton = document.querySelector("#sample-button");
const clearButton = document.querySelector("#clear-button");
const cardsInput = document.querySelector("#cards-input");
const deckCount = document.querySelector("#deck-count");
const modeHelp = document.querySelector("#mode-help");
const wasmStatus = document.querySelector("#wasm-status");
const errorMessage = document.querySelector("#error-message");
const resultBody = document.querySelector("#result-body");
const sideResultBody = document.querySelector("#side-result-body");
const recommendation = document.querySelector("#recommendation");
const multipleRecommendations = document.querySelector("#multiple-recommendations");
const multipleRecommendationBody = document.querySelector("#multiple-recommendation-body");
const csvFileInput = document.querySelector("#csv-file");
const selectedFile = document.querySelector("#selected-file");
const replayButton = document.querySelector("#replay-button");
const replayStatus = document.querySelector("#replay-status");
const replaySourceTabs = document.querySelectorAll("[data-replay-source]");
const csvReplaySource = document.querySelector("#csv-replay-source");
const simulationReplaySource = document.querySelector("#simulation-replay-source");
const simulationShoes = document.querySelector("#simulation-shoes");
const simulationRounds = document.querySelector("#simulation-rounds");
const simulationSeed = document.querySelector("#simulation-seed");
const simulationEstimate = document.querySelector("#simulation-estimate");
const MAX_SIMULATION_SHOES = 1_000_000;
const MAX_REPLAY_ROUNDS = 1_666_666;
const parallelReplay = document.querySelector("#parallel-replay");
const parallelWorkerCount = document.querySelector("#parallel-worker-count");
const parallelAutoTune = document.querySelector("#parallel-auto-tune");
const summaryOnlyReplay = document.querySelector("#summary-only-replay");
const replayProgressPanel = document.querySelector("#replay-progress-panel");
const replayProgressBar = document.querySelector("#replay-progress-bar");
const replayProgressValue = document.querySelector("#replay-progress-value");
const replayProgressLabel = document.querySelector("#replay-progress-label");
const replayProgressEta = document.querySelector("#replay-progress-eta");
const replayProgressTrack = document.querySelector("#replay-progress-track");
const replayRulesTitle = document.querySelector("#replay-rules-title");
const replayRulePrimary = document.querySelector("#replay-rule-primary");
const replayRuleSecondary = document.querySelector("#replay-rule-secondary");
const replayRuleOrder = document.querySelector("#replay-rule-order");
const replayError = document.querySelector("#replay-error");
const replayResults = document.querySelector("#replay-results");
const replayStreakPanel = document.querySelector("#replay-streak-panel");
const replayBody = document.querySelector("#replay-body");
const replayDetailWrap = document.querySelector(".replay-detail-wrap");
const replayPagination = document.querySelector("#replay-pagination");
const replayPageSize = document.querySelector("#replay-page-size");
const replayFirstPage = document.querySelector("#replay-first-page");
const replayPreviousPage = document.querySelector("#replay-previous-page");
const replayNextPage = document.querySelector("#replay-next-page");
const replayLastPage = document.querySelector("#replay-last-page");
const replayPageStatus = document.querySelector("#replay-page-status");
const replayExportFormat = document.querySelector("#replay-export-format");
const replayExportButton = document.querySelector("#replay-export-button");
const replayExportStatus = document.querySelector("#replay-export-status");
const betCountGrid = document.querySelector("#bet-count-grid");

// 图表是独立控制器：app.js 只把完整回放报告交给它，不关心 Canvas 坐标、
// 抽样或悬停提示的绘制细节。
const bankrollChartController = createBankrollChart({
  canvas: document.querySelector("#bankroll-chart"),
  plot: document.querySelector("#bankroll-chart-plot"),
  emptyState: document.querySelector("#bankroll-chart-empty"),
  pointCount: document.querySelector("#bankroll-chart-count"),
  tooltip: document.querySelector("#bankroll-chart-tooltip"),
  tooltipTitle: document.querySelector("#bankroll-tooltip-title"),
  tooltipMeta: document.querySelector("#bankroll-tooltip-meta"),
  tooltipBankroll: document.querySelector("#bankroll-tooltip-bankroll"),
  tooltipProfit: document.querySelector("#bankroll-tooltip-profit"),
  tooltipRound: document.querySelector("#bankroll-tooltip-round"),
  guide: document.querySelector("#bankroll-chart-guide"),
  focus: document.querySelector("#bankroll-chart-focus"),
});
const payoutRule = document.querySelector("#payout-rule");
const stakeStrategy = document.querySelector("#stake-strategy");
const strategyParameterField = document.querySelector("#strategy-parameter-field");
const strategyParameterLabel = document.querySelector("#strategy-parameter-label");
const strategyParameterWrapper = document.querySelector("#strategy-parameter-wrapper");
const strategyParameterInput = document.querySelector("#strategy-parameter");
const strategyParameterPrefix = document.querySelector("#strategy-parameter-prefix");
const strategyParameterSuffix = document.querySelector("#strategy-parameter-suffix");
const compareStrategies = document.querySelector("#compare-strategies");
const compareStrategyFields = document.querySelector("#compare-strategy-fields");
// B 使用 A 现有的三组设置结构，给所有 ID 加前缀后成为完全独立的控件。
// 新增边注字段时只维护一份 HTML，避免 A/B 的默认值或可选项悄悄分叉。
for (const source of [form.querySelector(".strategy-config-stack"), form.querySelector(".side-limit-config")]) {
  const clone = source.cloneNode(true);
  for (const element of [clone, ...clone.querySelectorAll("[id]")]) {
    if (element.id) element.id = `compare-${element.id}`;
  }
  for (const label of clone.querySelectorAll("[for]")) {
    label.htmlFor = `compare-${label.htmlFor}`;
  }
  compareStrategyFields.append(clone);
}
const compareStakeStrategy = document.querySelector("#compare-stake-strategy");
const compareStrategyParameterField = document.querySelector("#compare-strategy-parameter-field");
const compareStrategyParameterLabel = document.querySelector("#compare-strategy-parameter-label");
const compareStrategyParameterWrapper = document.querySelector("#compare-strategy-parameter-wrapper");
const compareStrategyParameterInput = document.querySelector("#compare-strategy-parameter");
const compareStrategyParameterPrefix = document.querySelector("#compare-strategy-parameter-prefix");
const compareStrategyParameterSuffix = document.querySelector("#compare-strategy-parameter-suffix");
const compareCopyPrimary = document.querySelector("#compare-copy-primary");
const allowMultipleBets = document.querySelector("#allow-multiple-bets");
const gameTabs = document.querySelectorAll(".game-tab");
const baccaratViewTabs = document.querySelectorAll("[data-baccarat-view-tab]");
const blackjackForm = document.querySelector("#blackjack-form");
const blackjackAnalyzeButton = document.querySelector("#blackjack-analyze-button");
const blackjackSampleButton = document.querySelector("#blackjack-sample-button");
const blackjackShoeCards = document.querySelector("#blackjack-shoe-cards");
const blackjackPlayerCards = document.querySelector("#blackjack-player-cards");
const blackjackDealerUpcard = document.querySelector("#blackjack-dealer-upcard");
const blackjackModeHelp = document.querySelector("#blackjack-mode-help");
const blackjackError = document.querySelector("#blackjack-error");
const blackjackActionBody = document.querySelector("#blackjack-action-body");

const betLabels = {
  player: "闲",
  banker: "庄",
  tie: "和",
};

const sideBetLabels = {
  any_pair: "任意对子",
  banker_pair: "庄对",
  player_pair: "闲对",
  perfect_pair: "完美对子",
  big: "大",
  small: "小",
  lucky_seven: "幸运 7",
  super_lucky_seven: "超级幸运 7",
  lucky_six: "幸运 6",
  banker_dragon_bonus: "庄龙宝",
  player_dragon_bonus: "闲龙宝",
};

const allBetLabels = { ...betLabels, ...sideBetLabels };
const betCountOrder = [...Object.keys(betLabels), ...Object.keys(sideBetLabels)];
const contributionChartController = createBetContributionCharts({
  section: document.querySelector("#bet-contribution-charts"),
  labels: allBetLabels,
});
const replayAnalysisChartController = createReplayAnalysisCharts({
  section: document.querySelector("#replay-analysis-charts"),
  labels: allBetLabels,
});
const replayTrendChartController = createReplayTrendCharts(
  document.querySelector("#replay-trend-charts"),
);
const sideBetRoundLimitInputs = {
  any_pair: "#limit-any-pair",
  banker_pair: "#limit-banker-pair",
  player_pair: "#limit-player-pair",
  perfect_pair: "#limit-perfect-pair",
  big: "#limit-big",
  small: "#limit-small",
  lucky_seven: "#limit-lucky-seven",
  super_lucky_seven: "#limit-super-lucky-seven",
  lucky_six: "#limit-lucky-six",
  banker_dragon_bonus: "#limit-banker-dragon-bonus",
  player_dragon_bonus: "#limit-player-dragon-bonus",
};

const sideBetRoundLimitStatus = Object.fromEntries(
  Object.keys(sideBetRoundLimitInputs).map((key) => [key, `#status-${sideBetRoundLimitInputs[key].slice(1)}`]),
);

const suitSymbols = {
  C: "♣",
  D: "♦",
  H: "♥",
  S: "♠",
};
const redSuits = new Set(["D", "H"]);

const resultLabels = {
  win: "赢",
  loss: "输",
  push: "和局退回",
};

const payoutRuleLabels = {
  standard: "标准庄佣金",
  no_commission: "庄免佣（庄 6 半赔）",
};

const stakeStrategyLabels = {
  full_kelly: "完整凯利",
  half_kelly: "半凯利",
  quarter_kelly: "四分之一凯利",
  custom_kelly: "自定义分数凯利",
  fixed: "固定金额",
  bankroll_fraction: "固定本金比例",
  target_expected_profit: "固定期望盈利",
  target_volatility: "目标波动率",
  martingale: "倍投（输后翻倍）",
  reverse_martingale: "反倍投（赢后翻倍）",
  dalembert: "达朗贝尔（输加赢减）",
};

const stakeStrategyParameters = {
  custom_kelly: { label: "完整凯利缩放系数", unit: "percent", defaultValue: 30, step: 1 },
  fixed: { label: "固定下注金额", unit: "money", defaultValue: 100, step: 10 },
  bankroll_fraction: { label: "每局本金比例", unit: "percent", defaultValue: 1, step: 0.1 },
  target_expected_profit: { label: "单笔目标期望盈利", unit: "money", defaultValue: 10, step: 1 },
  target_volatility: { label: "单笔目标波动率", unit: "percent", defaultValue: 1, step: 0.1 },
  martingale: { label: "倍投基础金额（输后翻倍）", unit: "money", defaultValue: 100, step: 10 },
  reverse_martingale: { label: "反倍投基础金额（赢后翻倍）", unit: "money", defaultValue: 100, step: 10 },
  dalembert: { label: "达朗贝尔基础单位", unit: "money", defaultValue: 100, step: 10 },
};

const stakeStrategyHelp = {
  full_kelly: "按完整凯利比例下注；只有有效 EV 为正时才会投入。",
  half_kelly: "使用完整凯利的一半，降低波动和模型误差影响。",
  quarter_kelly: "使用完整凯利的四分之一，更保守地控制回撤。",
  custom_kelly: "用输入百分比缩放完整凯利比例。",
  fixed: "每次通过 EV 门槛后使用固定金额，不随凯利比例变化。",
  bankroll_fraction: "每次使用当前本金的固定百分比。",
  target_expected_profit: "根据当前有效 EV 反推达到目标期望盈利所需的金额。",
  target_volatility: "根据收益分布的标准差反推目标波动率对应的金额。",
  martingale: "回放中每个下注目标独立记录级数：输后下一笔翻倍，赢或和局复位。",
  reverse_martingale: "回放中赢后下一笔翻倍，输或和局复位；第一笔使用基础金额。",
  dalembert: "回放中输一笔增加一个基础单位，赢一笔减少一个单位，和局保持。",
};

const skipReasonLabels = {
  below_minimum_ev: "最优方向仍低于最低有效 EV，本局跳过。",
  non_positive_kelly: "有效 EV 没有形成正凯利比例，本局跳过。",
  risk_limit_is_zero: "资金比例或金额上限为 0，本局跳过。",
};

const blackjackActionLabels = {
  blackjack: "天然 21 点",
  stand: "停牌",
  hit: "补牌",
  double: "加倍",
  split: "分牌",
  surrender: "投降",
};

const moneyFormatter = new Intl.NumberFormat("zh-CN", {
  minimumFractionDigits: 2,
  maximumFractionDigits: 2,
});
const integerFormatter = new Intl.NumberFormat("zh-CN", {
  maximumFractionDigits: 0,
});

let wasmReady = false;
let replayWorkerReady = false;
let replayRunning = false;
let currentCsvFile = null;
let replaySourceMode = "csv";
let currentReplayReport = null;
let currentReplayPage = 1;
let activeReplayRunId = null;
let replayDetailStore = null;
let replayDetailSequence = 0;
let replayDetailWriteTail = Promise.resolve();
let replayDetailPendingBatches = [];
let replayDetailFlushTimer = null;
let replayStorageElapsedMs = 0;
let replayDetailRenderToken = 0;
let replayProgressOverall = 0;
let replayProgressStartedAt = 0;
let replayProgressLastSampleAt = 0;
let replayProgressLastSampleOverall = 0;
let replayProgressRate = 0;
let activeGame = "baccarat";
let activeBaccaratView = "analysis";

// URL 上的版本标记强制浏览器为当前页面创建同版本 Worker，避免发布后仍复用
// 旧 Worker，进而把新增配置字段当成 undefined 传给 WASM。
let replayWorker;
function resetReplayWorker() {
  // WASM 线性内存增长后不会主动缩回。每轮结束销毁旧 Worker，让浏览器
  // 回收整块计算内存，防止连续回测累计保留大内存。
  replayWorker?.terminate();
  replayWorkerReady = false;
  replayWorker = new Worker(new URL("./replay-worker.js?v=35", import.meta.url), { type: "module" });
  replayWorker.addEventListener("message", handleReplayMessage);
  replayWorker.addEventListener("error", handleReplayError);
  replayWorker.addEventListener("messageerror", handleReplayError);
  updateReplayButton();
}

function selectedMode() {
  return form.elements["source-mode"].value;
}

function selectedBlackjackMode() {
  return blackjackForm.elements["blackjack-source-mode"].value;
}

function setText(selector, value) {
  document.querySelector(selector).textContent = value;
}

function percent(value, digits = 4) {
  return value == null ? "—" : `${(value * 100).toFixed(digits)}%`;
}

function money(value) {
  return value == null ? "—" : `¥${moneyFormatter.format(value)}`;
}

function signedMoney(value) {
  if (value == null) return "—";
  const numericValue = Number(value);
  if (!Number.isFinite(numericValue)) return "—";
  const absoluteValue = money(Math.abs(numericValue));
  if (numericValue > 0) return `+${absoluteValue}`;
  if (numericValue < 0) return `-${absoluteValue}`;
  return absoluteValue;
}

function evClass(value) {
  if (value > 0) return "value-positive";
  if (value < 0) return "value-negative";
  return "";
}

function applySignedClass(element, value) {
  element.classList.remove("value-positive", "value-negative");
  const className = evClass(value);
  if (className) element.classList.add(className);
}

function readNumber(selector, label, options = {}) {
  const value = Number.parseFloat(document.querySelector(selector).value);
  if (!Number.isFinite(value)) {
    throw new Error(`请输入有效的${label}`);
  }
  if (options.positive && value <= 0) {
    throw new Error(`${label}必须大于 0`);
  }
  if (options.min != null && value < options.min) {
    throw new Error(`${label}不能小于 ${options.min}`);
  }
  if (options.max != null && value > options.max) {
    throw new Error(`${label}不能大于 ${options.max}`);
  }
  if (options.integer && !Number.isInteger(value)) {
    throw new Error(`${label}必须是整数`);
  }
  return value;
}

/**
 * 把 A 或 B 表单中的“人类输入”转换成 Rust API 使用的配置对象。
 *
 * HTML 数字输入读出来都是字符串；这里统一转成 Number，并在边界层检查
 * 有限值、正数、范围和整数。页面按百分比显示返水/EV/本金比例，但 Rust
 * 使用 0.009、0.01 这样的比例小数，因此转换也集中在这里，避免 A/B
 * 不小心使用不同单位；prefix 只改变所读控件，业务字段结构完全相同。
 */
function strategyConfig(prefix = "") {
  const field = (id) => `#${prefix}${id}`;
  const labelPrefix = prefix ? "策略 B " : "";
  const selectedStakeStrategy = document.querySelector(field("stake-strategy")).value;
  const parameterDefinition = stakeStrategyParameters[selectedStakeStrategy];
  let strategyParameter = 0;
  if (parameterDefinition) {
    strategyParameter = readNumber(field("strategy-parameter"), `${labelPrefix}${parameterDefinition.label}`, {
      min: 0,
      max: parameterDefinition.unit === "percent" ? 100 : undefined,
    });
    if (parameterDefinition.unit === "percent") strategyParameter /= 100;
  }
  const sideBetRoundLimits = Object.fromEntries(
    Object.entries(sideBetRoundLimitInputs).map(([key, selector]) => [
      key,
      readNumber(field(selector.slice(1)), `${labelPrefix}${sideBetLabels[key]}最后可下注局数`, {
        min: 0,
        integer: true,
      }),
    ]),
  );

  return {
    decks: Number.parseInt(deckCount.value, 10),
    rebateRate: readNumber(field("rebate-rate"), `${labelPrefix}返水比例`, { min: 0, max: 100 }) / 100,
    minimumEffectiveEv: readNumber(field("minimum-ev"), `${labelPrefix}最低有效 EV`) / 100,
    minimumSideBetEv: readNumber(field("minimum-side-bet-ev"), `${labelPrefix}边注最低 EV`) / 100,
    bankroll: readNumber(field("bankroll"), `${labelPrefix}本金`, { positive: true }),
    maxFraction: readNumber(field("max-fraction"), `${labelPrefix}单局本金比例上限`, {
      min: 0,
      max: 100,
    }) / 100,
    maxRoundStake: readNumber(field("max-round-stake"), `${labelPrefix}单局金额上限`, { min: 0 }),
    sideBetLimit: readNumber(field("side-bet-limit"), `${labelPrefix}边注单笔金额上限`, { min: 0 }),
    sideBetRoundLimits,
    allowMultipleBets: document.querySelector(field("allow-multiple-bets")).checked,
    tableLimit: readNumber(field("table-limit"), `${labelPrefix}桌台金额上限`, { min: 0 }),
    payoutRule: document.querySelector(field("payout-rule")).value,
    stakeStrategy: selectedStakeStrategy,
    strategyParameter,
  };
}

/** B 拥有独立下注配置，只与 A 共用牌靴副数和输入牌局。 */
function comparisonStrategyConfig() {
  return compareStrategies.checked ? strategyConfig("compare-") : null;
}

function updateComparisonStrategyFields(resetParameter = false) {
  compareStrategyFields.hidden = !compareStrategies.checked;
  const definition = stakeStrategyParameters[compareStakeStrategy.value];
  compareStrategyParameterField.hidden = !definition;
  if (!definition) return;
  compareStrategyParameterLabel.textContent = `策略 B · ${definition.label}`;
  if (resetParameter) compareStrategyParameterInput.value = String(definition.defaultValue);
  compareStrategyParameterInput.step = String(definition.step);
  compareStrategyParameterInput.max = definition.unit === "percent" ? "100" : "";
  const isMoney = definition.unit === "money";
  compareStrategyParameterWrapper.className = isMoney ? "input-prefix" : "input-suffix";
  compareStrategyParameterPrefix.hidden = !isMoney;
  compareStrategyParameterSuffix.hidden = isMoney;
}

function copyPrimaryConfigToComparison() {
  for (const target of compareStrategyFields.querySelectorAll("input[id], select[id]")) {
    const source = document.getElementById(target.id.slice("compare-".length));
    if (!source) continue;
    if (target.type === "checkbox") target.checked = source.checked;
    else target.value = source.value;
  }
  updateComparisonStrategyFields(false);
  updateComparisonConfigSummaries();
  updateSideBetRoundLimitHints();
}

/**
 * 根据当前金额策略切换参数输入框的文案、单位和默认值。
 *
 * 完整/半凯利不需要额外参数；固定金额需要货币前缀；本金比例、分数凯利
 * 和目标波动率使用百分号后缀。这里只改变表单外观，真正的参数解释仍由
 * Rust 的 StakeSizingStrategy 决定。
 */
function updateStakeStrategyFields() {
  const definition = stakeStrategyParameters[stakeStrategy.value];
  const help = document.querySelector("#stake-strategy-help");
  if (help) help.textContent = stakeStrategyHelp[stakeStrategy.value] ?? "";
  strategyParameterField.hidden = !definition;
  if (!definition) return;

  strategyParameterLabel.textContent = definition.label;
  strategyParameterInput.value = String(definition.defaultValue);
  strategyParameterInput.step = String(definition.step);
  strategyParameterInput.max = definition.unit === "percent" ? "100" : "";
  const isMoney = definition.unit === "money";
  strategyParameterWrapper.className = isMoney ? "input-prefix" : "input-suffix";
  strategyParameterPrefix.hidden = !isMoney;
  strategyParameterSuffix.hidden = isMoney;
}

/**
 * 折叠面板关闭时仍显示关键配置，用户不用反复展开确认当前策略。
 * 这里只读取表单用于展示，不做业务校验；正式计算仍统一经过 strategyConfig()。
 */
function updateConfigSummaries() {
  const bankrollValue = Number.parseFloat(document.querySelector("#bankroll").value);
  const rebateValue = Number.parseFloat(document.querySelector("#rebate-rate").value);
  const maxFractionValue = Number.parseFloat(document.querySelector("#max-fraction").value);
  const maxRoundStakeValue = Number.parseFloat(document.querySelector("#max-round-stake").value);
  const sideBetLimitValue = Number.parseFloat(document.querySelector("#side-bet-limit").value);
  const payoutLabel = payoutRule.value === "standard" ? "标准庄" : "庄免佣";
  const multiLabel = allowMultipleBets.checked ? "多下注开" : "单一目标";

  setText(
    "#funding-config-summary",
    `${payoutLabel} · ${stakeStrategyLabels[stakeStrategy.value]} · 本金 ${Number.isFinite(bankrollValue) ? money(bankrollValue) : "—"} · 返水 ${Number.isFinite(rebateValue) ? rebateValue : "—"}%`,
  );
  setText(
    "#risk-config-summary",
    `单局 ${Number.isFinite(maxFractionValue) ? maxFractionValue : "—"}% · 上限 ${Number.isFinite(maxRoundStakeValue) ? money(maxRoundStakeValue) : "—"} · 边注 ${Number.isFinite(sideBetLimitValue) ? money(sideBetLimitValue) : "—"} · ${multiLabel}`,
  );

  const blackjackSummary = document.querySelector("#blackjack-config-summary");
  if (blackjackSummary) {
    const decks = document.querySelector("#blackjack-decks").selectedOptions[0]?.textContent ?? "—";
    const payout = document.querySelector("#blackjack-payout").selectedOptions[0]?.textContent ?? "—";
    const soft17 = document.querySelector("#dealer-soft-17").value === "stand" ? "S17" : "H17";
    const surrender = document.querySelector("#late-surrender").value === "yes" ? "允许晚投降" : "不允许投降";
    const baseStake = Number.parseFloat(document.querySelector("#blackjack-base-stake").value);
    blackjackSummary.textContent = `${decks} · ${payout} · ${soft17} · ${surrender} · 底注 ${Number.isFinite(baseStake) ? money(baseStake) : "—"}`;
  }
}

/** B 的折叠摘要只读 B 控件，修改 A 时不会把 B 的显示或实际配置一起改掉。 */
function updateComparisonConfigSummaries() {
  const number = (id) => Number.parseFloat(document.querySelector(`#compare-${id}`).value);
  const payout = document.querySelector("#compare-payout-rule").value === "standard" ? "标准庄" : "庄免佣";
  const strategy = stakeStrategyLabels[compareStakeStrategy.value] ?? "—";
  const bankroll = number("bankroll");
  const rebate = number("rebate-rate");
  const fraction = number("max-fraction");
  const roundLimit = number("max-round-stake");
  const sideLimit = number("side-bet-limit");
  const multi = document.querySelector("#compare-allow-multiple-bets").checked ? "多下注开" : "单一目标";
  setText("#compare-funding-config-summary",
    `${payout} · ${strategy} · 本金 ${Number.isFinite(bankroll) ? money(bankroll) : "—"} · 返水 ${Number.isFinite(rebate) ? rebate : "—"}%`);
  setText("#compare-risk-config-summary",
    `单局 ${Number.isFinite(fraction) ? fraction : "—"}% · 上限 ${Number.isFinite(roundLimit) ? money(roundLimit) : "—"} · 边注 ${Number.isFinite(sideLimit) ? money(sideLimit) : "—"} · ${multi}`);
}

/** 更新每个边注“最后可下注局数”下方的自然语言提示。 */
function updateSideBetRoundLimitHints() {
  for (const prefix of ["", "compare-"]) for (const [key, selector] of Object.entries(sideBetRoundLimitInputs)) {
    const input = document.querySelector(`#${prefix}${selector.slice(1)}`);
    const status = document.querySelector(`#${prefix}${sideBetRoundLimitStatus[key].slice(1)}`);
    if (!input || !status) continue;

    const lastPlayableRound = Number.parseInt(input.value, 10);
    if (!Number.isFinite(lastPlayableRound) || lastPlayableRound < 0) {
      status.textContent = "请输入 0 或正整数";
      status.classList.add("invalid");
    } else if (lastPlayableRound === 0) {
      status.textContent = "不限局数";
      status.classList.remove("invalid");
    } else {
      status.textContent = `第 ${integerFormatter.format(lastPlayableRound + 1)} 局起禁用`;
      status.classList.remove("invalid");
    }
  }
}

/** 根据 consumed/remaining 切换牌面输入的解释，减少输入语义歧义。 */
function updateModeHelp() {
  const consumed = selectedMode() === "consumed";
  modeHelp.textContent = consumed
    ? "留空表示还没有消耗任何牌，将计算完整牌靴的基线结果。"
    : "这里必须输入当前牌靴剩余的全部牌；只输入部分剩余牌会得到另一個牌靴。";
  cardsInput.placeholder = consumed
    ? "例如：AS, 10H, KD, 7C"
    : "例如：AS, 2S, 3S, 4S, 5S, 6S…（全部剩余牌）";
}

function updateBlackjackModeHelp() {
  const consumed = selectedBlackjackMode() === "consumed";
  blackjackModeHelp.textContent = consumed
    ? "填写本手开始前已经发走的牌；可留空。程序会另外扣除玩家两张牌和庄家明牌。"
    : "填写扣除玩家两张牌、庄家明牌和未知暗牌之前，当前牌靴中所有未知牌的完整集合。";
  blackjackShoeCards.placeholder = consumed
    ? "已消耗模式可留空，例如：AS 10H KD 7C"
    : "必须输入当前所有未知剩余牌（不得包含三张可见牌）";
}

/** 只显示当前百家乐子视图；导航本身没有 data-baccarat-view，因此始终保留。 */
function syncBaccaratView() {
  for (const tab of baccaratViewTabs) {
    const active = tab.dataset.baccaratViewTab === activeBaccaratView;
    tab.classList.toggle("active", active);
    tab.setAttribute("aria-selected", String(active));
  }
  for (const section of document.querySelectorAll(".baccarat-section")) {
    const view = section.dataset.baccaratView;
    section.hidden = activeGame !== "baccarat" || Boolean(view && view !== activeBaccaratView);
  }
}

function setActiveBaccaratView(view) {
  activeBaccaratView = view;
  syncBaccaratView();
}

/** 切换百家乐、21 点和规则说明页面，并按需触发当前游戏的计算。 */
function setActiveGame(game) {
  activeGame = game;
  for (const tab of gameTabs) {
    const active = tab.dataset.game === game;
    tab.classList.toggle("active", active);
    tab.setAttribute("aria-selected", String(active));
  }
  syncBaccaratView();
  for (const section of document.querySelectorAll(".blackjack-section")) {
    section.hidden = game !== "blackjack";
  }
  for (const section of document.querySelectorAll(".rules-section")) {
    section.hidden = game !== "rules";
  }
  if (game === "blackjack" && wasmReady) calculateBlackjack();
}

function metricCell(value, emphasize = false) {
  const cell = document.createElement("td");
  cell.textContent = percent(value);
  if (emphasize) cell.className = evClass(value);
  return cell;
}

function renderResults(data, elapsedMilliseconds) {
  // `data` 是 Rust 返回的稳定 JSON。渲染阶段只做标签映射、格式化和 DOM
  // 创建，不重新计算概率总和以外的业务指标；总和仅作为页面一致性提示。
  errorMessage.hidden = true;
  setText("#input-card-count", String(data.input_card_count));
  setText("#remaining-card-count", `${data.remaining_card_count} 张`);

  const keys = ["player", "banker", "tie"];
  const probabilityTotal = keys.reduce(
    (total, key) => total + data.bets[key].probability,
    0,
  );
  setText("#probability-total", percent(probabilityTotal));
  setText("#calculation-time", `${elapsedMilliseconds.toFixed(1)} ms`);

  resultBody.replaceChildren();
  for (const key of keys) {
    const metrics = data.bets[key];
    const row = document.createElement("tr");
    const name = document.createElement("td");
    name.textContent = betLabels[key];
    row.append(
      name,
      metricCell(metrics.probability),
      metricCell(metrics.base_ev),
      metricCell(metrics.rebate_ev),
      metricCell(metrics.effective_ev, true),
      metricCell(metrics.rtp),
    );
    resultBody.append(row);
  }

  sideResultBody.replaceChildren();
  for (const key of Object.keys(sideBetLabels)) {
    const metrics = data.side_bets[key];
    const row = document.createElement("tr");
    const name = document.createElement("td");
    name.textContent = sideBetLabels[key];
    row.append(
      name,
      metricCell(metrics.probability),
      metricCell(metrics.base_ev ?? metrics.ev),
      metricCell(metrics.rebate_ev ?? 0),
      metricCell(metrics.effective_ev ?? metrics.ev, true),
      metricCell(metrics.effective_rtp ?? metrics.rtp),
    );
    sideResultBody.append(row);
  }

  const decision = data.recommendation;
  recommendation.dataset.action = decision.action;
  setText("#recommendation-label", decision.action === "place" ? "本局下注目标" : "当前最优候选");
  setText("#recommended-bet", allBetLabels[decision.candidate_bet]);
  setText("#recommended-ev", percent(decision.effective_ev));
  applySignedClass(document.querySelector("#recommended-ev"), decision.effective_ev);
  setText("#recommended-action", decision.action === "place" ? "可下注" : "跳过");
  setText("#kelly-fraction", percent(decision.kelly_fraction, 3));
  setText("#strategy-fraction", percent(decision.strategy_fraction, 3));
  setText("#applied-fraction", percent(decision.applied_fraction, 3));
  setText("#suggested-amount", money(decision.suggested_amount));
  setText("#expected-profit", money(decision.expected_profit));
  applySignedClass(document.querySelector("#expected-profit"), decision.expected_profit);

  // 多注模式的每个计划都已经由 Rust 完成 EV、凯利和金额上限计算，页面只
  // 负责展示。主推荐仍然保留在上方，表格用于查看同局其余合格目标。
  const plans = Array.isArray(data.recommendations) ? data.recommendations : [decision];
  const placedPlans = plans.filter((plan) => plan.action === "place");
  const totalSuggestedAmount = data.total_suggested_amount
    ?? placedPlans.reduce((total, plan) => total + plan.suggested_amount, 0);
  setText("#summary-amount", money(data.allow_multiple_bets
    ? totalSuggestedAmount
    : decision.suggested_amount));

  if (data.allow_multiple_bets && plans.length > 0) {
    multipleRecommendations.hidden = false;
    setText("#multiple-bet-count", `${placedPlans.length} 笔可下注`);
    setText("#multiple-total-amount", money(totalSuggestedAmount));
    multipleRecommendationBody.replaceChildren();
    for (const plan of plans) {
      const row = document.createElement("tr");
      const actionText = plan.action === "place"
        ? "可下注"
        : (skipReasonLabels[plan.reason] ?? "跳过");
      row.append(
        detailCell(allBetLabels[plan.candidate_bet] ?? plan.candidate_bet),
        detailCell(plan.bet_category === "side" ? "边注" : "主注"),
        detailCell(percent(plan.effective_ev), evClass(plan.effective_ev)),
        detailCell(money(plan.suggested_amount)),
        detailCell(money(plan.expected_profit), evClass(plan.expected_profit)),
        detailCell(actionText),
      );
      multipleRecommendationBody.append(row);
    }
  } else {
    multipleRecommendations.hidden = true;
    multipleRecommendationBody.replaceChildren();
  }

  const targetLimitText = decision.bet_category === "side"
    ? "，并应用边注单独金额上限"
    : "";
  const reason = decision.action === "place"
    ? `${data.allow_multiple_bets ? `已开启同局多下注，共 ${placedPlans.length} 笔计划，合计 ${money(totalSuggestedAmount)}；` : ""}采用${payoutRuleLabels[data.payout_rule]}与${stakeStrategyLabels[data.stake_strategy]}，已通过对应 EV 门槛；建议下${allBetLabels[decision.candidate_bet]}，金额已经过共同风险上限${targetLimitText}。`
    : skipReasonLabels[decision.reason] ?? "当前策略决定跳过本局。";
  setText("#decision-reason", reason);
}

function renderBetBreakdown(breakdown = {}, legacyCounts = {}) {
  // Rust 在真实结算点为每个方向同时累计笔数、下注额和含返水净盈亏。
  // 浏览器不扫描分页明细重新计算，只按固定顺序展示权威汇总；legacyCounts
  // 让旧版回放结果在页面升级后仍能至少显示原有下注笔数。
  betCountGrid.replaceChildren();
  for (const key of betCountOrder) {
    const metrics = breakdown[key] ?? {};
    const item = document.createElement("article");
    item.className = "bet-count-card";
    const header = document.createElement("header");
    const label = document.createElement("span");
    label.className = "bet-count-label";
    const countGroup = document.createElement("span");
    countGroup.className = "bet-count-value";
    const value = document.createElement("strong");
    const unit = document.createElement("small");
    const count = Number(metrics.count ?? legacyCounts[key] ?? 0);
    label.textContent = allBetLabels[key];
    value.textContent = integerFormatter.format(Number.isFinite(count) ? count : 0);
    unit.textContent = "笔";
    countGroup.append(value, unit);
    header.append(label, countGroup);

    const details = document.createElement("dl");
    const stakeRow = document.createElement("div");
    const stakeLabel = document.createElement("dt");
    const stakeValue = document.createElement("dd");
    stakeLabel.textContent = "累计下注";
    stakeValue.textContent = money(Number(metrics.total_stake ?? 0));
    stakeRow.append(stakeLabel, stakeValue);

    const profitRow = document.createElement("div");
    const profitLabel = document.createElement("dt");
    const profitValue = document.createElement("dd");
    const profit = Number(metrics.total_profit ?? 0);
    profitLabel.textContent = "净盈亏";
    profitValue.textContent = signedMoney(profit);
    applySignedClass(profitValue, profit);
    profitRow.append(profitLabel, profitValue);

    details.append(stakeRow, profitRow);
    item.append(header, details);
    betCountGrid.append(item);
  }
}

function showError(target, error) {
  const message = typeof error === "string" ? error : error?.message ?? String(error);
  target.textContent = message;
  target.hidden = false;
}

function calculate() {
  // 这是手工分析的同步入口。WASM 计算直接发生在当前页面线程，适合单个
  // 牌靴状态；大型 CSV 则走下面的 replay Worker，避免阻塞输入和滚动。
  if (!wasmReady) return;

  analyzeButton.disabled = true;
  analyzeButton.textContent = "正在计算…";
  errorMessage.hidden = true;

  try {
    const config = strategyConfig();
    const started = performance.now();
    const json = analyzeBaccaratStrategy(
      selectedMode(),
      config.decks,
      cardsInput.value,
      config.rebateRate,
      config.minimumEffectiveEv,
      config.bankroll,
      config.maxFraction,
      config.maxRoundStake,
      config.tableLimit,
      config.payoutRule,
      config.stakeStrategy,
      config.strategyParameter,
      config.minimumSideBetEv,
      config.sideBetLimit,
      config.allowMultipleBets,
    );
    renderResults(JSON.parse(json), performance.now() - started);
  } catch (error) {
    showError(errorMessage, error);
  } finally {
    analyzeButton.disabled = false;
    analyzeButton.textContent = "计算策略";
  }
}

function blackjackActionCell(action, ev, optimalAction) {
  const row = document.createElement("tr");
  const name = detailCell(blackjackActionLabels[action]);
  const available = ev != null;
  const evCell = detailCell(available ? percent(ev) : "—", available ? evClass(ev) : "");
  const status = detailCell(available ? (action === optimalAction ? "最优" : "可用") : "不可用");
  if (action === optimalAction) row.classList.add("optimal-row");
  row.append(name, evCell, status);
  return row;
}

function renderBlackjack(data, elapsedMilliseconds) {
  // 二十一点的 action EV 已经在 Rust 内部按原始底注口径算好；页面只负责
  // 标记最优动作，并把加倍/分牌需要追加的底注显示出来。
  blackjackError.hidden = true;
  const totalLabel = `${data.player_total}${data.player_soft ? "（软）" : "（硬）"}`;
  setText("#blackjack-player-total", data.player_blackjack ? "天然 21 点" : totalLabel);
  setText("#blackjack-upcard", data.dealer_upcard);
  setText("#blackjack-remaining", `${data.remaining_card_count} 张`);
  setText("#blackjack-additional-stake", money(data.suggested_additional_stake));
  setText("#blackjack-calculation-time", `${elapsedMilliseconds.toFixed(1)} ms`);
  setText("#blackjack-optimal-action", blackjackActionLabels[data.optimal_action]);
  setText("#blackjack-optimal-ev", percent(data.optimal_ev));
  applySignedClass(document.querySelector("#blackjack-optimal-ev"), data.optimal_ev);
  setText("#blackjack-action-pill", blackjackActionLabels[data.optimal_action]);
  setText(
    "#dealer-blackjack-probability",
    percent(data.dealer_blackjack_probability_before_peek),
  );
  setText("#insurance-ev", percent(data.insurance_ev));
  if (data.insurance_ev != null) {
    applySignedClass(document.querySelector("#insurance-ev"), data.insurance_ev);
  } else {
    document.querySelector("#insurance-ev").classList.remove("value-positive", "value-negative");
  }

  blackjackActionBody.replaceChildren();
  for (const action of ["stand", "hit", "double", "split", "surrender"]) {
    blackjackActionBody.append(
      blackjackActionCell(action, data.actions[action], data.optimal_action),
    );
  }

  const condition = data.conditional_on_no_dealer_blackjack
    ? "庄家 A/10 明牌已按美式 Peek 排除暗牌 Blackjack；后续补牌仍保留未知暗牌的后验占牌影响。"
    : "庄家明牌无需 Peek；未知暗牌仍真实占用牌靴。";
  const extra = data.additional_stake_units > 0
    ? `最优动作需在原底注 ${money(data.current_base_stake)} 之外追加 ${money(data.suggested_additional_stake)}。`
    : "最优动作不需要追加筹码。";
  setText("#blackjack-result-note", `${condition} ${extra}`);
}

function calculateBlackjack() {
  // 21 点表单与百家乐表单独立读取，但复用同一个 WASM 初始化状态。
  if (!wasmReady) return;
  blackjackAnalyzeButton.disabled = true;
  blackjackAnalyzeButton.textContent = "正在枚举…";
  blackjackError.hidden = true;

  try {
    const started = performance.now();
    const json = analyzeBlackjack(
      selectedBlackjackMode(),
      Number.parseInt(document.querySelector("#blackjack-decks").value, 10),
      blackjackShoeCards.value,
      blackjackPlayerCards.value,
      blackjackDealerUpcard.value,
      document.querySelector("#dealer-soft-17").value === "hit",
      Number.parseFloat(document.querySelector("#blackjack-payout").value),
      document.querySelector("#late-surrender").value === "yes",
      readNumber("#blackjack-base-stake", "当前原始底注", { positive: true }),
    );
    renderBlackjack(JSON.parse(json), performance.now() - started);
  } catch (error) {
    showError(blackjackError, error);
  } finally {
    blackjackAnalyzeButton.disabled = false;
    blackjackAnalyzeButton.textContent = "计算动作 EV";
  }
}

function updateReplayButton() {
  const hasInput = replaySourceMode === "simulation" || Boolean(currentCsvFile);
  replayButton.disabled = !hasInput || !replayWorkerReady || replayRunning;
}

/**
 * 更新回测进度条；百分比由后台按“生成、概率、结算”三个阶段合成。
 *
 * Worker 消息可能因为并行任务完成顺序不同而交错到达，所以页面只接受
 * 单调递增的进度。否则例如“生成完成 15%”后收到“自动测速 3%”，进度条
 * 就会倒退，看起来像页面闪烁或重复计算。
 */
function setReplayProgress(
  overall,
  label,
  detail = "",
  { allowDecrease = false, indeterminate = false } = {},
) {
  if (!replayProgressBar || !replayProgressValue || !replayProgressLabel) return;
  const safeOverall = Math.max(0, Math.min(1, Number(overall) || 0));
  const now = performance.now();
  if (allowDecrease || replayProgressStartedAt === 0) {
    replayProgressStartedAt = now;
    replayProgressLastSampleAt = now;
    replayProgressLastSampleOverall = safeOverall;
    replayProgressRate = 0;
  }
  replayProgressOverall = allowDecrease
    ? safeOverall
    : Math.max(replayProgressOverall, safeOverall);
  const sampleElapsed = now - replayProgressLastSampleAt;
  const progressDelta = replayProgressOverall - replayProgressLastSampleOverall;
  // 生成百万靴时每一靴只贡献极小的总体进度；必须跨一段时间累计样本，
  // 不能按单条 Worker 消息计算，否则进度增量会小到被阈值吞掉。
  if (!indeterminate && sampleElapsed >= 500 && progressDelta > 0) {
    const currentRate = progressDelta / sampleElapsed;
    replayProgressRate = replayProgressRate > 0
      ? replayProgressRate * 0.7 + currentRate * 0.3
      : currentRate;
    replayProgressLastSampleAt = now;
    replayProgressLastSampleOverall = replayProgressOverall;
  }
  const percentage = Math.round(replayProgressOverall * 100);
  replayProgressPanel?.removeAttribute("hidden");
  replayProgressBar.style.width = `${percentage}%`;
  replayProgressBar.classList.toggle("is-indeterminate", indeterminate);
  replayProgressValue.textContent = `${percentage}%`;
  replayProgressLabel.textContent = detail ? `${label} · ${detail}` : label;
  if (replayProgressEta) {
    if (replayProgressOverall >= 0.999) {
      replayProgressEta.textContent = "已完成";
    } else if (indeterminate || replayProgressRate <= 0 || now - replayProgressStartedAt < 1_000) {
      replayProgressEta.textContent = "预计剩余：计算中";
    } else {
      replayProgressEta.textContent = `预计剩余：${formatReplayDuration((1 - replayProgressOverall) / replayProgressRate)}`;
    }
  }
  replayProgressBar.setAttribute("aria-valuenow", String(percentage));
  replayProgressTrack?.setAttribute("aria-valuenow", String(percentage));
}

function formatReplayDuration(milliseconds) {
  const seconds = Math.max(1, Math.round(Number(milliseconds) / 1_000));
  if (seconds < 60) return `${seconds} 秒`;
  const minutes = Math.floor(seconds / 60);
  const remainingSeconds = seconds % 60;
  if (minutes < 60) return `${minutes} 分 ${String(remainingSeconds).padStart(2, "0")} 秒`;
  const hours = Math.floor(minutes / 60);
  return `${hours} 小时 ${String(minutes % 60).padStart(2, "0")} 分`;
}

function resetReplayProgress(label = "等待开始") {
  setReplayProgress(0, label, "", { allowDecrease: true });
}

function updateParallelReplayControls() {
  const enabled = parallelReplay.checked;
  parallelWorkerCount.disabled = !enabled || replayRunning;
  parallelWorkerCount.closest("label")?.classList.toggle(
    "is-disabled",
    !enabled || replayRunning,
  );
  parallelAutoTune.disabled = !enabled || replayRunning;
  parallelAutoTune.closest("label")?.classList.toggle(
    "is-disabled",
    !enabled || replayRunning,
  );
}

function setReplayRunning(running, label) {
  replayRunning = running;
  replayStatus.textContent = label;
  replayStatus.classList.toggle("running", running);
  replayButton.textContent = running
    ? "正在回测…"
    : replaySourceMode === "simulation" ? "生成并开始回测" : "开始 CSV 回放";
  for (const tab of replaySourceTabs) tab.disabled = running;
  parallelReplay.disabled = running;
  summaryOnlyReplay.disabled = running;
  updateParallelReplayControls();
  updateReplayButton();
}

function maximumGuaranteedSimulationRounds() {
  return Math.floor(Number.parseInt(deckCount.value, 10) * 52 / 6);
}

function updateSimulationEstimate({ clampRounds = false } = {}) {
  const maximumRounds = maximumGuaranteedSimulationRounds();
  simulationRounds.max = String(maximumRounds);
  if (clampRounds && Number.parseInt(simulationRounds.value, 10) > maximumRounds) {
    simulationRounds.value = String(maximumRounds);
  }

  const shoes = Number.parseInt(simulationShoes.value, 10);
  const rounds = Number.parseInt(simulationRounds.value, 10);
  const maxShoes = Number.isInteger(rounds) && rounds > 0
    ? Math.min(MAX_SIMULATION_SHOES, Math.floor(MAX_REPLAY_ROUNDS / rounds))
    : MAX_SIMULATION_SHOES;
  simulationShoes.max = String(maxShoes);
  simulationShoes.setCustomValidity(shoes > maxShoes ? "单次回测最多支持 1,666,666 局" : "");
  if (!Number.isInteger(shoes) || shoes < 1 || !Number.isInteger(rounds) || rounds < 1) {
    simulationEstimate.textContent = `当前 ${deckCount.value} 副牌最多保证每靴生成 ${maximumRounds} 局。`;
  } else if (rounds > maximumRounds) {
    simulationEstimate.textContent = `当前 ${deckCount.value} 副牌最多保证每靴生成 ${maximumRounds} 局，请调小子局数。`;
  } else if (shoes * rounds > MAX_REPLAY_ROUNDS) {
    simulationEstimate.textContent = `超过单次 1,666,666 局上限；每靴 ${rounds} 局时最多 ${integerFormatter.format(maxShoes)} 靴。`;
  } else {
    simulationEstimate.textContent = `预计生成 ${integerFormatter.format(shoes)} 靴，共 ${integerFormatter.format(shoes * rounds)} 局（单次上限 1,666,666 局）。`;
  }
  updateReplayButton();
}

function simulationRequest() {
  const shoes = readNumber("#simulation-shoes", "生成牌靴数", {
    min: 1,
    max: MAX_SIMULATION_SHOES,
    integer: true,
  });
  const maxRoundsPerShoe = readNumber("#simulation-rounds", "每靴最大子局数", {
    min: 1,
    max: maximumGuaranteedSimulationRounds(),
    integer: true,
  });
  const seed = simulationSeed.value.trim();
  if (shoes * maxRoundsPerShoe > MAX_REPLAY_ROUNDS) {
    throw new Error("单次回测最多支持 1,666,666 局，请减少牌靴数或每靴局数");
  }
  if (!/^\d{1,20}$/.test(seed) || BigInt(seed) > 18_446_744_073_709_551_615n) {
    throw new Error("随机种子必须是 0 到 18446744073709551615 之间的整数");
  }
  return { shoes, maxRoundsPerShoe, seed };
}

function setReplaySourceMode(mode) {
  replaySourceMode = mode;
  const simulation = mode === "simulation";
  csvReplaySource.hidden = simulation;
  simulationReplaySource.hidden = !simulation;
  for (const tab of replaySourceTabs) {
    const active = tab.dataset.replaySource === mode;
    tab.classList.toggle("active", active);
    tab.setAttribute("aria-selected", String(active));
  }

  if (simulation) {
    replayRulesTitle.textContent = "随机回测说明";
    replayRulePrimary.textContent = "Rust 会创建完整牌靴、按种子洗牌，并依照真实补牌规则逐局发牌。";
    replayRuleSecondary.textContent = "副牌数和全部资金策略沿用页面上方配置，无需准备或上传 CSV。";
    replayRuleOrder.textContent = "相同牌靴数、子局数和种子会得到完全相同的牌局，适合比较策略。";
    replayStatus.textContent = replayWorkerReady ? "可以开始随机回测" : "正在准备回测核心…";
    updateSimulationEstimate();
  } else {
    replayRulesTitle.textContent = "CSV 回放前提";
    replayRulePrimary.textContent = "只需 session_id、round_no、raw_cards 三列，也支持中文列名。";
    replayRuleSecondary.textContent = "只有从第 1 局开始、局号连续且牌面合法的牌靴会进入策略计算。";
    replayRuleOrder.textContent = "完整格式按时间排序；精简格式按 CSV 行顺序回放。";
    replayStatus.textContent = currentCsvFile
      ? replayWorkerReady ? "可以开始回放" : "正在准备回放核心…"
      : "等待选择文件";
  }
  setReplayRunning(false, replayStatus.textContent);
}

function detailCell(value, className = "") {
  const cell = document.createElement("td");
  cell.textContent = value;
  if (className) cell.className = className;
  return cell;
}

function appendCardLine(container, label, cardsText, total) {
  const labelNode = document.createElement("span");
  labelNode.className = "hand-label";
  labelNode.textContent = `${label} `;
  container.append(labelNode);

  const cards = cardsText.trim().split(/\s+/).filter(Boolean);
  cards.forEach((rawCard, index) => {
    if (index > 0) container.append(document.createTextNode(" "));

    const normalized = rawCard.toUpperCase();
    const match = normalized.match(/^(10|[2-9]|[AJQK])([CDHS])$/);
    const card = document.createElement("span");
    card.className = "replay-card";
    card.title = rawCard;
    if (match) {
      const suit = match[2];
      card.classList.add(redSuits.has(suit) ? "suit-red" : "suit-black");
      card.textContent = `${match[1]}${suitSymbols[suit]}`;
    } else {
      // 未来如果供应商返回了新牌面格式，仍保留原始值，不让整行消失。
      card.textContent = rawCard;
    }
    container.append(card);
  });

  const totalNode = document.createElement("span");
  totalNode.className = "hand-total";
  totalNode.textContent = ` · ${total} 点`;
  container.append(totalNode);
}

function outcomeDetailCell(bet) {
  const cell = document.createElement("td");
  cell.className = "outcome-detail-cell";

  const result = document.createElement("strong");
  result.textContent = `${betLabels[bet.outcome]} · ${resultLabels[bet.result]}`;

  const playerCards = document.createElement("span");
  playerCards.className = "hand-line";
  appendCardLine(playerCards, "闲", bet.player_cards, bet.player_total);

  const bankerCards = document.createElement("span");
  bankerCards.className = "hand-line";
  appendCardLine(bankerCards, "庄", bet.banker_cards, bet.banker_total);

  cell.append(result, playerCards, bankerCards);
  return cell;
}

function replayDetailCount(report) {
  if (report?.details_saved === false) return 0;
  const reportBets = Array.isArray(report?.bets) ? report.bets.length : 0;
  return Math.max(reportBets, Number(report?.detail_count ?? report?.summary?.placed_bet_count ?? 0));
}

function setReplayExportAvailability(available) {
  replayExportFormat.disabled = !available;
  replayExportButton.disabled = !available;
}

async function forEachReplayDetailPage(report, callback) {
  const sourceBets = Array.isArray(report.bets) && report.bets.length > 0 ? report.bets : null;
  const count = replayDetailCount(report);
  const pageSize = 2_000;
  for (let offset = 0; offset < count;) {
    const page = sourceBets
      ? sourceBets.slice(offset, Math.min(offset + pageSize, count))
      : await readReplayDetails(report.run_id, offset, Math.min(pageSize, count - offset));
    if (!page.length) break;
    await callback(page, offset + page.length, count);
    offset += page.length;
  }
}

function triggerReplayDownload(file) {
  const url = URL.createObjectURL(file.blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = file.filename;
  link.hidden = true;
  document.body.append(link);
  link.click();
  link.remove();
  setTimeout(() => URL.revokeObjectURL(url), 0);
}

async function exportReplayDetails() {
  const report = currentReplayReport;
  if (!report || replayExportButton.disabled) return;
  const format = replayExportFormat.value;
  const detailCount = replayDetailCount(report);
  if (report.details_saved === false) {
    replayExportStatus.textContent = "本次只保存了汇总，没有可导出的下注明细。";
    return;
  }
  if (Number(report.omitted_bet_details ?? 0) > 0) {
    replayExportStatus.textContent = "明细不完整，请重新运行回测后导出。";
    return;
  }

  replayExportButton.disabled = true;
  replayExportFormat.disabled = true;
  replayExportStatus.textContent = `正在流式导出 ${integerFormatter.format(detailCount)} 笔明细…`;
  const encoder = createBetDetailExportEncoder(format, report.summary);
  let writable = null;
  try {
    let parts = null;
    if (typeof globalThis.showSaveFilePicker === "function") {
      const handle = await globalThis.showSaveFilePicker({
        suggestedName: encoder.filename,
        types: [{
          description: EXPORT_FORMATS[format].label,
          accept: { [encoder.mime.split(";")[0]]: [`.${encoder.extension}`] },
        }],
      });
      writable = await handle.createWritable();
      await writable.write(encoder.start());
    } else {
      // Safari/Firefox 等没有文件流 API 时仍然按页编码，不再保存完整下注对象；
      // 最后仅由 Blob 组合已编码的文本块完成传统下载。
      parts = [encoder.start()];
    }
    await forEachReplayDetailPage(report, async (page, completed, total) => {
      const chunk = encoder.append(page);
      if (writable) await writable.write(chunk);
      else parts.push(chunk);
      replayExportStatus.textContent = `正在流式导出 ${integerFormatter.format(completed)} / ${integerFormatter.format(total)} 笔…`;
    });
    if (encoder.count !== detailCount) {
      throw new Error(`仅读取到 ${encoder.count} / ${detailCount} 笔明细，请重新运行回测`);
    }
    const footer = encoder.finish();
    if (writable) {
      await writable.write(footer);
      await writable.close();
      writable = null;
    } else {
      parts.push(footer);
      triggerReplayDownload({
        blob: new Blob(parts, { type: encoder.mime }), filename: encoder.filename,
      });
    }
    replayExportStatus.textContent = `已导出 ${integerFormatter.format(encoder.count)} 笔明细 · ${EXPORT_FORMATS[format].extension.toUpperCase()}`;
  } catch (error) {
    if (writable) await writable.abort().catch(() => {});
    replayExportStatus.textContent = error?.name === "AbortError"
      ? "已取消导出。" : `导出失败：${error?.message ?? error}`;
  } finally {
    const available = Boolean(currentReplayReport)
      && replayDetailCount(currentReplayReport) > 0
      && currentReplayReport.details_saved !== false
      && Number(currentReplayReport.omitted_bet_details ?? 0) === 0;
    setReplayExportAvailability(available);
  }
}

async function renderReplayDetails() {
  // 大回测的明细已按批写入 IndexedDB；这里一次只读取当前页，避免把百万条
  // 下注重新装回 JavaScript 堆，也避免表格一次创建百万个 DOM 节点。
  if (!currentReplayReport) return;
  const token = ++replayDetailRenderToken;
  const report = currentReplayReport;
  const { bets, omitted_bet_details: omittedBetDetails, summary } = report;
  const summaryOnly = report.details_saved === false;
  const sourceBets = Array.isArray(bets) && bets.length > 0 ? bets : null;
  const detailCount = replayDetailCount(report);
  const pageSize = Number.parseInt(replayPageSize.value, 10);
  const totalPages = Math.max(1, Math.ceil(detailCount / pageSize));
  currentReplayPage = Math.min(Math.max(currentReplayPage, 1), totalPages);
  const startIndex = (currentReplayPage - 1) * pageSize;
  const endIndex = Math.min(startIndex + pageSize, detailCount);
  let visibleBets = sourceBets ? sourceBets.slice(startIndex, endIndex) : [];

  if (!sourceBets && detailCount > 0 && report.run_id) {
    setText("#detail-note", "正在从浏览器本地存储读取当前页明细…");
    try {
      visibleBets = await readReplayDetailPage(report.run_id, startIndex, pageSize);
    } catch (error) {
      if (token !== replayDetailRenderToken || currentReplayReport !== report) return;
      setText("#detail-note", `明细读取失败：${error?.message ?? error}`);
      visibleBets = [];
    }
  }
  if (token !== replayDetailRenderToken || currentReplayReport !== report) return;

  setText("#replayed-rounds", integerFormatter.format(summary.replayed_rounds));
  replayBody.replaceChildren();
  for (const bet of visibleBets) {
    const row = document.createElement("tr");
    const resultClass = bet.actual_profit > 0
      ? "value-positive"
      : bet.actual_profit < 0
        ? "value-negative"
        : "";
    row.append(
      detailCell(bet.started_at),
      detailCell(`${bet.table_id} / ${bet.session_id} / ${bet.round_no}`),
      detailCell(allBetLabels[bet.bet]),
      outcomeDetailCell(bet),
      detailCell(percent(bet.effective_ev), evClass(bet.effective_ev)),
      detailCell(money(bet.amount)),
      detailCell(money(bet.base_game_profit), evClass(bet.base_game_profit)),
      detailCell(money(bet.rebate_income), evClass(bet.rebate_income)),
      detailCell(money(bet.actual_profit), resultClass),
      detailCell(money(bet.bankroll_after), evClass(bet.bankroll_after - summary.initial_bankroll)),
    );
    replayBody.append(row);
  }

  if (visibleBets.length === 0) {
    const row = document.createElement("tr");
    row.className = "placeholder-row";
    const cell = document.createElement("td");
    cell.colSpan = 10;
    cell.textContent = summaryOnly
      ? "本次选择了“只保存汇总”，未保存下注明细。"
      : summary.replayed_rounds === 0
      ? "没有可从第 1 局完整重建的牌靴，请查看隔离局数。"
      : detailCount === 0
        ? "没有任何一局同时通过 EV 门槛和所选资金策略检查。"
        : "当前页没有可读取的明细，请重新运行回测。";
    row.append(cell);
    replayBody.append(row);
  }

  replayPagination.hidden = detailCount === 0;
  setReplayExportAvailability(!summaryOnly && detailCount > 0 && Number(omittedBetDetails ?? 0) === 0);
  replayFirstPage.disabled = currentReplayPage === 1;
  replayPreviousPage.disabled = currentReplayPage === 1;
  replayNextPage.disabled = currentReplayPage === totalPages;
  replayLastPage.disabled = currentReplayPage === totalPages;
  replayPageStatus.textContent = `第 ${integerFormatter.format(currentReplayPage)} / ${integerFormatter.format(totalPages)} 页`;

  if (summaryOnly) {
    setText("#detail-note", `已计算 ${integerFormatter.format(summary.placed_bet_count)} 笔下注，但未保存明细；如需导出，请重新回测并关闭“只保存汇总”。`);
  } else if (omittedBetDetails > 0) {
    setText("#detail-note", `仍有 ${integerFormatter.format(omittedBetDetails)} 笔明细未包含；请重新运行回放。`);
  } else if (detailCount === 0) {
    setText("#detail-note", "本次策略没有产生可下注明细。");
  } else if (totalPages === 1) {
    setText("#detail-note", `共 ${integerFormatter.format(detailCount)} 笔下注，已显示全部明细。`);
  } else {
    setText(
      "#detail-note",
      `共 ${integerFormatter.format(detailCount)} 笔下注；当前显示第 ${integerFormatter.format(startIndex + 1)}–${integerFormatter.format(endIndex)} 笔，可翻页查看全部明细。`,
    );
  }
  replayDetailWrap.scrollTop = 0;
}

function renderReplay(report, elapsedMilliseconds, timings = {}) {
  // 汇总卡片、下注分类、风险指标和本金图都来自同一份 Rust 回放报告。
  // 先更新摘要，再交给图表和明细表，避免用户看到新摘要配旧图表。
  replayError.hidden = true;
  replayResults.hidden = false;
  replayStreakPanel.hidden = false;
  currentReplayReport = report;
  currentReplayPage = 1;
  const { dataset, quality, summary } = report;

  setText("#replayed-rounds", integerFormatter.format(summary.replayed_rounds));
  setText("#placed-bets", integerFormatter.format(summary.placed_bet_count));
  setText("#total-stake", money(summary.total_stake));
  setText("#final-bankroll", money(summary.final_bankroll));
  setText("#total-profit", money(summary.total_profit));
  applySignedClass(document.querySelector("#total-profit"), summary.total_profit);
  setText("#rebate-income", money(summary.rebate_income));
  setText("#return-on-initial", percent(summary.return_on_initial, 2));
  applySignedClass(document.querySelector("#return-on-initial"), summary.return_on_initial);
  setText(
    "#maximum-drawdown",
    `${money(summary.maximum_drawdown)} · ${percent(summary.maximum_drawdown_rate, 2)}`,
  );
  setText("#maximum-profit", money(summary.maximum_profit));
  applySignedClass(document.querySelector("#maximum-profit"), summary.maximum_profit);
  setText("#maximum-bankroll", money(summary.maximum_bankroll));
  setText("#minimum-bankroll", money(summary.minimum_bankroll));
  setText(
    "#maximum-consecutive-wins",
    integerFormatter.format(Number(summary.maximum_consecutive_wins ?? 0)),
  );
  setText(
    "#maximum-consecutive-losses",
    integerFormatter.format(Number(summary.maximum_consecutive_losses ?? 0)),
  );
  setText("#maximum-single-stake", money(summary.maximum_single_stake));
  setText("#maximum-round-stake", money(summary.maximum_round_stake));

  const stopNotice = document.querySelector("#replay-stop-notice");
  if (stopNotice) {
    const stoppedEarly = Boolean(summary.stopped_early);
    stopNotice.hidden = !stoppedEarly;
    if (stoppedEarly) {
      const location = [summary.stop_table_id, summary.stop_session_id, summary.stop_round_no]
        .every((value) => Number.isFinite(Number(value)))
        ? `桌台 ${summary.stop_table_id} · 牌靴 ${summary.stop_session_id} · 第 ${summary.stop_round_no} 局`
        : `第 ${summary.replayed_rounds + 1} 个待处理局`;
      stopNotice.textContent = `回放已提前结束：本金已耗尽，停止位置为${location}；已完成 ${integerFormatter.format(summary.replayed_rounds)} 局，期末本金 ${money(summary.final_bankroll)}。`;
    } else {
      stopNotice.textContent = "";
    }
  }
  renderBetBreakdown(summary.bet_breakdown, summary.placed_bets);

  setText("#dataset-rows", integerFormatter.format(dataset.total_rows));
  setText("#valid-sessions", integerFormatter.format(quality.fully_observable_sessions));
  setText("#quarantined-rounds", integerFormatter.format(quality.quarantined_rounds));
  setText("#valid-card-rows", integerFormatter.format(quality.valid_card_rows));
  setText("#hit-rate", percent(summary.hit_rate, 2));
  setText("#replay-time", `${(elapsedMilliseconds / 1000).toFixed(2)} 秒`);
  const milliseconds = value => Number.isFinite(Number(value))
    ? `${(Number(value) / 1000).toFixed(2)} 秒`
    : "—";
  setText("#replay-input-time", milliseconds(timings.generationMs));
  setText("#replay-prepare-time", milliseconds(timings.prepareMs));
  setText("#replay-settlement-time", milliseconds(timings.settlementMs));
  setText("#replay-storage-time", milliseconds(timings.storageMs));
  setText(
    "#replay-throughput",
    Number.isFinite(Number(timings.roundsPerSecond)) && Number(timings.roundsPerSecond) > 0
      ? `${Number(timings.roundsPerSecond).toFixed(0)} 局/秒`
      : "—",
  );

  renderStrategyComparison(report);
  bankrollChartController.render(report);
  replayTrendChartController.render(report);
  contributionChartController.render(report);
  replayAnalysisChartController.render(report);
  renderReplayDetails();
  // 不自动平滑滚动：结果区出现时强制滚动会让整页突然跳动，尤其在移动端
  // 看起来像页面闪烁。连续表现卡片已经位于结果区顶部，用户可自然向下查看。
}

function renderStrategyComparison(report) {
  const section = document.querySelector("#strategy-compare-results");
  const description = document.querySelector("#bankroll-chart-description");
  const legend = document.querySelector("#bankroll-compare-legend");
  const comparison = report.comparison;
  section.hidden = !comparison;
  legend.hidden = !comparison;
  setText("#bankroll-primary-legend", comparison ? "策略 A" : "结算后本金");
  const distinctStartingBankrolls = Boolean(comparison)
    && Number(report.summary?.initial_bankroll) !== Number(comparison.summary?.initial_bankroll);
  document.querySelector("#bankroll-compare-baseline-legend").hidden = !distinctStartingBankrolls;
  setText("#bankroll-primary-baseline-legend",
    comparison ? distinctStartingBankrolls ? "A 初始本金" : "共同初始本金" : "初始本金");
  description.textContent = comparison
    ? comparison.timeline_mode === "bet_union"
      ? "横轴按两套策略实际下注局的共同时间顺序对齐；未下注局没有资金点。"
      : "横轴为同一输入流中的牌局顺序；A、B 共用牌局，只在各自下注时产生资金点。"
    : "横轴按实际下注结算局排列；同局多注合并成一个资金点，跳过局不会重复绘制。";
  if (!comparison) return;

  const a = report.summary;
  const b = comparison.summary;
  const aLabel = stakeStrategyLabels[report.primary_strategy] ?? "策略 A";
  const bLabel = stakeStrategyLabels[comparison.stake_strategy] ?? "策略 B";
  setText("#compare-a-heading", `A · ${aLabel}`);
  setText("#compare-b-heading", `B · ${bLabel}`);
  const rows = [
    ["初始本金", "initial_bankroll", money, money],
    ["期末本金", "final_bankroll", money, money],
    ["最终净盈亏", "total_profit", money, money],
    ["本金收益率", "return_on_initial", value => percent(value, 2), value => percent(value, 2)],
    ["累计下注额", "total_stake", money, money],
    ["实际下注笔数", "placed_bet_count", value => integerFormatter.format(value), value => integerFormatter.format(value)],
    ["命中率", "hit_rate", value => percent(value, 2), value => percent(value, 2)],
    ["返水收入", "rebate_income", money, money],
    ["最大回撤", "maximum_drawdown", money, money],
    ["最大连输局数", "maximum_consecutive_losses", value => integerFormatter.format(value), value => integerFormatter.format(value)],
    ["最低本金", "minimum_bankroll", money, money],
  ];
  const body = document.querySelector("#strategy-compare-body");
  body.replaceChildren(...rows.map(([label, key, format, formatDifference]) => {
    const tr = document.createElement("tr");
    const title = document.createElement("th");
    title.scope = "row";
    title.textContent = label;
    tr.append(title);
    for (const value of [a[key], b[key]]) {
      const cell = document.createElement("td");
      cell.textContent = format(Number(value) || 0);
      tr.append(cell);
    }
    const delta = Number(b[key] ?? 0) - Number(a[key] ?? 0);
    const difference = document.createElement("td");
    difference.textContent = `${delta > 0 ? "+" : ""}${formatDifference(delta)}`;
    tr.append(difference);
    return tr;
  }));
  setText(
    "#strategy-compare-note",
    `A ${a.stopped_early ? "提前停止" : "完成回测"}（${integerFormatter.format(a.replayed_rounds)} 局）；B ${b.stopped_early ? "提前停止" : "完成回测"}（${integerFormatter.format(b.replayed_rounds)} 局）。${distinctStartingBankrolls ? "初始本金不同，建议重点比较收益率与净盈亏。" : ""}B − A 只表示数值差，不代表每个指标越大越好。`,
  );
}

form.addEventListener("submit", (event) => {
  event.preventDefault();
  calculate();
});

// 事件监听只负责把用户动作路由到对应的“读取配置 -> 调用核心 -> 渲染”入口。
// 业务规则不写在事件回调里，便于后续增加自动刷新或其他输入方式。
blackjackForm.addEventListener("submit", (event) => {
  event.preventDefault();
  calculateBlackjack();
});

blackjackAnalyzeButton.addEventListener("click", calculateBlackjack);

for (const tab of gameTabs) {
  tab.addEventListener("click", () => setActiveGame(tab.dataset.game));
}

for (const tab of baccaratViewTabs) {
  tab.addEventListener("click", () => setActiveBaccaratView(tab.dataset.baccaratViewTab));
}

form.addEventListener("input", updateConfigSummaries);
form.addEventListener("change", updateConfigSummaries);
blackjackForm.addEventListener("input", updateConfigSummaries);
blackjackForm.addEventListener("change", updateConfigSummaries);

blackjackForm.elements["blackjack-source-mode"].forEach((radio) => {
  radio.addEventListener("change", updateBlackjackModeHelp);
});

blackjackSampleButton.addEventListener("click", () => {
  blackjackForm.elements["blackjack-source-mode"].value = "consumed";
  blackjackShoeCards.value = "";
  blackjackPlayerCards.value = "5S 6H";
  blackjackDealerUpcard.value = "6C";
  updateBlackjackModeHelp();
  calculateBlackjack();
});

form.elements["source-mode"].forEach((radio) => {
  radio.addEventListener("change", updateModeHelp);
});

stakeStrategy.addEventListener("change", () => {
  updateStakeStrategyFields();
  calculate();
});
compareStrategies.addEventListener("change", () => {
  if (compareStrategies.checked) copyPrimaryConfigToComparison();
  else updateComparisonStrategyFields(false);
});
compareStakeStrategy.addEventListener("change", () => {
  updateComparisonStrategyFields(true);
  updateComparisonConfigSummaries();
});
compareCopyPrimary.addEventListener("click", copyPrimaryConfigToComparison);
for (const eventType of ["input", "change"]) {
  compareStrategyFields.addEventListener(eventType, () => {
    updateComparisonConfigSummaries();
    updateSideBetRoundLimitHints();
  });
}

payoutRule.addEventListener("change", calculate);
allowMultipleBets.addEventListener("change", calculate);

for (const selector of Object.values(sideBetRoundLimitInputs)) {
  document.querySelector(selector).addEventListener("input", updateSideBetRoundLimitHints);
}

sampleButton.addEventListener("click", () => {
  form.elements["source-mode"].value = "consumed";
  cardsInput.value = "AS, 10H, KD, 7C, 3D, QH";
  updateModeHelp();
  calculate();
});

clearButton.addEventListener("click", () => {
  cardsInput.value = "";
  cardsInput.focus();
});

for (const tab of replaySourceTabs) {
  tab.addEventListener("click", () => setReplaySourceMode(tab.dataset.replaySource));
}

for (const input of [simulationShoes, simulationRounds, simulationSeed]) {
  input.addEventListener("input", () => updateSimulationEstimate());
}

parallelReplay.addEventListener("change", updateParallelReplayControls);
parallelWorkerCount.addEventListener("input", updateReplayButton);
updateParallelReplayControls();

deckCount.addEventListener("change", () => updateSimulationEstimate({ clampRounds: true }));

csvFileInput.addEventListener("change", () => {
  const [file] = csvFileInput.files;
  currentCsvFile = file ?? null;
  setReplayExportAvailability(false);
  replayExportStatus.textContent = "完成回测后可导出下注明细";
  replayResults.hidden = true;
  replayStreakPanel.hidden = true;
  document.querySelector("#strategy-compare-results").hidden = true;
  replayError.hidden = true;
  currentReplayReport = null;
  replayPagination.hidden = true;
  bankrollChartController.reset();
  replayTrendChartController.reset();
  contributionChartController.reset();
  replayAnalysisChartController.reset();

  if (!currentCsvFile) {
    selectedFile.textContent = "尚未选择文件";
    replayStatus.textContent = "等待选择文件";
  } else {
    selectedFile.textContent = `${currentCsvFile.name} · ${(currentCsvFile.size / 1024 / 1024).toFixed(2)} MB`;
    replayStatus.textContent = replayWorkerReady ? "可以开始回放" : "正在准备回放核心…";
  }
  updateReplayButton();
});

replayPageSize.addEventListener("change", () => {
  currentReplayPage = 1;
  renderReplayDetails();
});

replayFirstPage.addEventListener("click", () => {
  currentReplayPage = 1;
  renderReplayDetails();
});

replayPreviousPage.addEventListener("click", () => {
  currentReplayPage -= 1;
  renderReplayDetails();
});

replayNextPage.addEventListener("click", () => {
  currentReplayPage += 1;
  renderReplayDetails();
});

replayLastPage.addEventListener("click", () => {
  if (!currentReplayReport) return;
  const pageSize = Number.parseInt(replayPageSize.value, 10);
  const detailCount = Number(currentReplayReport.detail_count ?? currentReplayReport.bets?.length ?? 0);
  currentReplayPage = Math.max(1, Math.ceil(detailCount / pageSize));
  renderReplayDetails();
});

replayExportButton.addEventListener("click", exportReplayDetails);

replayButton.addEventListener("click", async () => {
  if (replayRunning || (replaySourceMode === "csv" && !currentCsvFile)) return;
  replayError.hidden = true;
  replayResults.hidden = true;
  replayStreakPanel.hidden = true;
  currentReplayReport = null;
  setReplayExportAvailability(false);
  replayExportStatus.textContent = "正在生成新的回测明细…";
  replayPagination.hidden = true;
  replayBody.replaceChildren();
  if (replayDetailFlushTimer) {
    clearTimeout(replayDetailFlushTimer);
    replayDetailFlushTimer = null;
  }
  replayDetailPendingBatches = [];
  replayStorageElapsedMs = 0;
  resetReplayProgress("准备回测");
  bankrollChartController.reset("正在回放，完成后显示新的本金变化曲线…");
  replayTrendChartController.reset();
  contributionChartController.reset();
  replayAnalysisChartController.reset();

  try {
    // 配置在主线程读取一次，再与 CSV 文本一起传给 Worker；Worker 不直接访问
    // DOM，因此所有页面输入都必须在这里变成可结构化传输的普通数据。
    const previousRunId = activeReplayRunId;
    activeReplayRunId = `run-${Date.now()}-${Math.random().toString(36).slice(2)}`;
    replayDetailStore?.close();
    const saveReplayDetails = !summaryOnlyReplay.checked;
    // 本轮明细由协调 Worker 直接写入，后台页面不参与计算背压。
    replayDetailStore = null;
    replayDetailSequence = 0;
    replayDetailWriteTail = Promise.resolve();
    globalThis.__replayDetailWriteTail = replayDetailWriteTail;
    // 只清理本应用上一轮生成的回测库；用户的其他 IndexedDB 不会被触碰。
    if (previousRunId && previousRunId !== activeReplayRunId) await deleteReplayStore(previousRunId);

    const config = {
      ...strategyConfig(),
      comparisonConfig: comparisonStrategyConfig(),
      runId: activeReplayRunId,
      saveReplayDetails,
      workerDetailStorage: true,
      parallelReplay: parallelReplay.checked,
      parallelWorkerCount: parallelReplay.checked
        ? readNumber("#parallel-worker-count", "并行数", { min: 1, max: 8, integer: true }) : 1,
      autoTuneWorkers: parallelReplay.checked && parallelAutoTune.checked,
    };
    if (!replayWorker) resetReplayWorker();
    if (replaySourceMode === "simulation") {
      const simulation = simulationRequest();
      setReplayRunning(true, "正在生成牌靴并回测策略…");
      replayWorker.postMessage({ type: "simulate", simulation, config });
    } else {
      if (currentCsvFile.size > 200 * 1024 * 1024) {
        throw new Error("CSV 超过 200 MB；请先按牌靴拆分后再回放。");
      }
      setReplayRunning(true, "正在重建牌靴并计算策略…");
      // File 可结构化传给 Worker；并行回放时 Worker 会按 Blob 流逐行拆分精简 CSV，
      // 不在主线程复制整份文件；带完整来源键的数据库格式仍由 Rust 完整校验。
      replayWorker.postMessage({ type: "replay", csvFile: currentCsvFile, config });
    }
  } catch (error) {
    setReplayRunning(false, "回放失败");
    bankrollChartController.reset("回放失败；修正文件或配置后重新运行即可生成本金变化曲线。");
    replayTrendChartController.reset();
    contributionChartController.reset();
    replayAnalysisChartController.reset();
    showError(replayError, error);
  }
});

function handleReplayMessage(event) {
  if (event.target !== replayWorker) return;
  // 协调 Worker 回传 ready/progress/complete/error。页面只展示进度和最终报告，
  // 不在主线程重新运行 CSV 回放。
  const message = event.data;
  if (message.type === "detail-batch") {
    persistReplayDetails(message);
    return;
  }
  if (message.type === "detail-reset") {
    if (replayDetailFlushTimer) {
      clearTimeout(replayDetailFlushTimer);
      replayDetailFlushTimer = null;
    }
    flushReplayDetails(true);
    replayDetailSequence = 0;
    // 降级重跑前先等待已经排队的批量写入，再清理旧明细；否则两个 IndexedDB
    // 事务可能交错，导致旧批次在清理后又“复活”。
    const store = replayDetailStore;
    replayDetailWriteTail = replayDetailWriteTail.then(async () => {
      const started = performance.now();
      await store?.clearTemporary?.();
      replayStorageElapsedMs += performance.now() - started;
    });
    globalThis.__replayDetailWriteTail = replayDetailWriteTail;
    return;
  }
  if (message.type === "ready") {
    replayWorkerReady = true;
    if (replayStatus.textContent === "等待选择文件" && replaySourceMode === "simulation") {
      replayStatus.textContent = "可以开始随机回测";
    } else if (replayStatus.textContent === "等待选择文件" && currentCsvFile) {
      replayStatus.textContent = "可以开始回放";
    }
    updateReplayButton();
    return;
  }

  if (message.type === "complete") {
    setReplayProgress(0.98, "正在保存本地明细", "整理回测结果");
    replayStatus.textContent = "正在保存本地明细…";
    if (replayDetailFlushTimer) {
      clearTimeout(replayDetailFlushTimer);
      replayDetailFlushTimer = null;
    }
    flushReplayDetails(true);
    const renderCompleted = () => {
      const executionLabel = message.parallel
        ? `${message.workerCount ?? 1} 个并行 Worker`
        : "单线程";
      setReplayRunning(false, `回放完成 · ${executionLabel}`);
      releaseReplayWorker();
      setReplayProgress(1, "回测完成", `${message.report?.summary?.replayed_rounds ?? 0} 局`);
      renderReplay(message.report, message.elapsedMilliseconds, {
        ...(message.timings ?? message.report?.performance ?? {}),
        storageMs: Number(message.timings?.storageMs ?? 0) + replayStorageElapsedMs,
      });
      if (message.fallbackReason) replayStatus.textContent += ` · ${message.fallbackReason}`;
    };
    // 有分批明细时先等待 IndexedDB 写入完成；旧报告没有这个 Promise，保持
    // 原来的同步渲染行为，兼容外部嵌入页面和无明细的快速分析。
    const pending = globalThis.__replayDetailWriteTail;
    if (pending) void pending.then(renderCompleted).catch((error) => {
      setReplayRunning(false, "回放完成但明细保存失败");
      showError(replayError, `完整明细未能保存：${error?.message ?? error}`);
      resetReplayProgress("明细保存失败");
      setReplayProgress(0, "明细保存失败", "请重新运行回测");
      releaseReplayWorker();
    });
    else renderCompleted();
    return;
  }

  if (message.type === "progress") {
    if (message.phase === "generate") {
      replayStatus.textContent = "正在生成可复现牌靴…";
      setReplayProgress(
        message.overall ?? 0.02,
        "生成牌靴",
        `${message.completed ?? 0}/${message.total ?? 0} 靴`,
      );
    } else if (message.phase === "probability") {
      const workerLabel = message.workerCount ? ` · ${message.workerCount} 个 Worker` : "";
      replayStatus.textContent = message.parallel
        ? `并行枚举牌靴概率 ${message.completed ?? 0}/${message.total ?? 0}${workerLabel}…`
        : "正在单线程枚举牌靴概率…";
      setReplayProgress(
        message.overall ?? 0.2,
        "计算概率",
        `${message.completed ?? 0}/${message.total ?? 0} 靴${workerLabel}`,
      );
    } else if (message.phase === "worker-tune") {
      replayStatus.textContent = "正在实测并行效率…";
      setReplayProgress(
        message.overall ?? 0.03,
        "自动选择 Worker",
        "正在用首副牌靴测速",
      );
    } else if (message.phase === "serial") {
      replayStatus.textContent = message.fallbackReason
        ? `${message.fallbackReason}；正在单线程回放…`
        : "正在单线程回放，请勿关闭页面…";
      setReplayProgress(
        message.overall ?? 0.2,
        "单线程回放",
        "正在计算，请勿关闭页面",
        { indeterminate: true },
      );
    } else if (message.phase === "settlement") {
      replayStatus.textContent = "正在按时间顺序合并本金与倍投…";
      setReplayProgress(
        message.overall ?? 0.7,
        "顺序结算",
        `${message.completed ?? message.settledRounds ?? 0}/${message.total ?? message.settlementTotal ?? 0} 局`,
      );
    } else if (message.phase === "input") {
      replayStatus.textContent = "正在准备回测数据…";
      setReplayProgress(message.overall ?? 0.02, "准备数据");
    }
    return;
  }

  if (message.type === "error") {
    setReplayRunning(false, "回放失败");
    bankrollChartController.reset("回放失败；修正文件或配置后重新运行即可生成本金变化曲线。");
    replayTrendChartController.reset();
    contributionChartController.reset();
    replayAnalysisChartController.reset();
    showError(replayError, message.message);
    resetReplayProgress("回测失败");
    setReplayProgress(0, "回测失败", "可以修正配置后重试");
    releaseReplayWorker();
  }
}

/**
 * 将明细批次聚合后写入 IndexedDB，并在事务提交后逐批确认给 Worker。
 *
 * Worker 最多允许 4 个未确认批次，因此即使 IndexedDB 变慢，计算端也会暂停，
 * 不会让等待写入的 Promise/数组随百万靴持续增长。
 */
const REPLAY_DETAIL_WRITE_BATCH = 2048;
const REPLAY_DETAIL_MAX_PENDING_BATCHES = 4;
function flushReplayDetails(force = false) {
  if (!replayDetailStore || !replayDetailPendingBatches.length) return replayDetailWriteTail;
  const pendingRows = replayDetailPendingBatches.reduce((sum, batch) => sum + batch.rows.length, 0);
  if (!force && pendingRows < REPLAY_DETAIL_WRITE_BATCH
      && replayDetailPendingBatches.length < REPLAY_DETAIL_MAX_PENDING_BATCHES) {
    return replayDetailWriteTail;
  }
  const batches = replayDetailPendingBatches.splice(0);
  const rows = batches.flatMap(batch => batch.rows);
  const worker = replayWorker;
  const runId = activeReplayRunId;
  replayDetailWriteTail = replayDetailWriteTail.then(async () => {
    try {
      const started = performance.now();
      await replayDetailStore.put("details", rows);
      replayStorageElapsedMs += performance.now() - started;
      for (const batch of batches) {
        worker?.postMessage({ type: "detail-ack", runId, batchId: batch.batchId });
      }
    } catch (error) {
      worker?.postMessage({
        type: "detail-storage-error", runId,
        message: error?.message ?? String(error),
      });
      throw error;
    }
  });
  // 错误仍保留在 writeTail 供 complete 分支统一展示；这里附加只读 catch，
  // 避免浏览器在 Worker 尚未回传 error 时报告未处理的 Promise 拒绝。
  void replayDetailWriteTail.catch(() => {});
  globalThis.__replayDetailWriteTail = replayDetailWriteTail;
  return replayDetailWriteTail;
}

function scheduleReplayDetailFlush() {
  // 兼容旧的页面写入协议时也立即启动事务，不依赖后台页会节流的定时器。
  void flushReplayDetails(true);
}

/** 将一个 Worker 批次加入本地明细缓冲，并在回测结束前强制刷完。 */
function persistReplayDetails(message) {
  if (message.runId !== activeReplayRunId) return;
  if (!replayDetailStore) {
    replayWorker?.postMessage({
      type: "detail-storage-error", runId: message.runId,
      message: "本轮未创建明细存储，无法保存 Worker 返回的明细",
    });
    return;
  }
  const rows = (message.details ?? []).map((bet) => ({
    id: replayDetailSequence++,
    bet,
  }));
  if (!rows.length) {
    replayWorker?.postMessage({
      type: "detail-ack", runId: message.runId, batchId: message.batchId,
    });
    return;
  }
  replayDetailPendingBatches.push({ batchId: message.batchId, rows });
  flushReplayDetails(false);
  scheduleReplayDetailFlush();
}

function handleReplayError(event) {
  if (event.target !== replayWorker) return;
  event.preventDefault?.();
  replayWorker.terminate();
  replayWorkerReady = false;
  setReplayRunning(false, "回放计算中断，可重新尝试");
  bankrollChartController.reset("计算中断；可关闭并行或减少数据量后重试。");
  replayTrendChartController.reset();
  contributionChartController.reset();
  replayAnalysisChartController.reset();
  showError(replayError, event.message || "CSV 回放 Worker 无法启动");
  resetReplayProgress("回测失败");
  setReplayProgress(0, "回测失败", "可以修正配置后重试");
  releaseReplayWorker();
}

function releaseReplayWorker() {
  // 下一次点击才创建新实例；加载失败时也不会进入无限自动重试。
  replayWorker?.terminate();
  replayWorkerReady = true;
  replayWorker = null;
  updateReplayButton();
}

async function start() {
  // wasm-bindgen 初始化完成前，所有计算按钮都保持禁用；初始化成功后再做
  // 一次默认分析，让用户打开页面即可看到完整八副牌基线结果。
  try {
    await init(new URL("./pkg/game_ev_engine_bg.wasm?v=35", import.meta.url));
    wasmReady = true;
    wasmStatus.textContent = "WASM 已就绪";
    wasmStatus.classList.add("ready");
    analyzeButton.disabled = false;
    blackjackAnalyzeButton.disabled = false;
    calculate();
  } catch (error) {
    wasmStatus.textContent = "WASM 加载失败";
    showError(errorMessage, `无法加载计算核心：${error?.message ?? error}`);
  }
}

updateModeHelp();
updateBlackjackModeHelp();
updateStakeStrategyFields();
updateComparisonStrategyFields();
updateComparisonConfigSummaries();
updateSideBetRoundLimitHints();
updateConfigSummaries();
updateSimulationEstimate();
setReplaySourceMode("csv");
resetReplayWorker();
syncBaccaratView();
start();
