/*
 * 牌靴回放的协调 Worker。
 *
 * 数据流：
 *
 *   页面主线程
 *      ↓ 一次提交配置/CSV
 *   本协调 Worker（任务调度 + 顺序合并）
 *      ↓ 按完整牌靴分片
 *   replay-shard-worker × 2～8（可选的并行概率预计算）
 *      ↓ 概率权重快照
 *   本协调 Worker 按键合并
 *      ↓ 原始 CSV + 预计算权重
 *   Rust 顺序回放器（本金、倍投、结算、提前停止）
 *      ↓ 完整报告
 *   页面主线程
 *
 * 重要边界：不同牌靴的概率枚举可以并行；共享滚动本金和递进策略不能并行。
 * 因此子 Worker 绝不直接跑最终下注结算，避免“每个 Worker 都拿一份初始本金”
 * 导致回测结果被错误放大。
 */
import init, {
  generateBaccaratCsv,
  replayBaccaratCsv,
  replayBaccaratCsvWithPreparedWeights,
} from "./pkg/game_ev_engine.js";

const defaultSideBetRoundLimits = {
  any_pair: 50,
  banker_pair: 50,
  player_pair: 50,
  perfect_pair: 45,
  big: 20,
  small: 20,
  lucky_seven: 50,
  super_lucky_seven: 30,
  lucky_six: 50,
  banker_dragon_bonus: 50,
  player_dragon_bonus: 50,
};

function finiteNumberOr(value, fallback) {
  const number = Number(value);
  return Number.isFinite(number) ? number : fallback;
}

/** 兼容旧版页面，补齐独立边注局数限制。 */
function normalizedSideBetRoundLimits(config) {
  const source = config.sideBetRoundLimits ?? {};
  const result = {};
  for (const [key, defaultValue] of Object.entries(defaultSideBetRoundLimits)) {
    const value = Number(source[key]);
    result[key] = Number.isInteger(value) && value >= 0 ? value : defaultValue;
  }

  if (!config.sideBetRoundLimits && Number.isInteger(Number(config.luckyBetMaxRound))) {
    const legacyLimit = Math.max(0, Number(config.luckyBetMaxRound));
    result.lucky_six = legacyLimit;
    result.lucky_seven = legacyLimit;
    result.super_lucky_seven = legacyLimit;
  }
  return result;
}

function commonArguments(config) {
  const minimumSideBetEv = finiteNumberOr(
    config.minimumSideBetEv,
    config.minimumEffectiveEv,
  );
  const sideBetLimit = finiteNumberOr(config.sideBetLimit, config.maxRoundStake);

  return [
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
    JSON.stringify(normalizedSideBetRoundLimits(config)),
    Boolean(config.allowMultipleBets),
  ];
}

/* ----------------------------- CSV 分片 ----------------------------- */

// 不能简单使用 split(",")：生成器的 raw_cards 字段本身包含逗号，并且数据库
// 导出可能使用双引号包裹字段。这里是一个小型 RFC4180 读取器，只用于边界分片；
// 最终的字段校验仍由 Rust csv crate 完成。
function parseCsvRecord(record) {
  const fields = [];
  let field = "";
  let quoted = false;

  for (let index = 0; index < record.length; index += 1) {
    const character = record[index];
    if (character === '"') {
      if (quoted && record[index + 1] === '"') {
        field += '"';
        index += 1;
      } else {
        quoted = !quoted;
      }
    } else if (character === "," && !quoted) {
      fields.push(field);
      field = "";
    } else {
      field += character;
    }
  }

  if (quoted) throw new Error("CSV 存在未闭合的双引号，无法安全拆分牌靴");
  fields.push(field);
  return fields;
}

function readCsvRecords(csvText) {
  const records = [];
  let start = 0;
  let quoted = false;

  for (let index = 0; index < csvText.length; index += 1) {
    const character = csvText[index];
    if (character === '"') {
      if (quoted && csvText[index + 1] === '"') {
        index += 1;
      } else {
        quoted = !quoted;
      }
    } else if (character === "\n" && !quoted) {
      const record = csvText.slice(start, index).replace(/\r$/, "");
      if (record.trim()) records.push(record);
      start = index + 1;
    }
  }

  if (quoted) throw new Error("CSV 存在未闭合的双引号，无法安全拆分牌靴");
  const tail = csvText.slice(start).replace(/\r$/, "");
  if (tail.trim()) records.push(tail);
  return records;
}

function normalizedHeader(value) {
  return value.replace(/^\uFEFF/, "").trim().toLowerCase();
}

function findColumn(header, aliases) {
  const wanted = new Set(aliases.map(normalizedHeader));
  return header.findIndex((value) => wanted.has(normalizedHeader(value)));
}

function valueAt(fields, index, fallback = "") {
  return index >= 0 ? fields[index]?.trim() ?? fallback : fallback;
}

function csvField(value) {
  const text = String(value ?? "");
  return /[",\r\n]/.test(text) ? `"${text.replaceAll('"', '""')}"` : text;
}

/**
 * 把完整 CSV 变成“每个元素都是完整牌靴”的若干分片。
 *
 * 子 Worker 不能从第 37 局开始猜前面已经扣过什么牌，所以分片单位必须是
 * `(table_id, session_id)`，不能按任意行号切断牌靴。这里把字段归一化成 Rust
 * 支持的标准列名，最终回放仍使用原始 CSV，因此不会改变数据库报告口径。
 */
function splitCsvIntoShards(csvText, desiredShardCount) {
  const records = readCsvRecords(csvText.replace(/^\uFEFF/, ""));
  if (records.length < 2) throw new Error("CSV 没有可拆分的数据行");

  const header = parseCsvRecord(records[0]);
  const tableIndex = findColumn(header, ["table_id", "table", "桌台", "桌号", "gi011"]);
  const sessionIndex = findColumn(header, ["session_id", "shoe", "牌靴", "gi002"]);
  const roundIndex = findColumn(header, ["round_no", "round", "局号", "子局数", "gi003"]);
  const startedIndex = findColumn(header, ["started_at", "开局时间", "gi004"]);
  const settledIndex = findColumn(header, ["settled_at", "开奖时间", "gi006"]);
  const cardsIndex = findColumn(header, ["raw_cards", "cards", "牌面", "开奖内容", "gi007"]);
  const resultIndex = findColumn(header, ["result_code", "result", "结果", "gi012"]);

  if (sessionIndex < 0 || roundIndex < 0 || cardsIndex < 0) {
    throw new Error("CSV 必须包含牌靴、子局数和牌面三列");
  }

  const groups = new Map();
  for (let rowIndex = 1; rowIndex < records.length; rowIndex += 1) {
    const fields = parseCsvRecord(records[rowIndex]);
    const tableId = valueAt(fields, tableIndex, "1") || "1";
    const sessionId = valueAt(fields, sessionIndex);
    const roundNo = valueAt(fields, roundIndex);
    const rawCards = valueAt(fields, cardsIndex);
    if (!sessionId || !roundNo || !rawCards) {
      // 保留这类行给最终 Rust 质量报告处理，而不是在 JS 预分片阶段静默丢掉。
      // 没有牌靴键的行无法分配到任何完整牌靴，使用稳定隔离键让最终回放明确
      // 报告它，而不是把它混入一张合法牌靴。
      const invalidKey = `__invalid__${rowIndex}`;
      groups.set(invalidKey, {
        rows: [[
          tableId,
          sessionId || String(900_000_000_000_000 + rowIndex),
          roundNo || "1",
          "",
          "",
          rawCards,
          "",
        ]],
      });
      continue;
    }

    const key = `${tableId}\u0000${sessionId}`;
    let group = groups.get(key);
    if (!group) {
      group = { rows: [] };
      groups.set(key, group);
    }
    group.rows.push([
      tableId,
      sessionId,
      roundNo,
      valueAt(fields, startedIndex),
      valueAt(fields, settledIndex),
      rawCards,
      valueAt(fields, resultIndex),
    ]);
  }

  const groupsArray = [...groups.values()];
  const shardCount = Math.max(1, Math.min(desiredShardCount, groupsArray.length));
  const buckets = Array.from({ length: shardCount }, () => ({ rows: [], weight: 0 }));

  // 保持输入中的牌靴顺序，按连续区间切分：例如 400 靴/4 个 Worker 会得到
  // 1～100、101～200、201～300、301～400。这样结果日志更容易审计，也不让
  // 相邻牌靴被无意义地打散；若各靴长度不同，目标仍按行数近似均衡。
  const targetWeight = groupsArray.reduce((total, group) => total + group.rows.length, 0)
    / shardCount;
  let bucketIndex = 0;
  for (let groupIndex = 0; groupIndex < groupsArray.length; groupIndex += 1) {
    const remainingGroups = groupsArray.length - groupIndex;
    const remainingBuckets = shardCount - bucketIndex;
    const bucket = buckets[bucketIndex];
    if (bucketIndex < shardCount - 1
        && bucket.rows.length > 0
        && bucket.weight >= targetWeight
        && remainingGroups >= remainingBuckets) {
      bucketIndex += 1;
    }
    buckets[bucketIndex].rows.push(...groupsArray[groupIndex].rows);
    buckets[bucketIndex].weight += groupsArray[groupIndex].rows.length;
  }

  const shardHeader = [
    "table_id",
    "session_id",
    "round_no",
    "started_at",
    "settled_at",
    "raw_cards",
    "result_code",
  ].join(",");
  return buckets.map((bucket) => [
    shardHeader,
    ...bucket.rows.map((row) => row.map(csvField).join(",")),
  ].join("\n"));
}

/* -------------------------- 子 Worker 调度 -------------------------- */

function workerCountFor(taskCount, requestedWorkerCount = 4) {
  const hardware = Number(globalThis.navigator?.hardwareConcurrency) || 4;
  const requested = Number.isInteger(Number(requestedWorkerCount))
    ? Number(requestedWorkerCount)
    : 4;
  // 给页面主线程和协调 Worker 留出余量；最多八个子 Worker，避免大 CSV 回测
  // 在普通笔记本上把浏览器的全部核心和内存同时打满。
  return Math.max(1, Math.min(8, requested, taskCount, Math.max(1, hardware - 1)));
}

function runShardPool(shards, decks, requestedWorkerCount, onProgress) {
  if (shards.length === 0) return Promise.resolve([]);

  const poolSize = workerCountFor(shards.length, requestedWorkerCount);
  const results = new Array(shards.length);
  const workers = [];
  let nextTask = 0;
  let completed = 0;
  let settled = false;

  return new Promise((resolve, reject) => {
    const cleanup = () => {
      for (const worker of workers) worker.terminate();
    };

    const fail = (error) => {
      if (settled) return;
      settled = true;
      cleanup();
      reject(error instanceof Error ? error : new Error(String(error)));
    };

    const dispatch = (worker) => {
      if (settled) return;
      if (nextTask >= shards.length) {
        if (completed === shards.length) {
          settled = true;
          cleanup();
          resolve(results);
        }
        return;
      }

      const taskId = nextTask;
      nextTask += 1;
      worker.busyTaskId = taskId;
      worker.postMessage({
        type: "prepare",
        taskId,
        decks,
        csvText: shards[taskId],
      });
    };

    for (let index = 0; index < poolSize; index += 1) {
      const worker = new Worker(
        new URL("./replay-shard-worker.js?v=21", import.meta.url),
        { type: "module" },
      );
      worker.busyTaskId = null;
      worker.addEventListener("message", (event) => {
        const message = event.data;
        if (message.type === "complete") {
          results[message.taskId] = message.prepared;
          completed += 1;
          onProgress(completed, shards.length, poolSize);
          worker.busyTaskId = null;
          dispatch(worker);
        } else if (message.type === "error") {
          fail(new Error(message.message));
        }
      });
      worker.addEventListener("error", (event) => {
        fail(new Error(event.message || "牌靴概率子 Worker 无法启动"));
      });
      workers.push(worker);
    }

    for (const worker of workers) dispatch(worker);
  });
}

function mergePreparedResults(results) {
  const merged = [];
  const keys = new Set();
  for (const result of results) {
    for (const item of result.rounds ?? []) {
      const key = `${item.table_id}\u0000${item.session_id}\u0000${item.round_no}`;
      if (keys.has(key)) throw new Error(`并行结果重复：${item.session_id} 牌靴第 ${item.round_no} 局`);
      keys.add(key);
      merged.push(item);
    }
  }
  // 这里排序只是让结构化数据稳定、便于日志复核；最终资金顺序由原始 CSV
  // 的 started_at/行顺序决定，绝不使用子 Worker 的完成顺序。
  merged.sort((left, right) => (
    Number(left.table_id) - Number(right.table_id)
      || Number(left.session_id) - Number(right.session_id)
      || Number(left.round_no) - Number(right.round_no)
  ));
  return { rounds: merged };
}

/* ----------------------------- 主流程 ----------------------------- */

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
  if (!new Set(["replay", "simulate"]).has(event.data?.type)) return;

  try {
    await ready;
    const { config } = event.data;
    const started = performance.now();
    let csvText;

    if (event.data.type === "simulate") {
      const { shoes, maxRoundsPerShoe, seed } = event.data.simulation;
      self.postMessage({ type: "progress", phase: "generate", completed: 0, total: shoes });
      csvText = generateBaccaratCsv(shoes, maxRoundsPerShoe, seed, config.decks);
    } else {
      csvText = typeof event.data.csvText === "string"
        ? event.data.csvText
        : new TextDecoder("utf-8").decode(event.data.csvBuffer);
    }

    const requestedWorkerCount = Number(config.parallelWorkerCount ?? 1);
    const parallel = Boolean(config.parallelReplay) && requestedWorkerCount > 1;
    if (!parallel) {
      // 单线程模式不拆 CSV、不复制牌靴分片，也不创建子 Worker；这条路径是
      // 大文件或浏览器内存紧张时的稳定兜底。资金、倍投和提前停止仍由 Rust
      // 在同一个顺序回放函数中完成，结果与并行模式保持一致。
      self.postMessage({
        type: "progress",
        phase: "serial",
        completed: 0,
        total: 1,
        parallel: false,
        workerCount: 1,
      });
      const reportJson = replayBaccaratCsv(
        csvText,
        ...commonArguments(config),
      );
      self.postMessage({
        type: "complete",
        report: JSON.parse(reportJson),
        elapsedMilliseconds: performance.now() - started,
        workerCount: 1,
        shardCount: 1,
        parallel: false,
      });
      return;
    }

    const shards = splitCsvIntoShards(
      csvText,
      workerCountFor(Number.MAX_SAFE_INTEGER, requestedWorkerCount),
    );
    const effectiveWorkerCount = workerCountFor(shards.length, requestedWorkerCount);
    self.postMessage({
      type: "progress",
      phase: "probability",
      completed: 0,
      total: shards.length,
      workerCount: effectiveWorkerCount,
      parallel: true,
    });

    const preparedResults = await runShardPool(
      shards,
      config.decks,
      requestedWorkerCount,
      (completed, total, workerCount) => self.postMessage({
        type: "progress",
        phase: "probability",
        completed,
        total,
        workerCount,
        parallel: true,
      }),
    );
    const preparedWeightsJson = JSON.stringify(mergePreparedResults(preparedResults));

    self.postMessage({ type: "progress", phase: "settlement" });
    const reportJson = replayBaccaratCsvWithPreparedWeights(
      csvText,
      ...commonArguments(config),
      preparedWeightsJson,
    );

    self.postMessage({
      type: "complete",
      report: JSON.parse(reportJson),
      elapsedMilliseconds: performance.now() - started,
      workerCount: effectiveWorkerCount,
      shardCount: shards.length,
      parallel: true,
    });
  } catch (error) {
    self.postMessage({
      type: "error",
      message: error?.message ?? String(error),
    });
  }
});
