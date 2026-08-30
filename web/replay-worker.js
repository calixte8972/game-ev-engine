import init, { replayBaccaratCsv } from "./pkg/game_ev_engine.js";

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
      config.minimumSideBetEv,
      config.sideBetLimit,
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
