/*
 * 牌靴概率预计算子 Worker。
 *
 * 它只做一件事：接收一批完整牌靴，调用 Rust/WASM 逐局枚举下注前的概率。
 * 它不读取 DOM、不更新本金，也不决定倍投级数。这样主协调 Worker 可以把
 * 不同牌靴分发给多个实例，而所有有资金状态的操作仍由最终顺序回放完成。
 */
import init, {
  prepareBaccaratCsvWeights,
  prepareGeneratedShoe,
  prepareReplayShoe,
  prepareReplayShoeWithSideBetLimits,
} from "./pkg/game_ev_engine.js";

const ready = init(new URL("./pkg/game_ev_engine_bg.wasm?v=27", import.meta.url));

self.addEventListener("message", async (event) => {
  if (!["prepare", "prepare-generated"].includes(event.data?.type)) return;

  try {
    await ready;
    const started = performance.now();
    // 新流水线一次只接收一副牌靴，返回“结构化牌面 + 概率快照”；
    // 随机牌靴直接走生成数据入口，不再经过 CSV 文本中间层。
    const preparedStarted = performance.now();
    const preparedJson = event.data.type === "prepare-generated"
      ? prepareGeneratedShoe(
        event.data.rowsJson,
        event.data.decks,
        event.data.sourceBase ?? 0,
        event.data.sideBetRoundLimitsJson ?? "{}",
      )
      : event.data.stream
        ? (typeof prepareReplayShoeWithSideBetLimits === "function"
          ? prepareReplayShoeWithSideBetLimits(
            event.data.csvText,
            event.data.decks,
            Boolean(event.data.timestampOrder),
            event.data.sideBetRoundLimitsJson ?? "{}",
          )
          : prepareReplayShoe(
            event.data.csvText,
            event.data.decks,
            Boolean(event.data.timestampOrder),
          ))
        : prepareBaccaratCsvWeights(event.data.csvText, event.data.decks);

    // 转移 UTF-8 缓冲区所有权，不再跨线程克隆数万条概率对象。
    const serializeStarted = performance.now();
    const preparedBuffer = new TextEncoder().encode(preparedJson).buffer;
    self.postMessage({
      type: "complete",
      taskId: event.data.taskId,
      preparedBuffer,
      elapsedMilliseconds: performance.now() - started,
      prepareMilliseconds: performance.now() - preparedStarted,
      serializeMilliseconds: performance.now() - serializeStarted,
      preparedBytes: preparedBuffer.byteLength,
    }, [preparedBuffer]);
  } catch (error) {
    self.postMessage({
      type: "error",
      taskId: event.data?.taskId,
      message: error?.message ?? String(error),
    });
  }
});
