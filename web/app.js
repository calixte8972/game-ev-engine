import init, { analyzeBaccaratStrategy, analyzeBlackjack } from "./pkg/game_ev_engine.js";

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
const replayDetailWrap = document.querySelector(".replay-detail-wrap");
const replayPagination = document.querySelector("#replay-pagination");
const replayPageSize = document.querySelector("#replay-page-size");
const replayFirstPage = document.querySelector("#replay-first-page");
const replayPreviousPage = document.querySelector("#replay-previous-page");
const replayNextPage = document.querySelector("#replay-next-page");
const replayLastPage = document.querySelector("#replay-last-page");
const replayPageStatus = document.querySelector("#replay-page-status");
const payoutRule = document.querySelector("#payout-rule");
const stakeStrategy = document.querySelector("#stake-strategy");
const strategyParameterField = document.querySelector("#strategy-parameter-field");
const strategyParameterLabel = document.querySelector("#strategy-parameter-label");
const strategyParameterWrapper = document.querySelector("#strategy-parameter-wrapper");
const strategyParameterInput = document.querySelector("#strategy-parameter");
const strategyParameterPrefix = document.querySelector("#strategy-parameter-prefix");
const strategyParameterSuffix = document.querySelector("#strategy-parameter-suffix");
const gameTabs = document.querySelectorAll(".game-tab");
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
};

const allBetLabels = { ...betLabels, ...sideBetLabels };

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
};

const stakeStrategyParameters = {
  custom_kelly: { label: "完整凯利缩放系数", unit: "percent", defaultValue: 30, step: 1 },
  fixed: { label: "固定下注金额", unit: "money", defaultValue: 100, step: 10 },
  bankroll_fraction: { label: "每局本金比例", unit: "percent", defaultValue: 1, step: 0.1 },
  target_expected_profit: { label: "单笔目标期望盈利", unit: "money", defaultValue: 10, step: 1 },
  target_volatility: { label: "单笔目标波动率", unit: "percent", defaultValue: 1, step: 0.1 },
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
let currentReplayReport = null;
let currentReplayPage = 1;

const replayWorker = new Worker(new URL("./replay-worker.js", import.meta.url), {
  type: "module",
});

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
  const parameterDefinition = stakeStrategyParameters[selectedStakeStrategy];
  let strategyParameter = 0;
  if (parameterDefinition) {
    strategyParameter = readNumber("#strategy-parameter", parameterDefinition.label, {
      min: 0,
      max: parameterDefinition.unit === "percent" ? 100 : undefined,
    });
    if (parameterDefinition.unit === "percent") strategyParameter /= 100;
  }
  return {
    decks: Number.parseInt(deckCount.value, 10),
    rebateRate: readNumber("#rebate-rate", "返水比例", { min: 0, max: 100 }) / 100,
    minimumEffectiveEv: readNumber("#minimum-ev", "最低有效 EV") / 100,
    minimumSideBetEv: readNumber("#minimum-side-bet-ev", "边注最低 EV") / 100,
    bankroll: readNumber("#bankroll", "本金", { positive: true }),
    maxFraction: readNumber("#max-fraction", "单局本金比例上限", {
      min: 0,
      max: 100,
    }) / 100,
    maxRoundStake: readNumber("#max-round-stake", "单局金额上限", { min: 0 }),
    sideBetLimit: readNumber("#side-bet-limit", "边注单笔金额上限", { min: 0 }),
    tableLimit: readNumber("#table-limit", "桌台金额上限", { min: 0 }),
    payoutRule: payoutRule.value,
    stakeStrategy: selectedStakeStrategy,
    strategyParameter,
  };
}

function updateStakeStrategyFields() {
  const definition = stakeStrategyParameters[stakeStrategy.value];
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

function setActiveGame(game) {
  for (const tab of gameTabs) {
    const active = tab.dataset.game === game;
    tab.classList.toggle("active", active);
    tab.setAttribute("aria-selected", String(active));
  }
  for (const section of document.querySelectorAll(".baccarat-section")) {
    section.hidden = game !== "baccarat";
  }
  for (const section of document.querySelectorAll(".blackjack-section")) {
    section.hidden = game !== "blackjack";
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
  setText("#recommended-bet", allBetLabels[decision.candidate_bet]);
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

  const targetLimitText = decision.bet_category === "side"
    ? "，并应用边注单独金额上限"
    : "";
  const reason = decision.action === "place"
    ? `采用${payoutRuleLabels[data.payout_rule]}与${stakeStrategyLabels[data.stake_strategy]}，已通过对应 EV 门槛；建议下${allBetLabels[decision.candidate_bet]}，金额已经过共同风险上限${targetLimitText}。`
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
      config.strategyParameter,
      config.minimumSideBetEv,
      config.sideBetLimit,
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

function renderReplayDetails() {
  if (!currentReplayReport) return;

  const { bets, omitted_bet_details: omittedBetDetails, summary } = currentReplayReport;
  const pageSize = Number.parseInt(replayPageSize.value, 10);
  const totalPages = Math.max(1, Math.ceil(bets.length / pageSize));
  currentReplayPage = Math.min(Math.max(currentReplayPage, 1), totalPages);
  const startIndex = (currentReplayPage - 1) * pageSize;
  const endIndex = Math.min(startIndex + pageSize, bets.length);
  const visibleBets = bets.slice(startIndex, endIndex);

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

  replayPagination.hidden = bets.length === 0;
  replayFirstPage.disabled = currentReplayPage === 1;
  replayPreviousPage.disabled = currentReplayPage === 1;
  replayNextPage.disabled = currentReplayPage === totalPages;
  replayLastPage.disabled = currentReplayPage === totalPages;
  replayPageStatus.textContent = `第 ${integerFormatter.format(currentReplayPage)} / ${integerFormatter.format(totalPages)} 页`;

  if (omittedBetDetails > 0) {
    setText(
      "#detail-note",
      `当前报告来自旧版回放核心，仍有 ${integerFormatter.format(omittedBetDetails)} 笔明细未包含；请重新运行回放。`,
    );
  } else if (bets.length === 0) {
    setText("#detail-note", "本次策略没有产生可下注明细。");
  } else if (totalPages === 1) {
    setText("#detail-note", `共 ${integerFormatter.format(bets.length)} 笔下注，已显示全部明细。`);
  } else {
    setText(
      "#detail-note",
      `共 ${integerFormatter.format(bets.length)} 笔下注；当前显示第 ${integerFormatter.format(startIndex + 1)}–${integerFormatter.format(endIndex)} 笔，可翻页查看全部明细。`,
    );
  }

  replayDetailWrap.scrollTop = 0;
}

function renderReplay(report, elapsedMilliseconds) {
  replayError.hidden = true;
  replayResults.hidden = false;
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

  setText("#dataset-rows", integerFormatter.format(dataset.total_rows));
  setText("#valid-sessions", integerFormatter.format(quality.fully_observable_sessions));
  setText("#quarantined-rounds", integerFormatter.format(quality.quarantined_rounds));
  setText("#valid-card-rows", integerFormatter.format(quality.valid_card_rows));
  setText("#hit-rate", percent(summary.hit_rate, 2));
  setText("#replay-time", `${(elapsedMilliseconds / 1000).toFixed(2)} 秒`);

  renderReplayDetails();
}

form.addEventListener("submit", (event) => {
  event.preventDefault();
  calculate();
});

blackjackForm.addEventListener("submit", (event) => {
  event.preventDefault();
  calculateBlackjack();
});

blackjackAnalyzeButton.addEventListener("click", calculateBlackjack);

for (const tab of gameTabs) {
  tab.addEventListener("click", () => setActiveGame(tab.dataset.game));
}

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
  currentReplayReport = null;
  replayPagination.hidden = true;

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
  currentReplayPage = Math.max(1, Math.ceil(currentReplayReport.bets.length / pageSize));
  renderReplayDetails();
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
start();
