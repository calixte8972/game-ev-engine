// 在 Node 的真实线程中运行网站 Worker 源码和发布用 WASM。
// 仅适配 Worker/self 的宿主 API，CSV 分片、接口参数、传输和结算均使用原代码。
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import { Worker as Thread, isMainThread, parentPort, workerData } from "node:worker_threads";
import * as wasm from "../pkg/game_ev_engine.js";

const web = new URL("../", import.meta.url);
const bytes = readFileSync(new URL("pkg/game_ev_engine_bg.wasm", web));
wasm.initSync({ module: bytes });

if (!isMainThread) {
  let created = 0;
  let terminated = 0;
  class BrowserWorker {
    constructor(url) {
      if (workerData.failCreation && created === 1) throw new Error("Injected worker creation failure");
      this.thread = new Thread(new URL(import.meta.url), {
        workerData: { script: url.pathname.split("/").pop() },
      });
      created += 1;
    }
    addEventListener(type, callback) {
      if (type === "message") this.thread.on("message", data => callback({ data }));
      if (type === "error") this.thread.on("error", error => callback({ message: error.message }));
    }
    postMessage(data, transfer) { this.thread.postMessage(data, transfer); }
    terminate() {
      if (this.stopped) return;
      this.stopped = true;
      terminated += 1;
      this.thread.terminate();
    }
  }
  const file = new URL(workerData.script, web);
  const source = readFileSync(file, "utf8")
    .replace(/import init,\s*\{[\s\S]*?\}\s*from\s*"\.\/pkg\/game_ev_engine.js";/, "")
    .replaceAll("import.meta.url", JSON.stringify(file.href));
  const context = vm.createContext({
    ...wasm, init: async () => {},
    ArrayBuffer, TextEncoder, TextDecoder, URL, performance,
    navigator: { hardwareConcurrency: 16, deviceMemory: workerData.memory ?? 8 },
    Worker: BrowserWorker,
    self: {
      addEventListener(type, callback) {
        if (type === "message") parentPort.on("message", data => callback({ data }));
      },
      postMessage(data, transfer) {
        parentPort.postMessage({ ...data, created, terminated }, transfer);
      },
    },
  });
  vm.runInContext(source, context, { filename: file.pathname });
} else {
  const limits = {
    any_pair: 1, banker_pair: 2, player_pair: 1, perfect_pair: 1, big: 1, small: 1,
    lucky_seven: 1, super_lucky_seven: 1, lucky_six: 1,
    banker_dragon_bonus: 1, player_dragon_bonus: 1,
  };
  const config = {
    decks: 8, rebateRate: 0.009, minimumEffectiveEv: -1, minimumSideBetEv: -1,
    bankroll: 10_000, maxFraction: 0.05, maxRoundStake: 500, tableLimit: 500,
    sideBetLimit: 100, payoutRule: "standard", stakeStrategy: "martingale",
    strategyParameter: 10, allowMultipleBets: true, sideBetRoundLimits: limits,
  };
  const csv = wasm.generateBaccaratCsv(8, 3, "42", 8);
  function run(settings, csvText = csv, options = {}) {
    return new Promise((resolve, reject) => {
      const thread = new Thread(new URL(import.meta.url), {
        workerData: { script: "replay-worker.js", ...options },
      });
      const timeout = setTimeout(() => { thread.terminate(); reject(new Error("Worker timeout")); }, 30_000);
      thread.on("error", reject);
      thread.on("message", message => {
        if (message.type === "ready") {
          const csvBuffer = new TextEncoder().encode(csvText).buffer;
          thread.postMessage({ type: "replay", csvBuffer, config: { ...config, ...settings } }, [csvBuffer]);
        }
        if (message.type === "complete" || message.type === "error") {
          clearTimeout(timeout);
          thread.terminate();
          if (message.type === "error") reject(new Error(message.message));
          else resolve(message);
        }
      });
    });
  }
  const serial = await run({ parallelReplay: false });
  assert.equal(serial.created, 0);
  const expected = JSON.parse(wasm.replayBaccaratCsvWithSideBetLimits(
    csv, 8, 0.009, -1, 10_000, 0.05, 500, 500,
    "standard", "martingale", 10, -1, 100, JSON.stringify(limits), true,
  ));
  assert.deepEqual(serial.report, expected, "单线程必须保留独立边注截止局数");
  for (const count of [2, 4, 8]) {
    const parallel = await run({ parallelReplay: true, parallelWorkerCount: count });
    assert.equal(parallel.parallel, true, parallel.fallbackReason);
    assert.equal(parallel.created, count);
    assert.equal(parallel.terminated, count);
    assert.deepEqual(parallel.report.bets, serial.report.bets, `${count} 线程逐笔下注与顺序回放相同`);
    assert.equal(parallel.report.summary.final_bankroll, serial.report.summary.final_bankroll);
  }
  for (const [input, options] of [
    [csv, { failCreation: true }],
    [csv, { memory: 2 }],
    [csv + "\n".repeat(12_001), {}],
    [csv + "\n".repeat(4 * 1024 * 1024), {}],
  ]) {
    const fallback = await run({ parallelReplay: true, parallelWorkerCount: 8 }, input, options);
    assert.equal(fallback.parallel, false);
    assert.ok(fallback.fallbackReason);
    assert.equal(fallback.created, fallback.terminated, "降级前必须回收所有子线程");
    assert.deepEqual(fallback.report.bets, serial.report.bets, "降级不截断数据或重复结算");
  }
  // 不先 parse 大整数；确认合并过程保留每一位权重和牌靴 ID。
  const source = readFileSync(new URL("replay-worker.js", web), "utf8");
  const merge = source.slice(source.indexOf("function mergePreparedResults"), source.indexOf("/* ----------------------------- 主流程"));
  const context = vm.createContext({ TextDecoder });
  vm.runInContext(merge, context);
  const raw = '{"rounds":[{"session_id":18446744073709551615,"weight":9007199254740993}]}';
  assert.equal(context.mergePreparedResults([new TextEncoder().encode(raw).buffer]), raw);
  // 页面完成回测后必须释放旧实例；重复点击才创建新的实例，过期消息不渲染。
  const appSource = readFileSync(new URL("app.js", web), "utf8");
  let instances = 0;
  let rendered = 0;
  class PageWorker {
    constructor() { instances += 1; }
    terminate() { this.stopped = true; }
    addEventListener() {}
  }
  const page = vm.createContext({
    Worker: PageWorker, URL, replayWorkerReady: false,
    replayStatus: { textContent: "等待选择文件" }, replaySourceMode: "csv", currentCsvFile: null,
    updateReplayButton() {}, setReplayRunning() {},
    renderReplay() { assert.equal(page.oldWorker.stopped, true); rendered += 1; },
  });
  const lifecycle = appSource.slice(appSource.indexOf("let replayWorker;"), appSource.indexOf("function selectedMode"))
    + appSource.slice(appSource.indexOf("function handleReplayMessage"), appSource.indexOf("async function start()"));
  vm.runInContext(lifecycle.replaceAll("import.meta.url", JSON.stringify(new URL("app.js", web).href)), page);
  vm.runInContext('resetReplayWorker(); globalThis.oldWorker = replayWorker; handleReplayMessage({ target: replayWorker, data: { type: "complete", parallel: true, workerCount: 2, report: {} } });', page);
  assert.equal(instances, 1, "完成后不自动再开线程");
  assert.equal(rendered, 1);
  vm.runInContext('handleReplayMessage({ target: oldWorker, data: { type: "complete" } }); resetReplayWorker();', page);
  assert.equal(rendered, 1, "已终止实例的迟到消息必须忽略");
  assert.equal(instances, 2, "新回测使用全新的 WASM 内存");
  if (process.argv.includes("--stress")) {
    const largeCsv = wasm.generateBaccaratCsv(200, 60, "20260912", 8);
    const sequential = await run({ parallelReplay: false }, largeCsv);
    for (const count of [4, 8]) {
      const result = await run({ parallelReplay: true, parallelWorkerCount: count }, largeCsv);
      assert.equal(result.parallel, true, result.fallbackReason);
      assert.equal(result.created, result.terminated);
      assert.deepEqual(result.report.bets, sequential.report.bets);
      assert.equal(result.report.summary.final_bankroll, sequential.report.summary.final_bankroll);
    }
    console.log("PASS: 12,000 rounds, serial vs 4/8 Workers, repeated runs");
  }
  console.log("PASS: real WASM Workers 2/4/8, serial limits, memory fallback, creation cleanup, exact integers");
}
