import init, { analyzeBaccaratStrategy } from "./pkg/game_ev_engine.js";

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
const csvFileInput = document.querySelector("#csv-file");
const selectedFile = document.querySelector("#selected-file");
const replayButton = document.querySelector("#replay-button");
const replayStatus = document.querySelector("#replay-status");
const replayError = document.querySelector("#replay-error");
const replayResults = document.querySelector("#replay-results");
const replayBody = document.querySelector("#replay-body");
const payoutRule = document.querySelector("#payout-rule");
const stakeStrategy = document.querySelector("#stake-strategy");
const fixedStakeField = document.querySelector("#fixed-stake-field");

const betLabels = {
  player: "闲",
  banker: "庄",
  tie: "和",
};

const sideBetLabels = {
  any_pair: "任意对子",
  banker_pair: "庄对",
  player_pair: "闲对",
  lucky_seven: "幸运 7",
  super_lucky_seven: "超级幸运 7",
};

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
  fixed: "固定金额",
};

const skipReasonLabels = {
  below_minimum_ev: "最优方向仍低于最低有效 EV，本局跳过。",
  non_positive_kelly: "有效 EV 没有形成正凯利比例，本局跳过。",
  risk_limit_is_zero: "资金比例或金额上限为 0，本局跳过。",
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

const replayWorker = new Worker(new URL("./replay-worker.js", import.meta.url), {
  type: "module",
});

function selectedMode() {
  return form.elements["source-mode"].value;
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
  return value;
}

function strategyConfig() {
  const selectedStakeStrategy = stakeStrategy.value;
  return {
    decks: Number.parseInt(deckCount.value, 10),
    rebateRate: readNumber("#rebate-rate", "返水比例", { min: 0, max: 100 }) / 100,
    minimumEffectiveEv: readNumber("#minimum-ev", "最低有效 EV") / 100,
    bankroll: readNumber("#bankroll", "本金", { positive: true }),
    maxFraction: readNumber("#max-fraction", "单局本金比例上限", {
      min: 0,
      max: 100,
    }) / 100,
    maxRoundStake: readNumber("#max-round-stake", "单局金额上限", { min: 0 }),
    tableLimit: readNumber("#table-limit", "桌台金额上限", { min: 0 }),
    payoutRule: payoutRule.value,
    stakeStrategy: selectedStakeStrategy,
    fixedStake: selectedStakeStrategy === "fixed"
      ? readNumber("#fixed-stake", "固定下注金额", { min: 0 })
      : 0,
  };
}

function updateStakeStrategyFields() {
  fixedStakeField.hidden = stakeStrategy.value !== "fixed";
}

function updateModeHelp() {
  const consumed = selectedMode() === "consumed";
  modeHelp.textContent = consumed
    ? "留空表示还没有消耗任何牌，将计算完整牌靴的基线结果。"
    : "这里必须输入当前牌靴剩余的全部牌；只输入部分剩余牌会得到另一個牌靴。";
  cardsInput.placeholder = consumed
    ? "例如：AS, 10H, KD, 7C"
    : "例如：AS, 2S, 3S, 4S, 5S, 6S…（全部剩余牌）";
}

function metricCell(value, emphasize = false) {
  const cell = document.createElement("td");
  cell.textContent = percent(value);
  if (emphasize) cell.className = evClass(value);
  return cell;
}

function renderResults(data, elapsedMilliseconds) {
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
      detailCell(metrics.payout),
      metricCell(metrics.ev, true),
      metricCell(metrics.house_edge),
      metricCell(metrics.rtp),
    );
    sideResultBody.append(row);
  }

  const decision = data.recommendation;
  recommendation.dataset.action = decision.action;
  setText("#recommended-bet", betLabels[decision.candidate_bet]);
  setText("#recommended-ev", percent(decision.effective_ev));
  applySignedClass(document.querySelector("#recommended-ev"), decision.effective_ev);
  setText("#recommended-action", decision.action === "place" ? "可下注" : "跳过");
  setText("#kelly-fraction", percent(decision.kelly_fraction, 3));
  setText("#strategy-fraction", percent(decision.strategy_fraction, 3));
  setText("#applied-fraction", percent(decision.applied_fraction, 3));
  setText("#suggested-amount", money(decision.suggested_amount));
  setText("#summary-amount", money(decision.suggested_amount));
  setText("#expected-profit", money(decision.expected_profit));
  applySignedClass(document.querySelector("#expected-profit"), decision.expected_profit);

  const reason = decision.action === "place"
    ? `采用${payoutRuleLabels[data.payout_rule]}与${stakeStrategyLabels[data.stake_strategy]}，已通过最低 EV 门槛；建议下${betLabels[decision.candidate_bet]}，金额已经过三项风险上限。`
    : skipReasonLabels[decision.reason] ?? "当前策略决定跳过本局。";
  setText("#decision-reason", reason);
}

function showError(target, error) {
  const message = typeof error === "string" ? error : error?.message ?? String(error);
  target.textContent = message;
  target.hidden = false;
}

function calculate() {
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
      config.fixedStake,
    );
    renderResults(JSON.parse(json), performance.now() - started);
  } catch (error) {
    showError(errorMessage, error);
  } finally {
    analyzeButton.disabled = false;
    analyzeButton.textContent = "计算策略";
  }
}

function updateReplayButton() {
  replayButton.disabled = !currentCsvFile || !replayWorkerReady || replayRunning;
}

function setReplayRunning(running, label) {
  replayRunning = running;
  replayStatus.textContent = label;
  replayStatus.classList.toggle("running", running);
  replayButton.textContent = running ? "正在回放…" : "开始策略回放";
  updateReplayButton();
}

function detailCell(value, className = "") {
  const cell = document.createElement("td");
  cell.textContent = value;
  if (className) cell.className = className;
  return cell;
}

function renderReplay(report, elapsedMilliseconds) {
  replayError.hidden = true;
  replayResults.hidden = false;
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

  setText("#dataset-rows", integerFormatter.format(dataset.total_rows));
  setText("#valid-sessions", integerFormatter.format(quality.fully_observable_sessions));
  setText("#quarantined-rounds", integerFormatter.format(quality.quarantined_rounds));
  setText("#valid-card-rows", integerFormatter.format(quality.valid_card_rows));
  setText("#hit-rate", percent(summary.hit_rate, 2));
  setText("#replay-time", `${(elapsedMilliseconds / 1000).toFixed(2)} 秒`);

  replayBody.replaceChildren();
  const visibleBets = report.bets.slice(0, 500);
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
      detailCell(betLabels[bet.bet]),
      detailCell(`${betLabels[bet.outcome]} · ${resultLabels[bet.result]}`),
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
    cell.textContent = summary.replayed_rounds === 0
      ? "没有可从第 1 局完整重建的牌靴，请查看隔离局数。"
      : "没有任何一局同时通过 EV 门槛和所选资金策略检查。";
    row.append(cell);
    replayBody.append(row);
  }

  const browserOmitted = report.bets.length - visibleBets.length;
  const omitted = report.omitted_bet_details + browserOmitted;
  setText(
    "#detail-note",
    omitted > 0
      ? `共 ${integerFormatter.format(summary.placed_bet_count)} 笔下注；页面展示前 ${visibleBets.length} 笔，省略 ${integerFormatter.format(omitted)} 笔明细，汇总仍包含全部数据。`
      : `共 ${integerFormatter.format(summary.placed_bet_count)} 笔下注，已显示全部明细。`,
  );
}

form.addEventListener("submit", (event) => {
  event.preventDefault();
  calculate();
});

form.elements["source-mode"].forEach((radio) => {
  radio.addEventListener("change", updateModeHelp);
});

stakeStrategy.addEventListener("change", () => {
  updateStakeStrategyFields();
  calculate();
});

payoutRule.addEventListener("change", calculate);

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

csvFileInput.addEventListener("change", () => {
  const [file] = csvFileInput.files;
  currentCsvFile = file ?? null;
  replayResults.hidden = true;
  replayError.hidden = true;

  if (!currentCsvFile) {
    selectedFile.textContent = "尚未选择文件";
    replayStatus.textContent = "等待选择文件";
  } else {
    selectedFile.textContent = `${currentCsvFile.name} · ${(currentCsvFile.size / 1024 / 1024).toFixed(2)} MB`;
    replayStatus.textContent = replayWorkerReady ? "可以开始回放" : "正在准备回放核心…";
  }
  updateReplayButton();
});

replayButton.addEventListener("click", async () => {
  if (!currentCsvFile || replayRunning) return;
  replayError.hidden = true;
  replayResults.hidden = true;

  try {
    if (currentCsvFile.size > 50 * 1024 * 1024) {
      throw new Error("CSV 超过 50 MB；请先按日期或桌台拆分后再回放。");
    }
    const config = strategyConfig();
    setReplayRunning(true, "正在读取 CSV…");
    const csvText = await currentCsvFile.text();
    setReplayRunning(true, "正在重建牌靴并计算策略…");
    replayWorker.postMessage({ type: "replay", csvText, config });
  } catch (error) {
    setReplayRunning(false, "回放失败");
    showError(replayError, error);
  }
});

replayWorker.addEventListener("message", (event) => {
  const message = event.data;
  if (message.type === "ready") {
    replayWorkerReady = true;
    if (currentCsvFile) replayStatus.textContent = "可以开始回放";
    updateReplayButton();
    return;
  }

  if (message.type === "complete") {
    setReplayRunning(false, "回放完成");
    renderReplay(message.report, message.elapsedMilliseconds);
    return;
  }

  if (message.type === "error") {
    setReplayRunning(false, "回放失败");
    showError(replayError, message.message);
  }
});

replayWorker.addEventListener("error", (event) => {
  setReplayRunning(false, "回放核心加载失败");
  showError(replayError, event.message || "CSV 回放 Worker 无法启动");
});

async function start() {
  try {
    await init();
    wasmReady = true;
    wasmStatus.textContent = "WASM 核心已就绪";
    wasmStatus.classList.add("ready");
    analyzeButton.disabled = false;
    calculate();
  } catch (error) {
    wasmStatus.textContent = "WASM 加载失败";
    showError(errorMessage, `无法加载计算核心：${error?.message ?? error}`);
  }
}

updateModeHelp();
updateStakeStrategyFields();
start();
