/*
 * CSV 回放专用 Web Worker。
 *
 * 主线程负责读取文件、读取表单和更新 UI；本线程负责等待 WASM 初始化，
 * 再执行“牌靴重建 -> 概率枚举 -> 策略回放”。Worker 与主线程之间只传递
 * 可结构化克隆的字符串/数字/普通对象，不直接访问 DOM，因此大 CSV 计算时
 * 页面仍可以滚动、取消或显示进度状态。
 */
import init, { replayBaccaratCsvWithSideBetLimits } from "./pkg/game_ev_engine.js";

// 这些默认值必须与 Rust::SideBetRoundLimits::default() 保持一致。
// Worker 需要一份副本，是为了兼容用户仍打开旧版本页面时缺少新字段的情况。
const defaultSideBetRoundLimits = {
  any_pair: 50,
  banker_pair: 50,
  player_pair: 50,
  perfect_pair: 45,
  big: 20,
  small: 20,
  lucky_seven: 50,
  super_lucky_seven: 50,
  lucky_six: 50,
  banker_dragon_bonus: 50,
  player_dragon_bonus: 50,
};

function finiteNumberOr(value, fallback) {
  const number = Number(value);
  return Number.isFinite(number) ? number : fallback;
}

/**
 * 规范化从主线程传来的独立边注局数限制。
 *
 * 边界层不能假设旧页面一定已经提供所有字段：缺失/非法值回退到当前
 * 默认值；只有旧版整体缺失时，才把 legacy luckyBetMaxRound 覆盖三种幸运玩法。
 */
function normalizedSideBetRoundLimits(config) {
  const source = config.sideBetRoundLimits ?? {};
  const result = {};
  for (const [key, defaultValue] of Object.entries(defaultSideBetRoundLimits)) {
    const value = Number(source[key]);
    result[key] = Number.isInteger(value) && value >= 0 ? value : defaultValue;
  }

  // 兼容只有旧“幸运 6/7 共用上限”字段的页面。
  if (!config.sideBetRoundLimits && Number.isInteger(Number(config.luckyBetMaxRound))) {
    const legacyLimit = Math.max(0, Number(config.luckyBetMaxRound));
    result.lucky_six = legacyLimit;
    result.lucky_seven = legacyLimit;
    result.super_lucky_seven = legacyLimit;
  }
  return result;
}

// 模块加载时先初始化 WASM。初始化成功后通知主线程可以启用“开始回放”按钮；
// 如果失败，后续回放请求也会通过统一 error 消息返回。
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
    // 同一 Worker 可能在 ready 消息发出前收到请求，所以这里再次 await 是
    // 必要的同步屏障，而不是重复初始化 WASM。
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
    const sideBetRoundLimits = normalizedSideBetRoundLimits(config);
    // 新字段缺失时关闭多注，保证旧页面仍然只选择一个最优目标。
    const allowMultipleBets = Boolean(config.allowMultipleBets);
    const started = performance.now();
    // Rust 入口只接收简单参数；边注限制对象在这里序列化成稳定 JSON，
    // 再由 Rust 反序列化为强类型 SideBetRoundLimits。
    const json = replayBaccaratCsvWithSideBetLimits(
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
      JSON.stringify(sideBetRoundLimits),
      allowMultipleBets,
    );
    // Rust 返回字符串 JSON，Worker 在边界处解析一次，主线程收到普通对象后
    // 可以直接渲染，不需要了解 wasm-bindgen 的返回类型。
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
