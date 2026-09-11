/*
 * 牌靴概率预计算子 Worker。
 *
 * 它只做一件事：接收一批完整牌靴，调用 Rust/WASM 逐局枚举下注前的概率。
 * 它不读取 DOM、不更新本金，也不决定倍投级数。这样主协调 Worker 可以把
 * 不同牌靴分发给多个实例，而所有有资金状态的操作仍由最终顺序回放完成。
 */
import init, { prepareBaccaratCsvWeights } from "./pkg/game_ev_engine.js";

const ready = init();

self.addEventListener("message", async (event) => {
  if (event.data?.type !== "prepare") return;

  try {
    await ready;
    const started = performance.now();
    const preparedJson = prepareBaccaratCsvWeights(
      event.data.csvText,
      event.data.decks,
    );

    self.postMessage({
      type: "complete",
      taskId: event.data.taskId,
      prepared: JSON.parse(preparedJson),
      elapsedMilliseconds: performance.now() - started,
    });
  } catch (error) {
    self.postMessage({
      type: "error",
      taskId: event.data?.taskId,
      message: error?.message ?? String(error),
    });
  }
});
