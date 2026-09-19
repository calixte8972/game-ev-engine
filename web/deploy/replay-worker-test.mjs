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
  const storedDetails = new Map();
  let storageOpens = 0;
  let activeWrites = 0;
  let maximumActiveWrites = 0;
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
    .replace(/import \* as wasm from "\.\/pkg\/game_ev_engine.js";/, "")
    .replace(/import \{ openReplayStore \} from "\.\/replay-storage.js";/, "")
    .replaceAll("import.meta.url", JSON.stringify(file.href));
  const context = vm.createContext({
    ...wasm, wasm, init: async () => {},
    ArrayBuffer, TextEncoder, TextDecoder, URL, performance,
    navigator: { hardwareConcurrency: 16, deviceMemory: workerData.memory ?? 8 },
    Worker: BrowserWorker,
    openReplayStore: async () => {
      storageOpens += 1;
      return {
        async put(store, rows) {
          activeWrites += 1;
          maximumActiveWrites = Math.max(maximumActiveWrites, activeWrites);
          try {
            if (workerData.storageDelayMs) await new Promise(resolve => setTimeout(resolve, workerData.storageDelayMs));
            if (workerData.failStorage) throw new Error("Injected storage quota failure");
            for (const row of rows) storedDetails.set(row.id, row.bet);
          } finally { activeWrites -= 1; }
        },
        async clearTemporary() { storedDetails.clear(); },
        close() {},
      };
    },
    self: {
      addEventListener(type, callback) {
        if (type === "message") parentPort.on("message", data => callback({ data }));
      },
      postMessage(data, transfer) {
        parentPort.postMessage({
          ...data, created, terminated,
          ...(data.type === "complete" ? {
            storedDetails: [...storedDetails.values()], storageOpens, maximumActiveWrites,
          } : {}),
        }, transfer);
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
      const detailBatches = [];
      const progressValues = [];
      let pendingDetailAcks = 0;
      let maximumPendingDetailAcks = 0;
      thread.on("error", reject);
      thread.on("message", message => {
        if (message.type === "progress" && Number.isFinite(Number(message.overall))) {
          progressValues.push(Number(message.overall));
        }
        if (message.type === "ready") {
          if (options.simulate) {
            thread.postMessage({
              type: "simulate", simulation: options.simulate,
              config: { ...config, ...settings },
            });
          } else if (options.useFile) {
            // 覆盖浏览器实际发送 File/Blob 的路径；Worker 应该按流读取精简 CSV，
            // 而不是要求主线程先复制整份文本或 ArrayBuffer。
            thread.postMessage({
              type: "replay", csvFile: new Blob([csvText]), config: { ...config, ...settings },
            });
          } else {
            const csvBuffer = new TextEncoder().encode(csvText).buffer;
            thread.postMessage({ type: "replay", csvBuffer, config: { ...config, ...settings } }, [csvBuffer]);
          }
        }
        if (message.type === "detail-reset") detailBatches.length = 0;
        if (message.type === "detail-batch") {
          detailBatches.push(...message.details);
          pendingDetailAcks += 1;
          maximumPendingDetailAcks = Math.max(maximumPendingDetailAcks, pendingDetailAcks);
          const acknowledge = () => {
            pendingDetailAcks -= 1;
            thread.postMessage({
              type: "detail-ack", runId: message.runId, batchId: message.batchId,
            });
          };
          if (options.ackDelayMs) setTimeout(acknowledge, options.ackDelayMs);
          else acknowledge();
        }
        if (message.type === "complete" || message.type === "error") {
          clearTimeout(timeout);
          thread.terminate();
          if (message.type === "error") reject(new Error(message.message));
          else resolve({
            ...message,
            report: { ...message.report, bets: message.report.bets?.length
              ? message.report.bets : detailBatches },
            progressValues,
            detailBatchCount: detailBatches.length,
            maximumPendingDetailAcks,
          });
        }
      });
    });
  }
  const serial = await run({ parallelReplay: false });
  assert.equal(serial.created, 0);
  assert.ok(serial.progressValues.some(value => value > 0.2), "单线程精简 CSV 应持续报告阶段进度");
  assert.ok(
    serial.progressValues.every((value, index, values) => index === 0 || value >= values[index - 1]),
    "单线程进度消息不能倒退",
  );
  const expected = JSON.parse(wasm.replayBaccaratCsvWithSideBetLimits(
    csv, 8, 0.009, -1, 10_000, 0.05, 500, 500,
    "standard", "martingale", 10, -1, 100, JSON.stringify(limits), true,
  ));
  assert.deepEqual(serial.report.bets, expected.bets, "单线程必须保留独立边注截止局数");
  assert.equal(serial.report.summary.final_bankroll, expected.summary.final_bankroll);
  assert.equal(serial.report.details_saved, true);
  const backpressured = await run(
    { parallelReplay: false }, csv,
    { simulate: { shoes: 12, maxRoundsPerShoe: 8, seed: "1042" }, ackDelayMs: 20 },
  );
  assert.ok(backpressured.detailBatchCount > 0);
  assert.ok(backpressured.maximumPendingDetailAcks <= 4,
    "IndexedDB 确认变慢时未确认明细批次不得超过 4");
  const summaryOnly = await run(
    { parallelReplay: false, saveReplayDetails: false }, csv,
    { simulate: { shoes: 4, maxRoundsPerShoe: 8, seed: "2042" } },
  );
  assert.equal(summaryOnly.report.details_saved, false);
  assert.equal(summaryOnly.report.detail_count, 0);
  assert.equal(summaryOnly.detailBatchCount, 0);
  assert.equal(summaryOnly.report.omitted_bet_details, summaryOnly.report.summary.placed_bet_count);
  // 不发送任何页面存储 ACK：后台 Worker 必须独立写完并结束。
  const workerStored = await run({ parallelReplay: true, parallelWorkerCount: 4, workerDetailStorage: true }, csv,
    { storageDelayMs: 10 });
  assert.equal(workerStored.detailBatchCount, 0);
  assert.equal(workerStored.storageOpens, 1);
  assert.equal(workerStored.maximumActiveWrites, 1, "写入背压只允许一个事务批次在途");
  assert.deepEqual(workerStored.storedDetails, serial.report.bets);
  assert.deepEqual(workerStored.report.summary, serial.report.summary);
  assert.ok(workerStored.timings.storageMs > 0);
  const workerSummary = await run({ workerDetailStorage: true, saveReplayDetails: false }, csv);
  assert.equal(workerSummary.storageOpens, 0, "汇总模式不得创建明细库");
  assert.deepEqual(workerSummary.report.summary, serial.report.summary);
  await assert.rejects(run({ workerDetailStorage: true }, csv, { failStorage: true }), /storage quota/);
  await assert.rejects(run({}, csv, {
    simulate: { shoes: 1_666_667, maxRoundsPerShoe: 60, seed: "42" },
  }), /1,666,666/);
  const limitSource = readFileSync(new URL("replay-worker.js", web), "utf8");
  const limitContext = vm.createContext({});
  vm.runInContext(limitSource.slice(limitSource.indexOf("const MAX_REPLAY_SHOES"),
    limitSource.indexOf("// 页面只有在对应")), limitContext);
  assert.doesNotThrow(() => limitContext.validateShoeCount(1_666_666));
  assert.throws(() => limitContext.validateShoeCount(1_666_667), /1,666,666/);
  const comparisonConfig = { stakeStrategy: "fixed", strategyParameter: 20 };
  const comparisonSerial = await run({
    parallelReplay: false, comparisonConfig,
  });
  const fixedSerial = await run({
    parallelReplay: false, ...comparisonConfig,
  });
  assert.deepEqual(comparisonSerial.report.bets, serial.report.bets,
    "开启对比后，策略 A 的逐笔结算不能改变");
  assert.deepEqual(comparisonSerial.report.summary, serial.report.summary,
    "开启对比后，策略 A 的汇总不能改变");
  assert.deepEqual(comparisonSerial.report.comparison.summary, fixedSerial.report.summary,
    "策略 B 必须等于对相同 CSV 单独运行 B 的结果");
  assert.deepEqual(comparisonSerial.report.comparison.chart_points, fixedSerial.report.chart_points,
    "策略 B 的资金点不能因对比模式而改变");
  const unrestrictedSideLimits = Object.fromEntries(Object.keys(limits).map(key => [key, 0]));
  const independentB = {
    payoutRule: "no_commission", rebateRate: 0.02,
    minimumEffectiveEv: -0.5, minimumSideBetEv: -0.5,
    bankroll: 20_000, maxFraction: 0.1, maxRoundStake: 700,
    tableLimit: 650, sideBetLimit: 25,
    stakeStrategy: "fixed", strategyParameter: 20,
    allowMultipleBets: true, sideBetRoundLimits: unrestrictedSideLimits,
  };
  const independentlyConfigured = await run({
    parallelReplay: false, comparisonConfig: independentB,
  });
  const independentBAlone = await run({ parallelReplay: false, ...independentB });
  assert.deepEqual(independentlyConfigured.report.summary, serial.report.summary,
    "B 修改资金、返水和边注限制后不能改变 A 的结算");
  assert.deepEqual(independentlyConfigured.report.comparison.summary, independentBAlone.report.summary,
    "B 的全部自定义设置应等于单独回测相同设置");
  assert.ok(independentlyConfigured.report.comparison.summary.placed_bets.big > serial.report.summary.placed_bets.big,
    "B 放宽大/小截止局后应确实增加对应边注下注");
  const independentlyConfiguredParallel = await run({
    parallelReplay: true, parallelWorkerCount: 4, comparisonConfig: independentB,
  });
  assert.deepEqual(independentlyConfiguredParallel.report.comparison.summary, independentBAlone.report.summary,
    "并行预计算也要覆盖 B 放宽后的边注范围");
  for (const point of comparisonSerial.report.comparison.chart_points.slice(1)) {
    assert.ok(point.timelineIndex >= point.index,
      "双策略曲线横轴应使用共同牌局顺序，而不是各自下注顺序");
  }
  const skippedInput = wasm.generateBaccaratCsv(5, 20, "20260914", 8);
  const withSkippedRounds = await run({
    minimumEffectiveEv: -0.002, minimumSideBetEv: 0,
    comparisonConfig,
  }, skippedInput);
  assert.ok(withSkippedRounds.report.comparison.chart_points.some(point => point.timelineIndex > point.index),
    "跳过局后资金曲线应保留共同牌局的横轴间隔");
  const trends = serial.report.trend_charts;
  const bettingRounds = new Set(serial.report.bets.map(bet =>
    [bet.table_id, bet.session_id, bet.round_no, bet.started_at].join("|")));
  assert.equal(trends.total_bets, serial.report.summary.placed_bet_count);
  assert.equal(trends.betting_rounds, bettingRounds.size);
  assert.equal(trends.stake_points.at(-1)?.index, trends.total_bets);
  assert.equal(trends.round_profit_points.at(-1)?.index, trends.betting_rounds);
  assert.equal(
    trends.round_distribution.reduce((sum, item) => sum + item.count, 0),
    trends.betting_rounds,
    "可下注子局数直方图应按局计数，同局多注不能重复",
  );
  assert.ok(Object.keys(trends.by_bet ?? {}).length > 0, "趋势图必须保留按玩法筛选的数据");
  for (const [bet, selected] of Object.entries(trends.by_bet)) {
    const selectedBets = serial.report.bets.filter(item => item.bet === bet);
    const selectedRounds = new Set(selectedBets.map(item =>
      [item.table_id, item.session_id, item.round_no, item.started_at].join("|")));
    assert.equal(selected.total_bets, selectedBets.length, `${bet} 的下注笔数筛选口径一致`);
    assert.equal(selected.betting_rounds, selectedRounds.size, `${bet} 的下注子局数筛选口径一致`);
    assert.equal(
      selected.round_distribution.reduce((sum, item) => sum + item.count, 0),
      selected.betting_rounds,
      `${bet} 的子局分布同局只计一次`,
    );
  }
  for (const count of [2, 4, 8]) {
    const parallel = await run({ parallelReplay: true, parallelWorkerCount: count });
    assert.equal(parallel.parallel, true, parallel.fallbackReason);
    assert.equal(parallel.created, Math.min(count, 8));
    assert.equal(parallel.terminated, count);
    assert.deepEqual(parallel.report.bets, serial.report.bets, `${count} 线程逐笔下注与顺序回放相同`);
    assert.equal(parallel.report.summary.final_bankroll, serial.report.summary.final_bankroll);
    assert.deepEqual(parallel.report.trend_charts, trends, `${count} 线程的三张新图表应与单线程一致`);
  }
  const streamedFile = await run(
    { parallelReplay: true, parallelWorkerCount: 4 }, csv, { useFile: true },
  );
  assert.equal(streamedFile.parallel, true, streamedFile.fallbackReason);
  assert.deepEqual(streamedFile.report.bets, serial.report.bets, "Blob 流式读取与 ArrayBuffer 回放结果相同");
  assert.equal(streamedFile.report.summary.final_bankroll, serial.report.summary.final_bankroll);

  // 随机回测不再经过 CSV 文本中间层；同时覆盖自动测速会复用首副牌靴，
  // 不改变单线程的逐笔结算结果。
  const simulation = { shoes: 8, maxRoundsPerShoe: 3, seed: "20260912" };
  const simulatedSerial = await run(
    { parallelReplay: false, autoTuneWorkers: false }, csv, { simulate: simulation },
  );
  const simulatedAuto = await run(
    { parallelReplay: true, parallelWorkerCount: 4, autoTuneWorkers: true },
    csv,
    { simulate: simulation },
  );
  assert.deepEqual(
    simulatedAuto.report.bets,
    simulatedSerial.report.bets,
    "随机 WASM 数据流与单线程结算结果相同",
  );
  assert.equal(
    simulatedAuto.report.summary.final_bankroll,
    simulatedSerial.report.summary.final_bankroll,
  );
  const simulatedComparison = await run(
    { parallelReplay: true, parallelWorkerCount: 4, comparisonConfig },
    csv,
    { simulate: simulation },
  );
  const simulatedFixed = await run(
    { parallelReplay: false, ...comparisonConfig },
    csv,
    { simulate: simulation },
  );
  assert.deepEqual(simulatedComparison.report.summary, simulatedSerial.report.summary,
    "同一个随机种子下，对比模式不能改变策略 A");
  assert.deepEqual(simulatedComparison.report.comparison.summary, simulatedFixed.report.summary,
    "同一个随机种子下，B 必须等于单独运行 B");
  // A 爆仓不能让调度器提前终止 B；B 仍要吃完相同随机牌靴的后续局。
  const stopSimulation = { shoes: 4, maxRoundsPerShoe: 30, seed: "1" };
  const stopConfig = {
    parallelReplay: false, bankroll: 100, rebateRate: 0, maxFraction: 1, maxRoundStake: 100,
    tableLimit: 100, sideBetLimit: 100, allowMultipleBets: false,
    stakeStrategy: "fixed", strategyParameter: 100,
  };
  // 实际穿过 Worker + WASM 的上限校验；使用确定的提前停止策略避免跑满亿局。
  const maximumShoes = await run({ ...stopConfig, saveReplayDetails: false }, csv, {
    simulate: { shoes: 1_666_666, maxRoundsPerShoe: 60, seed: "1" },
  });
  assert.equal(maximumShoes.report.dataset.session_count, 1_666_666);
  assert.equal(maximumShoes.report.dataset.total_rows, 99_999_960);
  assert.equal(maximumShoes.report.summary.stopped_early, true);
  assert.ok(maximumShoes.report.summary.replayed_rounds < 60);
  const stopComparison = await run({
    ...stopConfig, comparisonConfig: { stakeStrategy: "fixed", strategyParameter: 1 },
  }, csv, { simulate: stopSimulation });
  const stopBAlone = await run({
    ...stopConfig, stakeStrategy: "fixed", strategyParameter: 1,
  }, csv, { simulate: stopSimulation });
  assert.equal(stopComparison.report.summary.stopped_early, true);
  assert.ok(stopComparison.report.summary.replayed_rounds < stopComparison.report.comparison.summary.replayed_rounds,
    "A 提前停止后 B 必须继续结算");
  assert.deepEqual(stopComparison.report.comparison.summary, stopBAlone.report.summary);
  const stopBFirst = await run({
    parallelReplay: false,
    comparisonConfig: {
      bankroll: 100, rebateRate: 0, maxFraction: 1, maxRoundStake: 100,
      tableLimit: 100, sideBetLimit: 100, allowMultipleBets: false,
      stakeStrategy: "fixed", strategyParameter: 100,
    },
  }, csv, { simulate: stopSimulation });
  assert.equal(stopBFirst.report.comparison.summary.stopped_early, true);
  assert.equal(stopBFirst.report.summary.replayed_rounds, 120,
    "B 先爆仓时 A 仍须完成整个随机牌局流");
  assert.ok(Number(simulatedAuto.timings?.workerProbeMs) >= 0, "自动测速应记录阶段耗时");
  const failedCreation = await run({ parallelReplay: true, parallelWorkerCount: 8 }, csv, { failCreation: true });
  assert.equal(failedCreation.parallel, false);
  assert.ok(failedCreation.fallbackReason);
  assert.equal(failedCreation.created, failedCreation.terminated, "降级前必须回收所有子线程");
  assert.deepEqual(failedCreation.report.bets, expected.bets, "并行启动失败后从头回放，不重复结算");

  const lowMemory = await run({ parallelReplay: true, parallelWorkerCount: 8 }, csv, { memory: 2 });
  assert.equal(lowMemory.parallel, true);
  assert.equal(lowMemory.created, 2, "低内存设备将并行数限制为 2");
  assert.deepEqual(lowMemory.report.bets, expected.bets);

  for (const input of [csv + "\n".repeat(12_001), csv + "\n".repeat(4 * 1024 * 1024)]) {
    const padded = await run({ parallelReplay: true, parallelWorkerCount: 8 }, input);
    assert.equal(padded.parallel, true, "空白行不应触发整批单线程降级");
    assert.deepEqual(padded.report.bets, serial.report.bets, "空白行不应截断或重复结算");
  }
  // 多桌带时间的输入按“牌靴分组、全局时间交错”排列；让每副牌靴的第1局
  // 交错在前、第2/3局交错在后，覆盖等待结果占满背压队列时的优先派发逻辑。
  const interleavedSource = wasm.generateBaccaratCsv(5, 3, "4242", 8);
  const interleavedRows = interleavedSource.trim().split(/\r?\n/).slice(1).map((line) => {
    const match = line.match(/^(\d+),(\d+),(.*)$/);
    assert.ok(match, "生成的测试牌局格式应稳定");
    const sessionId = Number(match[1]);
    const roundNo = Number(match[2]);
    const shoeIndex = sessionId - 1_000_000;
    const timestampIndex = (roundNo - 1) * 5 + shoeIndex;
    return [
      "1", match[1], match[2],
      `2026-09-12T00:00:${String(timestampIndex).padStart(2, "0")}`,
      match[3],
    ].join(",");
  });
  const interleavedCsv = [
    "table_id,session_id,round_no,started_at,raw_cards",
    ...interleavedRows,
  ].join("\n");
  const interleavedSerial = await run({ parallelReplay: false }, interleavedCsv);
  const interleavedParallel = await run(
    { parallelReplay: true, parallelWorkerCount: 2 }, interleavedCsv,
  );
  assert.equal(interleavedParallel.parallel, true, interleavedParallel.fallbackReason);
  assert.deepEqual(
    interleavedParallel.report.bets,
    interleavedSerial.report.bets,
    "时间交错牌靴必须按全局顺序完成并行回放",
  );
  assert.equal(
    interleavedParallel.report.summary.final_bankroll,
    interleavedSerial.report.summary.final_bankroll,
  );
  const interleavedStored = await run({
    parallelReplay: true, parallelWorkerCount: 4, workerDetailStorage: true,
  }, interleavedCsv, { storageDelayMs: 10 });
  assert.deepEqual(interleavedStored.storedDetails, interleavedSerial.report.bets,
    "磁盘等待期间交错牌靴仍按原时间顺序结算");
  assert.deepEqual(interleavedStored.report.summary, interleavedSerial.report.summary);
  // 不先 parse 大整数；确认合并过程保留每一位权重和牌靴 ID。
  const source = readFileSync(new URL("replay-worker.js", web), "utf8");
  const merge = source.slice(source.indexOf("function mergePreparedResults"), source.indexOf("/* ----------------------------- 入口"));
  const context = vm.createContext({ TextDecoder });
  vm.runInContext(merge, context);
  const raw = '{"rounds":[{"session_id":18446744073709551615,"weight":9007199254740993}]}';
  assert.equal(context.mergePreparedResults([new TextEncoder().encode(raw).buffer]), raw);
  // 页面完成回测后必须释放旧实例；重复点击才创建新的实例，过期消息不渲染。
  const appSource = readFileSync(new URL("app.js", web), "utf8");
  const simulationPage = vm.createContext({
    MAX_SIMULATION_SHOES: 1_666_666,
    simulationShoes: { value: "1666666", setCustomValidity(value) { this.error = value; } },
    simulationRounds: { value: "60" }, simulationSeed: { value: "1" },
    simulationEstimate: {}, deckCount: { value: "8" },
    integerFormatter: new Intl.NumberFormat("en-US"), updateReplayButton() {},
    readNumber(selector, label, options) {
      const value = selector === "#simulation-shoes" ? 1_666_666 : 60;
      assert.ok(value <= options.max, `${label} 上限必须允许该配置`);
      return value;
    },
  });
  vm.runInContext(appSource.slice(appSource.indexOf("function maximumGuaranteedSimulationRounds"),
    appSource.indexOf("function setReplaySourceMode")), simulationPage);
  simulationPage.updateSimulationEstimate();
  assert.equal(simulationPage.simulationShoes.max, "1666666");
  assert.equal(simulationPage.simulationShoes.error, "");
  assert.match(simulationPage.simulationEstimate.textContent, /99,999,960 局/);
  assert.equal(simulationPage.simulationRequest().shoes, 1_666_666);
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
    updateReplayButton() {}, setReplayRunning() {}, setReplayProgress() {},
    replayDetailFlushTimer: null, replayDetailPendingRows: [], replayDetailSequence: 0,
    replayDetailStore: null, replayDetailWriteTail: Promise.resolve(),
    replayStorageStartedAt: 0, replayStorageElapsedMs: 0,
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
      assert.deepEqual(result.report.trend_charts, sequential.report.trend_charts);
    }
    console.log("PASS: 12,000 rounds, serial vs 4/8 Workers, repeated runs");
  }
  if (process.argv.includes("--profile")) {
    const sample = await run(
      { parallelReplay: true, parallelWorkerCount: 4, autoTuneWorkers: false },
      csv,
      { simulate: { shoes: 100, maxRoundsPerShoe: 10, seed: "20260914" } },
    );
    console.log("PROFILE:", JSON.stringify(sample.timings));
  }
  console.log("PASS: real WASM Workers 2/4/8, serial limits, memory fallback, creation cleanup, exact integers");
}
