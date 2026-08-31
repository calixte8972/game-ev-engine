import init, { replayBaccaratCsv } from "./pkg/game_ev_engine.js";

function finiteNumberOr(value, fallback) {
  const number = Number(value);
  return Number.isFinite(number) ? number : fallback;
}

// 大型 CSV 的牌靴重建和概率枚举放在独立线程，避免主页面在计算时失去响应。
const ready = init();

ready
  .then(() => self.postMessage({ type: "ready" }))
  .catch((error) => {
    self.postMessage({
      type: "error",
      message: `无法加载 CSV 回放核心：${error?.message ?? String(error)}`,
    });
  });

self.addEventListener("message", async (event) => {
  if (event.data?.type !== "replay") return;

  try {
    await ready;
    const { csvText, config } = event.data;
    // 旧页面在新 Worker 上运行时可能没有这两个后来新增的边注字段。
    // JavaScript 的 undefined 传给 Rust f64 会变成 NaN。此处沿用旧版语义：
    // 边注门槛跟随主注门槛，边注限额跟随单局金额上限。
    const minimumSideBetEv = finiteNumberOr(
      config.minimumSideBetEv,
      config.minimumEffectiveEv,
    );
    const sideBetLimit = finiteNumberOr(config.sideBetLimit, config.maxRoundStake);
    const started = performance.now();
    const json = replayBaccaratCsv(
      csvText,
      config.decks,
      config.rebateRate,
      config.minimumEffectiveEv,
      config.bankroll,
      config.maxFraction,
      config.maxRoundStake,
      config.tableLimit,
      config.payoutRule,
      config.stakeStrategy,
      config.strategyParameter,
      minimumSideBetEv,
      sideBetLimit,
    );
    self.postMessage({
      type: "complete",
      report: JSON.parse(json),
      elapsedMilliseconds: performance.now() - started,
    });
  } catch (error) {
    self.postMessage({
      type: "error",
      message: error?.message ?? String(error),
    });
  }
});
