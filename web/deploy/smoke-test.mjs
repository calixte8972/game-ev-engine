import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  analyzeBaccaratStrategy,
  initSync,
  replayBaccaratCsv,
  replayBaccaratCsvWithSideBetLimits,
  simulateBaccaratShoesWithSideBetLimits,
} from "../pkg/game_ev_engine.js";
import { buildBankrollSeries, sampleBankrollSeries } from "../bankroll-chart.js";
import { buildContributionSeries } from "../bet-contribution-charts.js";
import {
  buildDrawdownSeries,
  buildRiskReturnSeries,
  buildRoiSeries,
  buildWaterfallSeries,
} from "../replay-analysis-charts.js";

const deployDirectory = dirname(fileURLToPath(import.meta.url));
const webDirectory = resolve(deployDirectory, "..");
const wasmBytes = readFileSync(resolve(webDirectory, "pkg/game_ev_engine_bg.wasm"));
const pageHtml = readFileSync(resolve(webDirectory, "index.html"), "utf8");
const replayWorkerSource = readFileSync(resolve(webDirectory, "replay-worker.js"), "utf8");
initSync({ module: wasmBytes });

// min=0.01 与 step=100 会让 10000 产生 stepMismatch，浏览器会直接阻止
// 表单提交。金额使用 0.01 步长，既允许整数本金，也允许带分的金额。
if (!/<input id="bankroll"[^>]*step="0\.01"/.test(pageHtml)) {
  throw new Error("初始本金输入框必须允许 0.01 精度，避免整数本金被判为非法");
}

if (!/id="allow-multiple-bets"/.test(pageHtml)) {
  throw new Error("页面没有同局多下注开关");
}

for (const id of ["simulation-shoes", "simulation-rounds", "simulation-seed"]) {
  if (!new RegExp(`id="${id}"`).test(pageHtml)) {
    throw new Error(`随机牌靴回测缺少输入：${id}`);
  }
}

for (const id of [
  "limit-any-pair", "limit-banker-pair", "limit-player-pair", "limit-perfect-pair",
  "limit-big", "limit-small", "limit-lucky-six", "limit-lucky-seven",
  "limit-super-lucky-seven", "limit-banker-dragon-bonus", "limit-player-dragon-bonus",
]) {
  if (!new RegExp(`id="${id}"`).test(pageHtml)) {
    throw new Error(`页面缺少独立边注局数配置：${id}`);
  }
}

if (!/<input id="limit-super-lucky-seven"[^>]*value="30"/.test(pageHtml)
    || !/id="status-limit-super-lucky-seven">第 31 局起禁用/.test(pageHtml)
    || !/super_lucky_seven:\s*30/.test(replayWorkerSource)) {
  throw new Error("超级幸运 7 默认必须允许到第 30 局，并从第 31 局起禁用");
}

if (!/id="bankroll-chart"/.test(pageHtml)
    || !/id="bankroll-chart-tooltip"/.test(pageHtml)) {
  throw new Error("回放结果缺少本金变化折线图或逐点提示");
}

if (!/id="bet-contribution-charts"/.test(pageHtml)
    || !/data-kind="profit"/.test(pageHtml)
    || !/data-kind="loss"/.test(pageHtml)) {
  throw new Error("资金图表区缺少盈利或亏损下注类别贡献圆环");
}

if (!/id="replay-analysis-charts"/.test(pageHtml)
    || !/data-chart="roi"/.test(pageHtml)
    || !/data-chart="waterfall"/.test(pageHtml)
    || !/data-chart="drawdown"/.test(pageHtml)
    || !/data-chart="risk-return"/.test(pageHtml)) {
  throw new Error("资金分析区缺少 ROI、瀑布、回撤或风险收益图");
}

if (!/id="bet-count-grid"/.test(pageHtml)) {
  throw new Error("回放结果缺少各下注类型的下注笔数统计");
}

// 资金曲线不能放在默认 hidden 的回放结果容器中，否则用户第一次打开
// 页面时完全看不到这个功能，也不知道上传 CSV 后会生成图表。
if (pageHtml.indexOf('id="bankroll-chart-title"') > pageHtml.indexOf('id="replay-results"')) {
  throw new Error("本金变化曲线仍被隐藏在回放结果容器中");
}
if (!/bankrollChartController\.reset/.test(readFileSync(resolve(webDirectory, "app.js"), "utf8"))) {
  throw new Error("切换文件或开始新回放时没有重置旧的本金曲线");
}

for (const id of [
  "maximum-profit",
  "maximum-bankroll",
  "minimum-bankroll",
  "maximum-single-stake",
  "maximum-round-stake",
]) {
  if (!new RegExp(`id="${id}"`).test(pageHtml)) {
    throw new Error(`回放结果缺少风险指标：${id}`);
  }
}

if (!/id="replay-pagination"/.test(pageHtml)
    || !/id="replay-last-page"/.test(pageHtml)) {
  throw new Error("CSV 全量下注明细必须提供分页和末页导航");
}

const appSource = readFileSync(resolve(webDirectory, "app.js"), "utf8");
if (!/data-replay-source="simulation"/.test(pageHtml)
    || !/type:\s*"simulate"/.test(appSource)
    || !/simulateBaccaratShoesWithSideBetLimits/.test(replayWorkerSource)) {
  throw new Error("随机生成入口没有完整连接到 WASM 回测 Worker");
}
if (!/200 \* 1024 \* 1024/.test(appSource)
    || !/最大 200 MB/.test(pageHtml)) {
  throw new Error("CSV 上传上限没有统一提高到 200 MB");
}
if (!/currentCsvFile\.arrayBuffer\(\)/.test(appSource)
    || !/\[csvBuffer\]/.test(appSource)
    || !/TextDecoder/.test(replayWorkerSource)) {
  throw new Error("大型 CSV 没有通过 transferable ArrayBuffer 交给 Worker");
}
if (!/session_id.*round_no.*raw_cards/s.test(pageHtml)) {
  throw new Error("页面没有说明精简 CSV 的三列格式");
}
if (/report\.bets\.slice\(0,\s*500\)/.test(appSource)) {
  throw new Error("页面仍然只读取前 500 笔下注明细");
}
if (!/suitSymbols/.test(appSource) || !/appendCardLine/.test(appSource)) {
  throw new Error("回放明细没有把 ASCII 牌面转换为带花色符号的真实牌面");
}
if (!/createBankrollChart/.test(appSource)) {
  throw new Error("页面没有把回放报告交给本金变化折线图");
}
if (!/contributionChartController\.render\(report\)/.test(appSource)) {
  throw new Error("页面没有把 Rust 回放报告交给下注贡献圆环");
}
if (!/replayAnalysisChartController\.render\(report\)/.test(appSource)) {
  throw new Error("页面没有把 Rust 回放报告交给四张收益风险诊断图");
}
if (!/renderBetBreakdown\(summary\.bet_breakdown, summary\.placed_bets\)/.test(appSource)) {
  throw new Error("页面没有展示 Rust 回放报告中的分类下注金额与盈亏");
}

const manual = JSON.parse(
  analyzeBaccaratStrategy(
    "consumed", 8, "", 0.009, 0, 10_000, 0.05, 500, 10_000,
    "standard", "full_kelly", 0, 0, 100,
    false,
  ),
);

if (manual.remaining_card_count !== 416) {
  throw new Error("完整八副牌的剩余张数不正确");
}

const bankrollFraction = JSON.parse(
  analyzeBaccaratStrategy(
    "consumed", 8, "", 0.02, 0, 10_000, 1, 1_000, 1_000,
    "standard", "bankroll_fraction", 0.02, 0, 100,
    false,
  ),
);

if (bankrollFraction.stake_strategy !== "bankroll_fraction"
    || bankrollFraction.strategy_parameter !== 0.02
    || bankrollFraction.recommendation.action !== "place"
    || bankrollFraction.recommendation.suggested_amount !== 200) {
  throw new Error("固定本金比例策略没有按当前本金计算下注金额");
}

const sideRecommendation = JSON.parse(
  analyzeBaccaratStrategy(
    "remaining", 8, "AS AC AD AH AS AC", 0, 0, 1_000, 1, 500, 1_000,
    "standard", "full_kelly", 0, 0, 25,
    false,
  ),
);

const multipleRecommendation = JSON.parse(
  analyzeBaccaratStrategy(
    "consumed", 8, "", 0, -1, 1_000, 1, 500, 1_000,
    "standard", "fixed", 100, -1, 25, true,
  ),
);

if (!multipleRecommendation.allow_multiple_bets
    || multipleRecommendation.recommendations.length < 2
    || multipleRecommendation.total_suggested_amount > 500 + 1e-9) {
  throw new Error("同局多下注没有返回多个合格目标，或没有共享本局总风险上限");
}

if (sideRecommendation.recommendation.candidate_bet !== "banker_pair"
    || sideRecommendation.recommendation.suggested_amount !== 25) {
  throw new Error("边注没有参与策略，或没有应用独立金额上限");
}

if (manual.side_bets.banker_pair.payout !== "11:1") {
  throw new Error("庄对赔付表没有进入 WASM 输出");
}

if (manual.side_bets.perfect_pair.payout !== "25:1"
    || manual.side_bets.perfect_pair.probability <= 0) {
  throw new Error("完美对子概率或赔付表没有进入 WASM 输出");
}

if (manual.side_bets.big.payout !== "0.5:1"
    || manual.side_bets.small.payout !== "1.5:1"
    || Math.abs(manual.side_bets.big.probability + manual.side_bets.small.probability - 1) > 1e-12) {
  throw new Error("大/小概率没有覆盖全部牌局，或赔付表没有进入 WASM 输出");
}

for (const [name, metrics] of Object.entries(manual.side_bets)) {
  if (Math.abs(metrics.rebate_ev - 0.009) > 1e-12
      || Math.abs(metrics.effective_ev - (metrics.ev + 0.009)) > 1e-12
      || Math.abs(metrics.effective_rtp - (metrics.rtp + 0.009)) > 1e-12) {
    throw new Error(`${name} 没有把返水加入边注有效 EV 与有效 RTP`);
  }
}

if (Math.abs(manual.side_bets.lucky_seven.rtp - 0.8170) > 0.00005) {
  throw new Error("幸运 7 的完整牌靴 RTP 偏离规则基线");
}

if (manual.side_bets.lucky_six.probability <= 0
    || manual.side_bets.banker_dragon_bonus.probability <= 0
    || manual.side_bets.player_dragon_bonus.probability <= 0) {
  throw new Error("幸运 6 或龙宝没有进入 WASM 概率与 EV 输出");
}

const tinyCsv = `__source_pk,table_id,session_id,round_no,started_at,settled_at,raw_cards,result_code
a,1,9001,1,2026-08-20 00:00:12,2026-08-20 00:00:44,"b:24,31,45;p:31,42,47",36
b,1,9001,2,2026-08-20 00:00:54,2026-08-20 00:01:17,"b:73,62,;p:53,8,",322
`;
const tinyReplay = JSON.parse(
  replayBaccaratCsv(
    tinyCsv, 8, 0.02, 0, 10_000, 0.05, 1_000, 1_000,
    "no_commission", "half_kelly", 0, 0, 100,
    1,
    false,
  ),
);

const compactCsv = `session_id,round_no,raw_cards
9001,1,"b:24,31,45;p:31,42,47"
9001,2,"b:73,62,;p:53,8,"
`;
const compactReplay = JSON.parse(
  replayBaccaratCsv(
    compactCsv, 8, 0.02, 0, 10_000, 0.05, 1_000, 1_000,
    "standard", "fixed", 100, 0, 100,
    1,
    false,
  ),
);

// 模拟旧页面没有 minimumSideBetEv / sideBetLimit 两个字段时，wasm-bindgen
// 会收到的 NaN。新核心必须回退到主注门槛和单局限额，而不是拒绝整个回放。
const legacyReplay = JSON.parse(
  replayBaccaratCsv(
    tinyCsv, 8, 0.02, 0, 10_000, 0.05, 1_000, 1_000,
    "standard", "fixed", 100, Number.NaN, Number.NaN,
    0,
    false,
  ),
);

const customRoundLimits = {
  any_pair: 12,
  banker_pair: 13,
  player_pair: 14,
  perfect_pair: 45,
  big: 20,
  small: 20,
  lucky_seven: 31,
  super_lucky_seven: 32,
  lucky_six: 33,
  banker_dragon_bonus: 40,
  player_dragon_bonus: 41,
};
const customLimitReplay = JSON.parse(
  replayBaccaratCsvWithSideBetLimits(
    tinyCsv, 8, 0.02, 0, 10_000, 0.05, 1_000, 1_000,
    "standard", "fixed", 100, 0, 100,
    JSON.stringify(customRoundLimits),
    false,
  ),
);
const simulatedReplay = JSON.parse(
  simulateBaccaratShoesWithSideBetLimits(
    2, 3, "42",
    8, 0.009, 0, 10_000, 0.05, 500, 500,
    "standard", "fixed", 100, 0, 100,
    JSON.stringify(customRoundLimits),
    false,
  ),
);

if (tinyReplay.summary.replayed_rounds !== 2) {
  throw new Error("WASM CSV 回放没有完成两局测试数据");
}
if (compactReplay.summary.replayed_rounds !== 2
    || compactReplay.dataset.table_count !== 1
    || compactReplay.dataset.business_date_count !== 0
    || compactReplay.quality.valid_card_rows !== 2) {
  throw new Error("WASM 没有正确回放仅含牌靴、子局数和牌面的精简 CSV");
}
if (simulatedReplay.dataset.session_count !== 2
    || simulatedReplay.dataset.total_rows !== 6
    || simulatedReplay.quality.fully_observable_sessions !== 2
    || simulatedReplay.summary.replayed_rounds !== 6) {
  throw new Error("WASM 随机牌靴入口没有生成并回测指定的牌靴数与子局数");
}

const placedBetCountFromCategories = Object.values(tinyReplay.summary.placed_bets)
  .reduce((total, count) => total + count, 0);
if (placedBetCountFromCategories !== tinyReplay.summary.placed_bet_count) {
  throw new Error("各下注类型笔数之和与实际下注总数不一致");
}

const betBreakdown = Object.values(tinyReplay.summary.bet_breakdown);
const breakdownCount = betBreakdown.reduce((total, item) => total + item.count, 0);
const breakdownStake = betBreakdown.reduce((total, item) => total + item.total_stake, 0);
const breakdownProfit = betBreakdown.reduce((total, item) => total + item.total_profit, 0);
const grossProfit = betBreakdown.reduce((total, item) => total + item.gross_profit, 0);
const grossLoss = betBreakdown.reduce((total, item) => total + item.gross_loss, 0);
const baseGrossProfit = betBreakdown.reduce((total, item) => total + item.base_gross_profit, 0);
const baseGrossLoss = betBreakdown.reduce((total, item) => total + item.base_gross_loss, 0);
const classifiedResults = betBreakdown.reduce(
  (total, item) => total + item.win_count + item.loss_count + item.push_count,
  0,
);
if (breakdownCount !== tinyReplay.summary.placed_bet_count
    || classifiedResults !== breakdownCount
    || Math.abs(breakdownStake - tinyReplay.summary.total_stake) > 1e-9
    || Math.abs(breakdownProfit - tinyReplay.summary.total_profit) > 1e-9
    || Math.abs(grossProfit - grossLoss - tinyReplay.summary.total_profit) > 1e-9
    || Math.abs(baseGrossProfit - baseGrossLoss - tinyReplay.summary.base_game_profit) > 1e-9
    || Math.abs(
      baseGrossProfit + tinyReplay.summary.rebate_income
        - baseGrossLoss - tinyReplay.summary.total_profit,
    ) > 1e-9) {
  throw new Error("各下注类型的笔数、下注额或盈亏没有与回放总指标对账");
}

const replayLabels = { player: "闲", banker: "庄", tie: "和" };
const roiSeries = buildRoiSeries(tinyReplay.summary.bet_breakdown, replayLabels);
const waterfallSeries = buildWaterfallSeries(
  tinyReplay.summary,
  tinyReplay.summary.bet_breakdown,
);
const riskReturnSeries = buildRiskReturnSeries(tinyReplay.summary.bet_breakdown, replayLabels);
if (roiSeries.length === 0
    || riskReturnSeries.length !== roiSeries.length
    || riskReturnSeries.some((item) => item.lossRate < 0 || item.lossRate > 1)
    || Math.abs(waterfallSeries.reconciliationDifference) > 1e-9
    || Math.abs(waterfallSeries.net - tinyReplay.summary.total_profit) > 1e-9) {
  throw new Error("ROI、瀑布或风险收益图的数据模型没有与 Rust 回放汇总对账");
}

// 同一个玩法的正收益和负收益必须进入两张不同的图，不能先算成净亏 100。
const contributionFixture = {
  banker: { gross_profit: 500, gross_loss: 600 },
  player: { gross_profit: 300, gross_loss: 100 },
  tie: { gross_profit: 200, gross_loss: 0 },
};
const profitContribution = buildContributionSeries(
  contributionFixture,
  "gross_profit",
  { banker: "庄", player: "闲", tie: "和" },
);
const lossContribution = buildContributionSeries(
  contributionFixture,
  "gross_loss",
  { banker: "庄", player: "闲", tie: "和" },
);
if (profitContribution.total !== 1_000
    || lossContribution.total !== 700
    || profitContribution.items[0].label !== "庄"
    || profitContribution.items[0].share !== 0.5
    || lossContribution.items[0].value !== 600) {
  throw new Error("盈利与亏损贡献没有分开统计，或圆环百分比计算错误");
}

const longTailContribution = buildContributionSeries({
  player: { gross_profit: 600 },
  banker: { gross_profit: 500 },
  tie: { gross_profit: 400 },
  big: { gross_profit: 300 },
  small: { gross_profit: 200 },
  any_pair: { gross_profit: 90 },
  perfect_pair: { gross_profit: 10 },
}, "gross_profit", {
  player: "闲",
  banker: "庄",
  tie: "和",
  big: "大",
  small: "小",
  any_pair: "任意对子",
  perfect_pair: "完美对子",
});
const longTailShares = longTailContribution.items.reduce((sum, item) => sum + item.share, 0);
if (longTailContribution.items.length !== 6
    || longTailContribution.items.at(-1)?.key !== "other"
    || longTailContribution.items.at(-1)?.value !== 100
    || Math.abs(longTailShares - 1) > 1e-12) {
  throw new Error("下注类别超过五种时，没有正确合并为“其他”或占比之和不为 100%");
}

for (const field of [
  "maximum_bankroll",
  "maximum_profit",
  "minimum_bankroll",
  "maximum_single_stake",
  "maximum_round_stake",
]) {
  if (!Number.isFinite(tinyReplay.summary[field])) {
    throw new Error(`WASM 回放没有返回有限的风险指标：${field}`);
  }
}

if (tinyReplay.summary.maximum_bankroll < tinyReplay.summary.minimum_bankroll
    || tinyReplay.summary.maximum_round_stake < tinyReplay.summary.maximum_single_stake
    || Math.abs(
      tinyReplay.summary.maximum_profit
        - (tinyReplay.summary.maximum_bankroll - tinyReplay.summary.initial_bankroll),
    ) > 1e-9) {
  throw new Error("WASM 回放的最高/最低本金、最大盈利或下注暴露指标关系不一致");
}

if (legacyReplay.config.minimum_side_bet_ev !== 0
    || legacyReplay.config.side_bet_limit !== 1_000
    || legacyReplay.config.lucky_bet_max_round !== null) {
  throw new Error("旧页面缺少边注配置字段时没有正确回退");
}

if (tinyReplay.config.lucky_bet_max_round !== 1) {
  throw new Error("幸运 6/7 最晚下注局数没有进入 WASM 回放配置");
}

for (const [bet, maxRound] of Object.entries(customRoundLimits)) {
  if (customLimitReplay.config.side_bet_round_limits[bet] !== maxRound) {
    throw new Error(`${bet} 的自定义最晚下注局数没有进入 WASM 回放配置`);
  }
}

if (!tinyReplay.bets[0]?.player_cards
    || !tinyReplay.bets[0]?.banker_cards
    || !Number.isInteger(tinyReplay.bets[0]?.player_total)
    || !Number.isInteger(tinyReplay.bets[0]?.banker_total)) {
  throw new Error("CSV 回放明细没有返回庄闲具体牌面与最终点数");
}

const bankrollSeries = buildBankrollSeries(tinyReplay);
if (bankrollSeries.length !== 3
    || bankrollSeries[0].bankroll !== tinyReplay.summary.initial_bankroll
    || bankrollSeries.at(-1).bankroll !== tinyReplay.summary.final_bankroll) {
  throw new Error("本金曲线没有从初始本金连接到最终本金");
}

const syntheticMultipleReplay = {
  summary: { initial_bankroll: 1_000 },
  bets: [
    {
      table_id: 1, session_id: 10, round_no: 1, started_at: "2026-08-20 00:00:00",
      amount: 100, actual_profit: 100, bankroll_after: 1_050,
    },
    {
      table_id: 1, session_id: 10, round_no: 1, started_at: "2026-08-20 00:00:00",
      amount: 50, actual_profit: -50, bankroll_after: 1_050,
    },
    {
      table_id: 1, session_id: 10, round_no: 2, started_at: "2026-08-20 00:01:00",
      amount: 40, actual_profit: -40, bankroll_after: 1_010,
    },
  ],
};
const syntheticSeries = buildBankrollSeries(syntheticMultipleReplay);
const syntheticDrawdown = buildDrawdownSeries(syntheticMultipleReplay);
if (syntheticSeries.length !== 3
    || syntheticSeries[1].betCount !== 2
    || syntheticSeries[1].roundStake !== 150
    || syntheticSeries[1].roundProfit !== 50
    || syntheticSeries[2].drawdown !== 40
    || syntheticSeries[2].drawdownDuration !== 1
    || syntheticDrawdown.maximumDrawdown !== 40
    || syntheticDrawdown.maximumDuration !== 1) {
  throw new Error("本金曲线没有正确合并同局多注或计算逐点回撤");
}

const denseSeries = Array.from({ length: 500 }, (_, index) => ({
  index,
  bankroll: index === 247 ? 2_000 : 1_000 + index,
}));
const sampledSeries = sampleBankrollSeries(denseSeries, 40);
if (sampledSeries[0].index !== 0
    || sampledSeries.at(-1).index !== 499
    || !sampledSeries.some((point) => point.index === 247)) {
  throw new Error("本金曲线绘制抽样丢失了起点、终点或关键峰值");
}

const output = {
  manual: {
    remaining_cards: manual.remaining_card_count,
    candidate: manual.recommendation.candidate_bet,
    action: manual.recommendation.action,
    amount: manual.recommendation.suggested_amount,
    side_bets: {
      banker_pair_probability: manual.side_bets.banker_pair.probability,
      perfect_pair_probability: manual.side_bets.perfect_pair.probability,
      big_probability: manual.side_bets.big.probability,
      small_probability: manual.side_bets.small.probability,
      lucky_seven_rtp: manual.side_bets.lucky_seven.rtp,
      super_lucky_seven_rtp: manual.side_bets.super_lucky_seven.rtp,
      lucky_six_rtp: manual.side_bets.lucky_six.rtp,
      banker_dragon_bonus_rtp: manual.side_bets.banker_dragon_bonus.rtp,
      player_dragon_bonus_rtp: manual.side_bets.player_dragon_bonus.rtp,
    },
  },
  bankroll_fraction: {
    parameter: bankrollFraction.strategy_parameter,
    amount: bankrollFraction.recommendation.suggested_amount,
  },
  tiny_replay: {
    rounds: tinyReplay.summary.replayed_rounds,
    bets: tinyReplay.summary.placed_bet_count,
    final_bankroll: tinyReplay.summary.final_bankroll,
    maximum_profit: tinyReplay.summary.maximum_profit,
    maximum_bankroll: tinyReplay.summary.maximum_bankroll,
    minimum_bankroll: tinyReplay.summary.minimum_bankroll,
    maximum_single_stake: tinyReplay.summary.maximum_single_stake,
    maximum_round_stake: tinyReplay.summary.maximum_round_stake,
  },
};

const csvPath = process.argv[2];
if (csvPath) {
  const csvText = readFileSync(resolve(csvPath), "utf8");
  const started = performance.now();
  const replay = JSON.parse(
    replayBaccaratCsv(
      csvText, 8, 0.009, 0, 10_000, 0.05, 500, 10_000,
      "no_commission", "half_kelly", 0, 0, 100,
      0,
      false,
    ),
  );
  output.full_replay = {
    elapsed_seconds: (performance.now() - started) / 1_000,
    rows: replay.dataset.total_rows,
    complete_sessions: replay.quality.fully_observable_sessions,
    replayed_rounds: replay.summary.replayed_rounds,
    placed_bets: replay.summary.placed_bet_count,
    placed_bets_by_target: replay.summary.placed_bets,
    total_profit: replay.summary.total_profit,
    final_bankroll: replay.summary.final_bankroll,
    omitted_bet_details: replay.omitted_bet_details,
  };
}

console.log(JSON.stringify(output, null, 2));
