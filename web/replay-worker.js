/*
 * 牌靴回放协调 Worker。
 *
 *   CSV/随机生成 -> 每个子 Worker 只计算一副牌靴的概率
 *                -> 有界在途队列
 *                -> 本 Worker 的 ReplaySession 按全局顺序结算本金/倍投
 *                -> 对比模式下，第二个 ReplaySession 消费同一批牌局概率
 *                -> 每个小批次的明细交给页面 IndexedDB
 *
 * 子 Worker 数量可以改变，但每套策略始终只有一个资金状态。这样不会因
 * 概率计算并行而把本金按牌靴复制，也不用把全部概率 JSON 留在内存中。
 */
import * as wasm from "./pkg/game_ev_engine.js";

const init = wasm.default;
const {
  generateBaccaratCsv,
  replayBaccaratCsvWithSideBetLimits,
  replayBaccaratCsvWithPreparedWeights,
  ReplaySession,
  ShoeGenerator,
  inspectReplayShoe,
  prepareReplayShoe,
  prepareReplayShoeWithSideBetLimits,
  prepareGeneratedShoe,
} = wasm;

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

// Rust 的流式结算接口当前允许最多 256 局一批。统一在这里使用上限，减少
// `session.push()` 调用次数；如果将来核心上限变化，只需要改这一处。
const REPLAY_BATCH_SIZE = 256;
const PROGRESS_THROTTLE_MS = 120;

function finiteNumberOr(value, fallback) {
  const number = Number(value);
  return Number.isFinite(number) ? number : fallback;
}

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
  const minimumSideBetEv = finiteNumberOr(config.minimumSideBetEv, config.minimumEffectiveEv);
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

function streamConfigJson(config, sessionCount) {
  return JSON.stringify({
    decks: config.decks,
    rebateRate: config.rebateRate,
    minimumEffectiveEv: config.minimumEffectiveEv,
    minimumSideBetEv: finiteNumberOr(config.minimumSideBetEv, config.minimumEffectiveEv),
    bankroll: config.bankroll,
    maxFraction: config.maxFraction,
    maxRoundStake: config.maxRoundStake,
    tableLimit: config.tableLimit,
    sideBetLimit: finiteNumberOr(config.sideBetLimit, config.maxRoundStake),
    payoutRule: config.payoutRule,
    stakeStrategy: config.stakeStrategy,
    strategyParameter: config.strategyParameter,
    sideBetRoundLimits: normalizedSideBetRoundLimits(config),
    allowMultipleBets: Boolean(config.allowMultipleBets),
    sessionCount,
  });
}

function preparationSideBetRoundLimitsJson(config, comparisonConfig = null) {
  const primary = normalizedSideBetRoundLimits(config);
  if (!comparisonConfig) return JSON.stringify(primary);
  const secondary = normalizedSideBetRoundLimits(comparisonConfig);
  // 概率预计算必须覆盖 A/B 任意一方仍可下注的边注。0 是“不限局数”，
  // 因而并集里的 0 比任何有限截止局都更宽；各资金会话仍按自己的限制决策。
  return JSON.stringify(Object.fromEntries(Object.keys(primary).map((key) => [
    key,
    primary[key] === 0 || secondary[key] === 0
      ? 0 : Math.max(primary[key], secondary[key]),
  ])));
}

/* ----------------------------- CSV 读取 ----------------------------- */

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
      if (quoted && csvText[index + 1] === '"') index += 1;
      else quoted = !quoted;
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

/**
 * File 流式 CSV 读取器。浏览器把 Blob 分块交给 Worker，跨块保留引号状态；
 * 因此 200 MB 文件不会先在主线程再复制一份完整字符串。
 */
async function* readCsvFieldsFromBlob(blob) {
  const reader = blob.stream().getReader();
  const decoder = new TextDecoder("utf-8", { fatal: true });
  let fields = [];
  let field = "";
  let quoted = false;
  let afterQuote = false;
  let skipLf = false;
  let firstChunk = true;
  let lineLength = 0;
  try {
    while (true) {
      const { value, done } = await reader.read();
      let text = done ? decoder.decode() : decoder.decode(value, { stream: true });
      if (firstChunk) {
        text = text.replace(/^\uFEFF/, "");
        firstChunk = false;
      }
      for (const character of text) {
        if (skipLf) {
          skipLf = false;
          if (character === "\n") continue;
        }
        lineLength += 1;
        if (lineLength > 1024 * 1024) throw new Error("CSV 单行超过 1 MB，请检查引号或文件格式");
        if (quoted && !afterQuote) {
          if (character === '"') afterQuote = true;
          else field += character;
          continue;
        }
        if (quoted && afterQuote && character === '"') {
          field += '"';
          afterQuote = false;
          continue;
        }
        if (afterQuote) {
          if (character !== "," && character !== "\r" && character !== "\n") {
            throw new Error("CSV 闭合引号后有非法字符");
          }
          quoted = false;
          afterQuote = false;
        }
        if (character === '"') {
          if (field.length) throw new Error("CSV 未转义的双引号");
          quoted = true;
        } else if (character === ",") {
          fields.push(field);
          field = "";
        } else if (character === "\r" || character === "\n") {
          fields.push(field);
          if (fields.length > 1 || fields[0].length) yield fields;
          fields = [];
          field = "";
          lineLength = 0;
          skipLf = character === "\r";
        } else {
          field += character;
        }
      }
      if (done) break;
    }
    if (quoted && !afterQuote) throw new Error("CSV 引号未闭合");
    if (field.length || fields.length || afterQuote) {
      fields.push(field);
      yield fields;
    }
  } finally {
    reader.releaseLock();
  }
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

function compareNumericText(left, right) {
  try {
    const a = BigInt(left);
    const b = BigInt(right);
    return a < b ? -1 : a > b ? 1 : 0;
  } catch {
    return String(left).localeCompare(String(right));
  }
}

/**
 * 按完整牌靴切成任务。每个任务最多约 104 局，结果可以在结算后立即释放。
 * source_order 保留原始行号，避免 JS 用 Number 处理 u64 牌靴编号。
 */
function splitCsvIntoShoeTasks(csvText) {
  const records = readCsvRecords(csvText.replace(/^\uFEFF/, ""));
  if (records.length < 2) throw new Error("CSV 没有可拆分的数据行");
  const header = parseCsvRecord(records[0]);
  const tableIndex = findColumn(header, ["table_id", "table", "桌台", "桌号", "gi011"]);
  const sourcePkIndex = findColumn(header, ["__source_pk", "source_pk"]);
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
  const timeline = [];
  let allHaveStartedAt = true;
  for (let rowIndex = 1; rowIndex < records.length; rowIndex += 1) {
    const fields = parseCsvRecord(records[rowIndex]);
    const tableId = valueAt(fields, tableIndex, "1") || "1";
    const sessionId = valueAt(fields, sessionIndex);
    const roundNo = valueAt(fields, roundIndex);
    const startedAt = valueAt(fields, startedIndex);
    const metadata = { sourceOrder: rowIndex - 1, tableId, sessionId, roundNo, startedAt };
    const row = [
      valueAt(fields, sourcePkIndex), tableId, sessionId, roundNo, startedAt,
      valueAt(fields, settledIndex), valueAt(fields, cardsIndex), valueAt(fields, resultIndex),
      String(rowIndex - 1),
    ];
    timeline.push(metadata);
    if (!startedAt) allHaveStartedAt = false;
    const key = `${tableId}\u0000${sessionId || `__invalid__${rowIndex}`}`;
    if (!groups.has(key)) groups.set(key, { rows: [], metadata: [] });
    groups.get(key).rows.push(row);
    groups.get(key).metadata.push(metadata);
  }

  const shardHeader = [
    "__source_pk", "table_id", "session_id", "round_no", "started_at",
    "settled_at", "raw_cards", "result_code", "source_order",
  ].join(",");
  const tasks = [...groups.values()].map((group, id) => ({
    id,
    csvText: [shardHeader, ...group.rows.map((row) => row.map(csvField).join(","))].join("\n"),
    metadata: group.metadata,
  }));
  timeline.sort((left, right) => {
    if (allHaveStartedAt) {
      return left.startedAt.localeCompare(right.startedAt)
        || compareNumericText(left.tableId, right.tableId)
        || compareNumericText(left.sessionId, right.sessionId)
        || compareNumericText(left.roundNo, right.roundNo)
        || left.sourceOrder - right.sourceOrder;
    }
    return left.sourceOrder - right.sourceOrder;
  });
  return { tasks, order: timeline.map((row) => row.sourceOrder), allHaveStartedAt };
}

function splitStatsDataset(stats) {
  const dates = [...stats.dates].sort();
  return {
    total_rows: stats.totalRows,
    table_count: stats.tables.size,
    session_count: stats.sessions.size,
    business_date_count: dates.length,
    business_date_min: dates[0] ?? "",
    business_date_max: dates.at(-1) ?? "",
    started_at_min: stats.startedAtMin,
    started_at_max: stats.startedAtMax,
    settled_at_min: stats.settledAtMin,
    settled_at_max: stats.settledAtMax,
    duplicate_source_pk_rows: 0,
    duplicate_round_keys: 0,
  };
}

/** 对 Blob 做一次流式分组，返回和字符串分片相同的任务协议。 */
async function splitCsvBlobIntoShoeTasks(blob) {
  let header = null;
  let indexes;
  const taskHeader = [
    "__source_pk", "table_id", "session_id", "round_no", "started_at",
    "settled_at", "raw_cards", "result_code", "source_order",
  ].join(",");
  const tasks = [];
  const seenKeys = new Set();
  let activeKey = null;
  let activeRows = [];
  let activeSourceOrders = [];
  const timeline = [];
  const stats = {
    totalRows: 0, tables: new Set(), sessions: new Set(), dates: new Set(),
    startedAtMin: "", startedAtMax: "", settledAtMin: "", settledAtMax: "",
  };
  let allHaveStartedAt = true;

  // 精简 CSV 通常按“牌靴 -> 局号”导出。牌靴结束时立刻把当前行数组编码成
  // 一段任务文本，避免把百万行拆成长期存活的 JS 小对象；任务文本仍会在
  // 派发给子 Worker 后释放。若同一牌靴再次出现，说明输入交错，转交旧的
  // 完整读取路径处理，避免在这里猜测牌靴边界。
  const finishActiveTask = () => {
    if (activeKey === null) return;
    tasks.push({
      id: tasks.length,
      csvText: [taskHeader, ...activeRows.map((row) => row.map(csvField).join(","))].join("\n"),
      sourceOrders: activeSourceOrders,
    });
    activeRows = [];
    activeSourceOrders = [];
    activeKey = null;
  };

  for await (const fields of readCsvFieldsFromBlob(blob)) {
    if (!header) {
      header = fields;
      indexes = {
        table: findColumn(header, ["table_id", "table", "桌台", "桌号", "gi011"]),
        sourcePk: findColumn(header, ["__source_pk", "source_pk"]),
        session: findColumn(header, ["session_id", "shoe", "牌靴", "gi002"]),
        round: findColumn(header, ["round_no", "round", "局号", "子局数", "gi003"]),
        started: findColumn(header, ["started_at", "开局时间", "gi004"]),
        settled: findColumn(header, ["settled_at", "开奖时间", "gi006"]),
        cards: findColumn(header, ["raw_cards", "cards", "牌面", "开奖内容", "gi007"]),
        result: findColumn(header, ["result_code", "result", "结果", "gi012"]),
      };
      if (indexes.session < 0 || indexes.round < 0 || indexes.cards < 0) {
        throw new Error("CSV 必须包含牌靴、子局数和牌面三列");
      }
      // 来源主键的全局去重需要扫描完整数据；保留旧 Rust 路径，避免把大 Set
      // 引入浏览器内存。精简三列 CSV 才进入真正的流式分组路径。
      if (indexes.sourcePk >= 0) return { requiresLegacy: true };
      continue;
    }
    const sourceOrder = stats.totalRows;
    const tableId = valueAt(fields, indexes.table, "1") || "1";
    const sessionId = valueAt(fields, indexes.session);
    const roundNo = valueAt(fields, indexes.round);
    const startedAt = valueAt(fields, indexes.started);
    const settledAt = valueAt(fields, indexes.settled);
    const metadata = { sourceOrder, tableId, sessionId, roundNo, startedAt };
    const row = [
      "", tableId, sessionId, roundNo, startedAt, settledAt,
      valueAt(fields, indexes.cards), valueAt(fields, indexes.result), String(sourceOrder),
    ];
    stats.totalRows += 1;
    stats.tables.add(tableId);
    stats.sessions.add(`${tableId}\u0000${sessionId}`);
    if (!startedAt) allHaveStartedAt = false;
    if (startedAt) timeline.push(metadata);
    if (startedAt) {
      const date = startedAt.slice(0, 10);
      stats.dates.add(date);
      if (!stats.startedAtMin || startedAt < stats.startedAtMin) stats.startedAtMin = startedAt;
      if (!stats.startedAtMax || startedAt > stats.startedAtMax) stats.startedAtMax = startedAt;
    }
    if (settledAt) {
      if (!stats.settledAtMin || settledAt < stats.settledAtMin) stats.settledAtMin = settledAt;
      if (!stats.settledAtMax || settledAt > stats.settledAtMax) stats.settledAtMax = settledAt;
    }
    const key = `${tableId}\u0000${sessionId || `__invalid__${sourceOrder}`}`;
    if (activeKey === null) {
      activeKey = key;
    } else if (key !== activeKey) {
      if (seenKeys.has(key)) return { requiresLegacy: true };
      seenKeys.add(activeKey);
      finishActiveTask();
      activeKey = key;
    }
    activeRows.push(row);
    activeSourceOrders.push(sourceOrder);
  }

  if (!header || stats.totalRows === 0) throw new Error("CSV 没有可拆分的数据行");
  finishActiveTask();
  let order = null;
  if (allHaveStartedAt) {
    timeline.sort((left, right) => {
      return left.startedAt.localeCompare(right.startedAt)
        || compareNumericText(left.tableId, right.tableId)
        || compareNumericText(left.sessionId, right.sessionId)
        || compareNumericText(left.roundNo, right.roundNo)
        || left.sourceOrder - right.sourceOrder;
    });
    order = timeline.map((row) => row.sourceOrder);
  }
  const totalRows = stats.totalRows;
  const sessionCount = tasks.length;
  return {
    tasks,
    order,
    allHaveStartedAt,
    dataset: splitStatsDataset(stats),
    quality: {
      valid_card_rows: totalRows, empty_card_rows: 0, invalid_card_rows: 0,
      outcome_mismatch_rows: 0, sessions_starting_at_one: sessionCount,
      sessions_starting_mid_shoe: 0, sessions_with_round_gaps: 0,
      sessions_with_empty_cards: 0, sessions_with_invalid_rows: 0,
      fully_observable_sessions: sessionCount, quarantined_rounds: 0,
    },
  };
}

/** 建立“原始行号 -> 牌靴任务”索引，让时间交错时可以优先派发下一项所需任务。 */
function sourceOrderTaskMap(tasks) {
  const result = new Map();
  for (let taskId = 0; taskId < tasks.length; taskId += 1) {
    const task = tasks[taskId];
    const sourceOrders = task.sourceOrders
      ?? task.metadata?.map((entry) => entry.sourceOrder)
      ?? [];
    for (const sourceOrder of sourceOrders) {
      if (result.has(Number(sourceOrder))) {
        throw new Error("同一原始行被分配到多个牌靴任务");
      }
      result.set(Number(sourceOrder), taskId);
    }
  }
  return result;
}

function serializeShoeTask(task) {
  if (typeof task.csvText === "string") {
    const csvText = task.csvText;
    task.csvText = null;
    return { csvText };
  }
  const header = task.header ?? [
    "__source_pk", "table_id", "session_id", "round_no", "started_at",
    "settled_at", "raw_cards", "result_code", "source_order",
  ].join(",");
  const csvText = [header, ...task.rows.map((row) => row.map(csvField).join(","))].join("\n");
  task.rows = null;
  task.metadata = null;
  return { csvText };
}

/* -------------------------- 有界并行调度 -------------------------- */

function workerCountFor(taskCount, requestedWorkerCount = 4) {
  const hardware = Number(globalThis.navigator?.hardwareConcurrency) || 4;
  const requested = Number.isInteger(Number(requestedWorkerCount)) ? Number(requestedWorkerCount) : 4;
  const memory = Number(globalThis.navigator?.deviceMemory);
  const memoryLimit = memory > 0 && memory <= 4 ? 2 : 8;
  return Math.max(1, Math.min(8, requested, taskCount, memoryLimit, Math.max(1, hardware - 1)));
}

/**
 * 用第一副真实任务做一次轻量预热，再决定是否值得把工作拆给更多 Worker。
 *
 * 这里的“自动”不是盲目按 hardwareConcurrency 开满：浏览器、WASM 线性内存
 * 和当前设备的实际负载都会影响并行收益。预热测的是同一条概率预计算路径，
 * 因而比只读取 CPU 核心数更接近本次回测的真实成本。用户输入的并行数仍是
 * 上限，自动选择只会在 1..上限范围内调整。
 */
function tunedWorkerCount(taskCount, upperBound, probePrepareMilliseconds) {
  const upper = Math.max(1, Math.min(upperBound, taskCount));
  if (upper <= 1 || taskCount < 2) return 1;
  const prepareMs = Math.max(0, Number(probePrepareMilliseconds) || 0);
  let recommended = 1;
  if (prepareMs >= 3) recommended = 2;
  if (prepareMs >= 10) recommended = 4;
  if (prepareMs >= 30) recommended = upper;
  return Math.max(1, Math.min(upper, recommended));
}

function decodePreparedBuffer(buffer) {
  return decodePreparedJson(new TextDecoder().decode(buffer));
}

/** 直接处理 Rust 返回的 JSON；单线程路径不再为了读取 JSON 重新编码 UTF-8。 */
function decodePreparedJson(json) {
  const packets = JSON.parse(json);
  if (!Array.isArray(packets)) throw new Error("并行牌靴结果格式无效");
  return packets
    .map((packet) => {
      const sourceOrder = Number(packet?.source?.source_order);
      if (!Number.isSafeInteger(sourceOrder) || !packet?.source || !packet?.prepared) {
        throw new Error("并行牌靴结果缺少稳定行号或结构化牌局");
      }
      return { sourceOrder, packet };
    })
    .sort((left, right) => left.sourceOrder - right.sourceOrder);
}

function isArrayBuffer(value) {
  // 不同浏览器 Worker 实现可能让 ArrayBuffer 来自另一个 JS realm，
  // 不能只依赖 `instanceof ArrayBuffer` 判断 transferable 结果。
  return value && typeof value.byteLength === "number"
    && Object.prototype.toString.call(value) === "[object ArrayBuffer]";
}

/** 每笔金额/每局盈亏只保留每段的最高、最低点，避免百万局图表占满内存。 */
class BoundedMetric {
  constructor() {
    this.index = 0;
    this.width = 1;
    this.buckets = new Map();
    this.first = null;
    this.last = null;
  }

  add(value) {
    const point = { index: ++this.index, value: Number(value) || 0 };
    this.first ??= point;
    this.last = point;
    this.addPoint(point);
  }

  addPoint(point) {
    const key = Math.floor((point.index - 1) / this.width);
    const bucket = this.buckets.get(key);
    if (!bucket) this.buckets.set(key, { low: point, high: point });
    else {
      if (point.value < bucket.low.value) bucket.low = point;
      if (point.value > bucket.high.value) bucket.high = point;
    }
    if (this.buckets.size > 600) {
      const saved = this.points();
      this.width *= 2;
      this.buckets.clear();
      for (const item of saved) this.addPoint(item);
    }
  }

  points() {
    const unique = new Map();
    if (this.first) unique.set(this.first.index, this.first);
    for (const bucket of this.buckets.values()) {
      unique.set(bucket.low.index, bucket.low);
      unique.set(bucket.high.index, bucket.high);
    }
    if (this.last) unique.set(this.last.index, this.last);
    return [...unique.values()].sort((a, b) => a.index - b.index);
  }
}

/** 曲线是有界诊断数据，完整下注明细由页面另行落盘。 */
class BoundedCurve {
  constructor(initial) {
    this.initial = Number(initial);
    this.index = 0;
    this.peak = this.initial;
    this.duration = 0;
    this.width = 1;
    this.buckets = new Map();
    this.last = null;
    this.pending = null;
    this.stakeMetric = new BoundedMetric();
    this.roundProfitMetric = new BoundedMetric();
    this.roundCounts = new Map();
    // 同一份回放报告同时保留“全部下注”和“按玩法”的有界曲线。
    // 这样页面切换庄、闲、对子等筛选时不需要重新回放，也不会把百万笔
    // 明细全部复制到主线程；每种玩法只保留峰谷采样点和局号计数。
    this.betTrends = new Map();
    this.first = {
      index: 0, bankroll: this.initial, cumulativeProfit: 0, drawdown: 0,
      drawdownRate: 0, drawdownDuration: 0, peakBankroll: this.initial,
      roundStake: 0, roundProfit: 0, betCount: 0, startedAt: "",
      tableId: null, sessionId: null, roundNo: null,
    };
  }

  addBets(bets, timelinePositions = null) {
    for (const bet of bets) {
      const key = [bet.table_id, bet.session_id, bet.round_no, bet.started_at].join("|");
      if (this.pending && this.pending.key !== key) this.flush();
      if (!this.pending) {
        this.pending = {
          key, bankroll: Number(bet.bankroll_after), roundStake: 0, roundProfit: 0,
          betCount: 0, tableId: bet.table_id, sessionId: bet.session_id,
          roundNo: bet.round_no, startedAt: bet.started_at, byBet: new Map(),
          timelineIndex: timelinePositions?.get(key) ?? this.index + 1,
        };
      }
      const stake = Number(bet.amount) || 0;
      this.stakeMetric.add(stake);
      this.pending.roundStake += stake;
      this.pending.roundProfit += Number(bet.actual_profit) || 0;
      this.pending.betCount += 1;
      const betKey = String(bet.bet || "unknown");
      const betPending = this.pending.byBet.get(betKey) ?? { stake: 0, profit: 0 };
      betPending.stake += stake;
      betPending.profit += Number(bet.actual_profit) || 0;
      this.pending.byBet.set(betKey, betPending);
    }
  }

  flush() {
    const pending = this.pending;
    if (!pending) return;
    this.peak = Math.max(this.peak, pending.bankroll);
    const drawdown = Math.max(0, this.peak - pending.bankroll);
    this.duration = drawdown > 0 ? this.duration + 1 : 0;
    const point = {
      ...pending, index: ++this.index,
      cumulativeProfit: pending.bankroll - this.initial,
      peakBankroll: this.peak, drawdown,
      drawdownRate: this.peak > 0 ? drawdown / this.peak : 0,
      drawdownDuration: this.duration,
    };
    this.last = point;
    this.addPoint(point);
    this.roundProfitMetric.add(pending.roundProfit);
    const roundNo = Number(pending.roundNo);
    if (Number.isSafeInteger(roundNo) && roundNo > 0) {
      this.roundCounts.set(roundNo, (this.roundCounts.get(roundNo) ?? 0) + 1);
    }
    for (const [betKey, betPending] of pending.byBet) {
      const trend = this.betTrends.get(betKey) ?? {
        stakeMetric: new BoundedMetric(),
        roundProfitMetric: new BoundedMetric(),
        roundCounts: new Map(),
      };
      trend.stakeMetric.add(betPending.stake);
      trend.roundProfitMetric.add(betPending.profit);
      if (Number.isSafeInteger(roundNo) && roundNo > 0) {
        trend.roundCounts.set(roundNo, (trend.roundCounts.get(roundNo) ?? 0) + 1);
      }
      this.betTrends.set(betKey, trend);
    }
    this.pending = null;
  }

  finish() { this.flush(); }

  trends() {
    const serialize = (trend) => ({
      stake_points: trend.stakeMetric.points(),
      round_profit_points: trend.roundProfitMetric.points(),
      round_distribution: [...trend.roundCounts.entries()]
        .sort(([left], [right]) => left - right)
        .map(([roundNo, count]) => ({ roundNo, count })),
      total_bets: trend.stakeMetric.index,
      betting_rounds: trend.roundProfitMetric.index,
    });
    return {
      stake_points: this.stakeMetric.points(),
      round_profit_points: this.roundProfitMetric.points(),
      round_distribution: [...this.roundCounts.entries()]
        .sort(([left], [right]) => left - right)
        .map(([roundNo, count]) => ({ roundNo, count })),
      total_bets: this.stakeMetric.index,
      betting_rounds: this.roundProfitMetric.index,
      by_bet: Object.fromEntries(
        [...this.betTrends.entries()].map(([betKey, trend]) => [betKey, serialize(trend)]),
      ),
    };
  }

  addPoint(point) {
    const key = Math.floor((point.index - 1) / this.width);
    const bucket = this.buckets.get(key);
    if (!bucket) {
      this.buckets.set(key, { low: point, high: point, drawdown: point, rate: point, duration: point });
    } else {
      if (point.bankroll < bucket.low.bankroll) bucket.low = point;
      if (point.bankroll > bucket.high.bankroll) bucket.high = point;
      if (point.drawdown > bucket.drawdown.drawdown) bucket.drawdown = point;
      if (point.drawdownRate > bucket.rate.drawdownRate) bucket.rate = point;
      if (point.drawdownDuration > bucket.duration.drawdownDuration) bucket.duration = point;
    }
    if (this.buckets.size > 1024) {
      const points = this.points();
      this.width *= 2;
      this.buckets.clear();
      for (const saved of points) if (saved.index > 0) this.addPoint(saved);
    }
  }

  points() {
    const unique = new Map([[0, this.first]]);
    for (const bucket of this.buckets.values()) {
      for (const point of Object.values(bucket)) unique.set(point.index, point);
    }
    if (this.last) unique.set(this.last.index, this.last);
    return [...unique.values()].sort((left, right) => left.index - right.index);
  }
}

/**
 * 任务结果进入 readyTasks 后仍占用槽位；只有从 ReplaySession 结算完才释放。
 */
async function runStreamPipeline({
  order, taskAt, taskCount, config, sessionCount, timestampOrder,
  parallel, requestedWorkerCount, runId, taskForSourceOrder, totalRoundCount,
  comparisonConfig,
}) {
  const session = new ReplaySession(streamConfigJson(config, sessionCount));
  const curve = new BoundedCurve(config.bankroll);
  // 两套资金状态独立，但消费完全相同的 prepared round；概率枚举只做一次。
  const comparisonSession = comparisonConfig
    ? new ReplaySession(streamConfigJson(comparisonConfig, sessionCount)) : null;
  const comparisonCurve = comparisonConfig ? new BoundedCurve(comparisonConfig.bankroll) : null;
  let primaryStopped = false;
  let comparisonStopped = false;
  let timelineOffset = 0;
  const targetOrder = order;
  let detailSequence = 0;
  let probabilityCompleted = 0;
  let settledRounds = 0;
  let lastProgressAt = 0;
  let lastOverall = 0;
  const timings = {
    generationMs: 0,
    prepareMs: 0,
    decodeMs: 0,
    settlementMs: 0,
    curveMs: 0,
    serializeMs: 0,
    workerProbeMs: 0,
    preparedBytes: 0,
    settlementBatches: 0,
    selectedWorkerCount: 1,
  };
  const progressStep = Math.max(1, Math.ceil(taskCount / 100));
  const progressTotal = Math.max(1, Number(totalRoundCount) || taskCount);
  const staticPoolSize = parallel ? workerCountFor(taskCount, requestedWorkerCount) : 1;
  const postPipelineProgress = (phase, completed, total, workerCount, isParallel, force = false) => {
    const now = performance.now();
    if (!force && now - lastProgressAt < PROGRESS_THROTTLE_MS
        && completed !== total) return;
    lastProgressAt = now;
    const probabilityRatio = taskCount > 0
      ? Math.min(1, probabilityCompleted / taskCount) : 1;
    const settlementRatio = progressTotal > 0
      ? Math.min(1, settledRounds / progressTotal) : 0;
    const calculatedOverall = Math.min(0.98, 0.05 + probabilityRatio * 0.55 + settlementRatio * 0.38);
    // 并行任务的完成消息不是严格按派发顺序到达；Worker 自己也保证单调，
    // 主线程即使收到交错阶段消息，也不会把进度条写回较小百分比。
    const overall = Math.max(lastOverall, calculatedOverall);
    lastOverall = overall;
    self.postMessage({
      type: "progress", phase, completed, total, workerCount, parallel: isParallel,
      overall, settledRounds, settlementTotal: progressTotal,
    });
  };
  const postProbabilityProgress = (completed, workerCount, isParallel, force = false) => {
    if (!force && completed !== 0 && completed !== taskCount && completed % progressStep !== 0) return;
    probabilityCompleted = completed;
    postPipelineProgress("probability", completed, taskCount, workerCount, isParallel, force);
  };

  const processEntries = (entries) => {
    for (let offset = 0; offset < entries.length; offset += REPLAY_BATCH_SIZE) {
      const batch = entries.slice(offset, offset + REPLAY_BATCH_SIZE);
      // 横轴用本次回放中的真实局序，而非各策略各自的“第几次下注”。
      // 因此 A 跳过某局而 B 下注时，两条线仍落在同一时间位置。
      const timelinePositions = new Map(batch.map((entry, index) => {
        const source = (entry.packet ?? entry).source;
        return [[source.table_id, source.session_id, source.round_no, source.started_at].join("|"), timelineOffset + index + 1];
      }));
      timelineOffset += batch.length;
      const serializeStarted = performance.now();
      const payload = JSON.stringify(batch.map((entry) => entry.packet ?? entry));
      timings.serializeMs += performance.now() - serializeStarted;
      const settleStarted = performance.now();
      const response = primaryStopped ? null : JSON.parse(session.push(payload));
      const comparisonResponse = comparisonSession && !comparisonStopped
        ? JSON.parse(comparisonSession.push(payload)) : null;
      timings.settlementMs += performance.now() - settleStarted;
      timings.settlementBatches += 1;
      settledRounds = Math.max(
        settledRounds,
        Number(response?.summary?.replayed_rounds ?? 0),
        Number(comparisonResponse?.summary?.replayed_rounds ?? 0),
      );
      postPipelineProgress(
        "settlement", settledRounds, progressTotal,
        localOnly ? 1 : poolSize, !localOnly,
      );
      const bets = Array.isArray(response?.bets) ? response.bets : [];
      if (bets.length) {
        const curveStarted = performance.now();
        curve.addBets(bets, timelinePositions);
        timings.curveMs += performance.now() - curveStarted;
        self.postMessage({ type: "detail-batch", runId, batchId: detailSequence++, details: bets });
      }
      const comparisonBets = Array.isArray(comparisonResponse?.bets) ? comparisonResponse.bets : [];
      if (comparisonBets.length) comparisonCurve.addBets(comparisonBets, timelinePositions);
      primaryStopped ||= Boolean(response?.summary?.stopped_early);
      comparisonStopped ||= Boolean(comparisonResponse?.summary?.stopped_early);
      if (primaryStopped && (!comparisonSession || comparisonStopped)) return true;
    }
    return false;
  };

  const limitsJson = preparationSideBetRoundLimitsJson(config, comparisonConfig);
  let prefetchedTask = null;
  let prefetchedPreparedJson = null;
  let prefetchedEntries = null;
  const getTask = async (taskId) => {
    if (taskId === 0 && prefetchedTask) {
      const task = prefetchedTask;
      prefetchedTask = null;
      return task;
    }
    const started = performance.now();
    const task = await taskAt(taskId);
    timings.generationMs += performance.now() - started;
    return task;
  };
  const prepareTaskJson = (task) => task.generatedRowsJson
      ? prepareGeneratedShoe(
        task.generatedRowsJson,
        config.decks,
        task.sourceBase ?? 0,
        limitsJson,
      )
      : (typeof prepareReplayShoeWithSideBetLimits === "function"
        ? prepareReplayShoeWithSideBetLimits(
          task.csvText, config.decks, timestampOrder, limitsJson,
        )
        : prepareReplayShoe(task.csvText, config.decks, timestampOrder));
  const prepareLocalTask = (taskId, task) => {
    if (taskId === 0 && prefetchedPreparedJson !== null) {
      const preparedJson = prefetchedPreparedJson;
      prefetchedPreparedJson = null;
      return preparedJson;
    }
    const started = performance.now();
    const preparedJson = prepareTaskJson(task);
    timings.prepareMs += performance.now() - started;
    // 这里使用 UTF-16 长度只做相对诊断，避免单线程路径为了统计字节数
    // 再把整个 JSON 编码一遍；并行路径使用实际 transferable 字节数。
    timings.preparedBytes += preparedJson.length;
    return preparedJson;
  };

  let poolSize = staticPoolSize;
  if (parallel
      && config.autoTuneWorkers === true
      && staticPoolSize > 1
      && taskCount >= 8) {
    // 先实际计算一副牌靴。结果会直接复用到后续流水线，不会因为测速再重复
    // 计算同一副牌靴；因此自动选择只增加一次预热，而不是额外复制整批任务。
    self.postMessage({
      type: "progress", phase: "worker-tune", completed: 0, total: 1,
      overall: 0.03, workerCount: staticPoolSize, parallel: true,
    });
    const probeStarted = performance.now();
    const generationStarted = performance.now();
    prefetchedTask = await taskAt(0);
    timings.generationMs += performance.now() - generationStarted;
    const prepareStarted = performance.now();
    prefetchedPreparedJson = prepareTaskJson(prefetchedTask);
    const prepareMilliseconds = performance.now() - prepareStarted;
    timings.prepareMs += prepareMilliseconds;
    timings.preparedBytes += prefetchedPreparedJson.length;
    const decodeStarted = performance.now();
    prefetchedEntries = decodePreparedJson(prefetchedPreparedJson);
    timings.decodeMs += performance.now() - decodeStarted;
    timings.workerProbeMs = performance.now() - probeStarted;
    poolSize = tunedWorkerCount(taskCount, staticPoolSize, prepareMilliseconds);
  }
  const localOnly = !parallel || poolSize === 1;
  timings.selectedWorkerCount = poolSize;

  postPipelineProgress("input", 0, progressTotal, localOnly ? 1 : poolSize, !localOnly, true);
  postProbabilityProgress(prefetchedEntries ? 1 : 0, localOnly ? 1 : poolSize, !localOnly, true);

  if (localOnly) {
    // 设备资源把有效并行数压到 1 时仍不能改变多桌时间线。这个分支先按牌靴
    // 计算，再按 source_order 收集快照；没有时间字段的精简 CSV 本来就是连续
    // 牌靴顺序，可以边算边结算，不需要额外缓存。
    const preparedBySourceOrder = targetOrder ? new Map() : null;
    let stopped = false;
    for (let taskId = 0; taskId < taskCount; taskId += 1) {
      const task = await getTask(taskId);
      const preparedJson = prepareLocalTask(taskId, task);
      const decodeStarted = performance.now();
      const entries = decodePreparedJson(preparedJson);
      timings.decodeMs += performance.now() - decodeStarted;
      if (preparedBySourceOrder) {
        for (const entry of entries) {
          if (preparedBySourceOrder.has(entry.sourceOrder)) {
            throw new Error("单线程预计算结果出现重复原始行");
          }
          preparedBySourceOrder.set(entry.sourceOrder, entry);
        }
      } else {
        stopped = processEntries(entries);
      }
      postProbabilityProgress(taskId + 1, 1, false);
      if (stopped) break;
    }
    if (preparedBySourceOrder) {
      for (let offset = 0; offset < targetOrder.length && !stopped; offset += REPLAY_BATCH_SIZE) {
        const entries = targetOrder.slice(offset, offset + REPLAY_BATCH_SIZE).map((sourceOrder) => {
          const entry = preparedBySourceOrder.get(sourceOrder);
          if (!entry) throw new Error("单线程预计算结果缺少原始行");
          return entry;
        });
        stopped = processEntries(entries);
      }
      preparedBySourceOrder.clear();
    }
  } else {
    const workers = [];
    const readyTasks = new Map();
    const packetsByOrder = new Map();
    const inFlight = new Map();
    const available = [];
    let nextTask = 0;
    let nextSequentialTask = 0;
    let nextOrder = 0;
    let completed = 0;
    let stopped = false;
    let pumping = false;
    let settled = false;
    // 百万靴回测只需要记住任务是否已派发。每靴一个字节的标记比保存
    // 百万个数字的 Set 更省内存，也减少长时间回测时的 GC 压力。
    const assignedTasks = new Uint8Array(taskCount);
    let assignedTaskCount = 0;
    const markAssigned = (taskId) => {
      if (assignedTasks[taskId] !== 0) throw new Error("并行任务编号无效或重复派发");
      assignedTasks[taskId] = 1;
      assignedTaskCount += 1;
    };

    const cleanup = () => {
      for (const worker of workers) worker.terminate();
      workers.length = 0;
    };

    const flush = async () => {
      if (stopped) return;
      if (!targetOrder) {
        while (readyTasks.has(nextSequentialTask)) {
          const entries = readyTasks.get(nextSequentialTask);
          readyTasks.delete(nextSequentialTask);
          stopped = processEntries(entries);
          nextSequentialTask += 1;
          if (stopped) return;
        }
        return;
      }
      while (!stopped && nextOrder < targetOrder.length) {
        const entries = [];
        while (nextOrder < targetOrder.length
            && packetsByOrder.has(targetOrder[nextOrder])
            && entries.length < REPLAY_BATCH_SIZE) {
          const packet = packetsByOrder.get(targetOrder[nextOrder]);
          packetsByOrder.delete(targetOrder[nextOrder]);
          entries.push(packet);
          const owner = readyTasks.get(packet.taskId);
          owner.remaining -= 1;
          if (owner.remaining === 0) readyTasks.delete(packet.taskId);
          nextOrder += 1;
        }
        if (!entries.length) return;
        stopped = processEntries(entries);
      }
    };

    if (prefetchedEntries) {
      // 自动测速已经完成第 0 副牌靴：把它放回与子 Worker 完全相同的
      // ready 队列，并标记为已派发，后面的 pump 会从第 1 副继续。
      markAssigned(0);
      completed = 1;
      if (!targetOrder) {
        readyTasks.set(0, prefetchedEntries);
      } else {
        readyTasks.set(0, { remaining: prefetchedEntries.length });
        for (const entry of prefetchedEntries) {
          if (packetsByOrder.has(entry.sourceOrder)) throw new Error("自动测速结果出现重复原始行");
          packetsByOrder.set(entry.sourceOrder, { ...entry, taskId: 0 });
        }
      }
      prefetchedEntries = null;
      prefetchedPreparedJson = null;
      prefetchedTask = null;
      postProbabilityProgress(completed, poolSize, true, true);
    }

    const pump = async () => {
      if (pumping || settled || stopped) return;
      pumping = true;
      try {
        while (!stopped && available.length && assignedTaskCount < taskCount) {
          // 带时间的多桌数据可能把不同牌靴交错排列。若全局下一局属于尚未
          // 派发的牌靴，即使普通背压名额已满，也必须先派发这一靴；否则前面
          // 牌靴的后续快照会占满 readyTasks，回放会永远等不到下一局。
          const requiredTaskId = targetOrder && nextOrder < targetOrder.length
            ? taskForSourceOrder?.get(targetOrder[nextOrder])
            : undefined;
          const requiredTaskMissing = Number.isInteger(requiredTaskId)
            && !assignedTasks[requiredTaskId];
          const hasCapacity = inFlight.size + readyTasks.size < poolSize * 2;
          if (!hasCapacity && !requiredTaskMissing) break;

          const worker = available.pop();
          let taskId;
          if (requiredTaskMissing) {
            // 时间线需要的任务可以跳过普通队列直接派发。nextTask 仍然指向
            // 第一个未派发的普通任务，之后再补齐被跳过的任务，避免任务丢失。
            taskId = requiredTaskId;
          } else {
            while (nextTask < taskCount && assignedTasks[nextTask]) nextTask += 1;
            if (nextTask >= taskCount) break;
            taskId = nextTask;
            nextTask += 1;
          }
          markAssigned(taskId);
          const task = await getTask(taskId);
          inFlight.set(worker, taskId);
          worker.postMessage(task.generatedRowsJson
            ? {
              type: "prepare-generated", taskId, decks: config.decks,
              sourceBase: task.sourceBase ?? 0,
              sideBetRoundLimitsJson: limitsJson,
              rowsJson: task.generatedRowsJson,
            }
            : {
              type: "prepare", stream: true, taskId, decks: config.decks,
              timestampOrder, sideBetRoundLimitsJson: limitsJson,
              csvText: task.csvText,
            });
        }
      } finally {
        pumping = false;
      }
    };

    const done = new Promise((resolve, reject) => {
      const check = async () => {
        await flush();
        await pump();
        if (stopped || (completed === taskCount && inFlight.size === 0 && readyTasks.size === 0
            && (!targetOrder || nextOrder === targetOrder.length))) {
          settled = true;
          cleanup();
          resolve();
        }
      };
      const onMessage = async (worker, message) => {
        if (settled) return;
        try {
          if (message.type === "error") throw new Error(message.message);
          if (message.type !== "complete" || message.taskId !== inFlight.get(worker)
              || !isArrayBuffer(message.preparedBuffer)) {
            throw new Error("并行牌靴 Worker 返回了无效结果");
          }
          const taskId = inFlight.get(worker);
          inFlight.delete(worker);
          available.push(worker);
          timings.prepareMs += Number(message.prepareMilliseconds ?? message.elapsedMilliseconds) || 0;
          timings.serializeMs += Number(message.serializeMilliseconds) || 0;
          timings.preparedBytes += Number(message.preparedBytes) || 0;
          const decodeStarted = performance.now();
          const entries = decodePreparedBuffer(message.preparedBuffer);
          timings.decodeMs += performance.now() - decodeStarted;
          if (!targetOrder) {
            readyTasks.set(taskId, entries);
          } else {
            readyTasks.set(taskId, { remaining: entries.length });
            for (const entry of entries) {
              if (packetsByOrder.has(entry.sourceOrder)) throw new Error("并行结果出现重复原始行");
              packetsByOrder.set(entry.sourceOrder, { ...entry, taskId });
            }
          }
          completed += 1;
          probabilityCompleted = completed;
          postProbabilityProgress(completed, poolSize, true);
          await check();
        } catch (error) {
          cleanup();
          reject(error);
        }
      };
      for (let index = 0; index < poolSize; index += 1) {
        let worker;
        try {
          worker = new Worker(new URL("./replay-shard-worker.js?v=32", import.meta.url), { type: "module" });
        } catch (error) {
          reject(error);
          return;
        }
        worker.addEventListener("message", (event) => { void onMessage(worker, event.data); });
        worker.addEventListener("error", (event) => reject(new Error(event.message || "并行 Worker 无法启动")));
        worker.addEventListener("messageerror", () => reject(new Error("并行结果传输失败")));
        workers.push(worker);
        available.push(worker);
      }
      void (async () => {
        try {
          await pump();
          await check();
        } catch (error) {
          cleanup();
          reject(error);
        }
      })();
    });
    try {
      await done;
    } finally {
      cleanup();
    }
  }

  const finishStarted = performance.now();
  const report = JSON.parse(session.finish());
  timings.settlementMs += performance.now() - finishStarted;
  report.bets = [];
  report.omitted_bet_details = 0;
  report.detail_count = Number(report.summary?.placed_bet_count ?? 0);
  report.run_id = runId;
  curve.finish();
  report.chart_points = curve.points();
  report.trend_charts = curve.trends();
  report.streamed = true;
  report.performance = timings;
  if (comparisonSession) {
    const comparisonReport = JSON.parse(comparisonSession.finish());
    comparisonCurve.finish();
    comparisonReport.bets = [];
    comparisonReport.detail_count = Number(comparisonReport.summary?.placed_bet_count ?? 0);
    comparisonReport.chart_points = comparisonCurve.points();
    comparisonReport.trend_charts = comparisonCurve.trends();
    comparisonReport.streamed = true;
    report.comparison = comparisonReport;
  }
  postPipelineProgress(
    "settlement", settledRounds, progressTotal,
    localOnly ? 1 : poolSize, !localOnly, true,
  );
  return { report, timings, workerCount: poolSize };
}

/** 兼容旧核心回放，并把它的完整明细转成和流式回放相同的页面协议。 */
function legacyComparisonPositions(primaryBets, comparisonBets) {
  const keyOf = (bet) => [bet.table_id, bet.session_id, bet.round_no, bet.started_at].join("|");
  const unique = new Map([...primaryBets, ...comparisonBets].map((bet) => [keyOf(bet), bet]));
  const bets = [...unique.values()];
  const csvLine = (bet) => /^CSV 第 (\d+) 行$/.exec(String(bet.started_at));
  const hasCsvLines = bets.length > 0 && bets.every(csvLine);
  bets.sort((left, right) => {
    if (hasCsvLines) return Number(csvLine(left)[1]) - Number(csvLine(right)[1]);
    return String(left.started_at).localeCompare(String(right.started_at))
      || Number(left.table_id) - Number(right.table_id)
      || Number(left.session_id) - Number(right.session_id)
      || Number(left.round_no) - Number(right.round_no);
  });
  return {
    positions: new Map(bets.map((bet, index) => [
      keyOf(bet), hasCsvLines ? Number(csvLine(bet)[1]) - 1 : index + 1,
    ])),
    mode: hasCsvLines ? "csv_row" : "bet_union",
    end: hasCsvLines ? Number(csvLine(bets.at(-1))?.[1] ?? 1) - 1 : bets.length,
  };
}

function externalizeLegacyReport(report, runId, postDetails = true, timelinePositions = null) {
  const bets = Array.isArray(report.bets) ? report.bets : [];
  const curve = new BoundedCurve(report.summary?.initial_bankroll ?? 0);
  curve.addBets(bets, timelinePositions);
  for (let offset = 0, batchId = 0; postDetails && offset < bets.length; offset += REPLAY_BATCH_SIZE, batchId += 1) {
    self.postMessage({
      type: "detail-batch", runId, batchId,
      details: bets.slice(offset, offset + REPLAY_BATCH_SIZE),
    });
  }
  report.bets = [];
  report.detail_count = bets.length;
  report.omitted_bet_details = 0;
  report.run_id = runId;
  curve.finish();
  report.chart_points = curve.points();
  report.trend_charts = curve.trends();
  report.streamed = false;
  return report;
}

// 兼容旧调试工具，验证合并概率 JSON 时不经过 JS Number。
function mergePreparedResults(results) {
  const parts = [];
  for (let index = 0; index < results.length; index += 1) {
    const json = new TextDecoder().decode(results[index]);
    results[index] = null;
    if (!json.startsWith('{"rounds":[') || !json.endsWith(']}')) {
      throw new Error("并行概率格式无效");
    }
    const rows = json.slice(11, -2);
    if (rows) parts.push(rows);
  }
  results.length = 0;
  return '{"rounds":[' + parts.join(",") + ']}';
}

/* ----------------------------- 入口 ----------------------------- */

const ready = init(new URL("./pkg/game_ev_engine_bg.wasm?v=32", import.meta.url));
ready.then(() => self.postMessage({ type: "ready" })).catch((error) => {
  self.postMessage({ type: "error", message: `无法加载 CSV 回放核心：${error?.message ?? String(error)}` });
});

let running = false;
self.addEventListener("message", async (event) => {
  if (!new Set(["replay", "simulate"]).has(event.data?.type) || running) return;
  running = true;
  const config = event.data.config ?? {};
  // B 有完整独立的资金、EV、赔付和边注限制配置；只有牌靴副数必须与 A
  // 相同，因为两者对照的是同一批真实牌局和同一条概率预计算流。
  const comparisonConfig = config.comparisonConfig
    ? {
      ...config,
      ...config.comparisonConfig,
      decks: config.decks,
    }
    : null;
  const runId = config.runId ?? `run-${Date.now()}`;
  try {
    await ready;
    const started = performance.now();
    let csvText = null;
    const csvFile = event.data.csvFile;
    let dataset;
    let quality;
    let taskAt;
    let taskCount;
    let sessionCount;
    let order = null;
    let taskForSourceOrder = null;
    let timestampOrder = false;
    const requestedWorkerCount = Number(config.parallelWorkerCount ?? 1);
    const requestedParallel = config.parallelReplay === true && requestedWorkerCount > 1;

    if (event.data.type === "simulate") {
      const { shoes, maxRoundsPerShoe, seed } = event.data.simulation;
      sessionCount = Number(shoes);
      taskCount = Number(shoes);
      const generator = new ShoeGenerator(shoes, maxRoundsPerShoe, seed, config.decks);
      taskAt = async (taskId) => {
        const rowsJson = generator.next();
        if (rowsJson === "null") throw new Error("随机牌靴生成器提前结束");
        // 直接把生成器的结构化 JSON 交给子 Worker；不再先拼 CSV，再由
        // 子 Worker 重新运行 CSV 读取器。sourceBase 用于恢复全局稳定行号。
        self.postMessage({
          type: "progress", phase: "generate", completed: taskId + 1,
          total: taskCount, overall: Math.min(0.15, 0.05 + (taskId + 1) / taskCount * 0.1),
        });
        return {
          generatedRowsJson: rowsJson,
          sourceBase: taskId * maxRoundsPerShoe,
        };
      };
      const totalRows = Number(shoes) * Number(maxRoundsPerShoe);
      dataset = {
        total_rows: totalRows, table_count: 1, session_count: sessionCount,
        business_date_count: 0, business_date_min: "", business_date_max: "",
        started_at_min: "", started_at_max: "", settled_at_min: "", settled_at_max: "",
        duplicate_source_pk_rows: 0, duplicate_round_keys: 0,
      };
      quality = {
        valid_card_rows: totalRows, empty_card_rows: 0, invalid_card_rows: 0,
        outcome_mismatch_rows: 0, sessions_starting_at_one: sessionCount,
        sessions_starting_mid_shoe: 0, sessions_with_round_gaps: 0,
        sessions_with_empty_cards: 0, sessions_with_invalid_rows: 0,
        fully_observable_sessions: sessionCount, quarantined_rounds: 0,
      };
      self.postMessage({
        type: "progress", phase: "generate", completed: 0, total: taskCount,
        overall: 0.02,
      });
    } else {
      if (typeof event.data.csvText === "string") {
        csvText = event.data.csvText;
      } else if (requestedParallel && typeof csvFile?.stream === "function") {
        // 精简 CSV 优先走 Blob 流：Worker 按行读取并拆成牌靴任务，主线程不会
        // 先把整份文件复制成字符串。带来源主键的完整数据库格式仍保留旧路径，
        // 因为全局重复键校验需要由 Rust 一次性完成。
        const split = await splitCsvBlobIntoShoeTasks(csvFile);
        if (split.requiresLegacy) {
          csvText = await csvFile.text();
        } else {
          taskAt = async (taskId) => serializeShoeTask(split.tasks[taskId]);
          taskCount = split.tasks.length;
          sessionCount = taskCount;
          order = split.order;
          taskForSourceOrder = split.order ? sourceOrderTaskMap(split.tasks) : null;
          timestampOrder = split.allHaveStartedAt;
          dataset = split.dataset;
          quality = split.quality;
        }
      }
      if (csvText === null) {
        csvText = csvFile?.text
          ? await csvFile.text()
          : new TextDecoder().decode(event.data.csvBuffer);
      }
      // 精简 CSV 不论是否开启并行都进入同一条流式流水线；单线程也能按
      // 牌靴和结算批次持续报告进度。只有带来源主键或牌靴交错的完整格式
      // 才保留 Rust 一次性回放路径。
      if (!taskAt) {
        const split = splitCsvIntoShoeTasks(csvText);
        if (!split.requiresLegacy) {
          taskAt = async (taskId) => split.tasks[taskId];
          taskCount = split.tasks.length;
          sessionCount = taskCount;
          order = split.order;
          taskForSourceOrder = split.order ? sourceOrderTaskMap(split.tasks) : null;
          timestampOrder = split.allHaveStartedAt;
          if (typeof inspectReplayShoe === "function") {
            const inspected = JSON.parse(inspectReplayShoe(csvText));
            dataset = inspected.dataset;
            quality = inspected.quality;
          }
        }
      }
    }

    let report;
    let performanceTimings = null;
    let pipelineFallbackReason = "";
    if (event.data.type === "replay" && !taskAt) {
      // 无法安全拆成独立牌靴任务的完整格式仍使用 Rust 原有的一次性路径；
      // 普通精简 CSV 已在上面进入流式流水线，即使单线程也能持续报告进度。
      self.postMessage({ type: "progress", phase: "serial" });
      const serialStarted = performance.now();
      const primaryRaw = JSON.parse(replayBaccaratCsvWithSideBetLimits(csvText, ...commonArguments(config)));
      const comparisonRaw = comparisonConfig
        ? JSON.parse(replayBaccaratCsvWithSideBetLimits(csvText, ...commonArguments(comparisonConfig))) : null;
      const timeline = comparisonRaw
        ? legacyComparisonPositions(primaryRaw.bets, comparisonRaw.bets) : null;
      report = externalizeLegacyReport(primaryRaw, runId, true, timeline?.positions);
      if (comparisonConfig) {
        report.comparison = externalizeLegacyReport(comparisonRaw, runId, false, timeline.positions);
        report.comparison.timeline_mode = timeline.mode;
        report.comparison.timeline_end = timeline.end;
      }
      performanceTimings = {
        generationMs: 0,
        prepareMs: 0,
        decodeMs: 0,
        settlementMs: performance.now() - serialStarted,
        curveMs: 0,
        serializeMs: 0,
        preparedBytes: 0,
        settlementBatches: 1,
      };
    } else {
      try {
        const pipelineResult = await runStreamPipeline({
          order, taskAt, taskCount, config, sessionCount, timestampOrder,
          parallel: requestedParallel, requestedWorkerCount, runId, taskForSourceOrder,
          totalRoundCount: dataset?.total_rows ?? taskCount,
          comparisonConfig,
        });
        report = pipelineResult.report;
        performanceTimings = pipelineResult.timings;
      } catch (error) {
        self.postMessage({ type: "detail-reset", runId });
        if (csvText === null && csvFile?.text) csvText = await csvFile.text();
        if (csvText === null) throw error;
        // 分片路径的质量摘要只是读取阶段的快速画像；如果分片计算失败，
        // 完整 Rust 回放会重新校验并生成权威摘要，不能再被旧画像覆盖。
        dataset = undefined;
        quality = undefined;
        const fallbackStarted = performance.now();
        const legacy = JSON.parse(replayBaccaratCsvWithSideBetLimits(csvText, ...commonArguments(config)));
        const comparisonRaw = comparisonConfig
          ? JSON.parse(replayBaccaratCsvWithSideBetLimits(csvText, ...commonArguments(comparisonConfig))) : null;
        const timeline = comparisonRaw
          ? legacyComparisonPositions(legacy.bets, comparisonRaw.bets) : null;
        report = externalizeLegacyReport(legacy, runId, true, timeline?.positions);
        if (comparisonConfig) {
          report.comparison = externalizeLegacyReport(comparisonRaw, runId, false, timeline.positions);
          report.comparison.timeline_mode = timeline.mode;
          report.comparison.timeline_end = timeline.end;
        }
        performanceTimings = {
          generationMs: 0,
          prepareMs: 0,
          decodeMs: 0,
          settlementMs: performance.now() - fallbackStarted,
          curveMs: 0,
          serializeMs: 0,
          preparedBytes: 0,
          settlementBatches: 1,
        };
        self.postMessage({
          type: "progress", phase: "serial",
          fallbackReason: `分批流水线失败，已从头单线程回放：${error?.message ?? String(error)}`,
        });
        pipelineFallbackReason = `分批流水线失败，已从头单线程回放：${error?.message ?? String(error)}`;
      }
    }

    if (dataset && quality) {
      report.dataset = dataset;
      report.quality = quality;
    }
    if (comparisonConfig && report.comparison) {
      report.primary_strategy = config.stakeStrategy;
      report.comparison.stake_strategy = comparisonConfig.stakeStrategy;
    }
    // “开启并行”不等于“本次实际创建了多个线程”：只有一副牌靴、单核设备
    // 或低内存设备可能把有效 Worker 数压到 1，页面应准确显示实际执行方式。
    // 自动测速选择的结果优先于静态资源上限，这样页面显示的是本次真正创建
    // 的子 Worker 数，而不是用户填写的理论上限。
    const effectiveWorkerCount = requestedParallel && taskCount > 0
      ? Number(performanceTimings?.selectedWorkerCount)
        || workerCountFor(taskCount, requestedWorkerCount) : 1;
    const usedParallel = requestedParallel && report.streamed === true && effectiveWorkerCount > 1;
    const fallbackReason = requestedParallel && !usedParallel
      ? pipelineFallbackReason
        || (effectiveWorkerCount <= 1 && taskCount <= 1
          ? "只有一副可回放牌靴，已使用单线程"
          : "设备资源限制为单线程")
      : "";
    const totalElapsed = performance.now() - started;
    performanceTimings ??= {};
    performanceTimings.totalMs = totalElapsed;
    performanceTimings.totalRows = Number(report.dataset?.total_rows ?? dataset?.total_rows ?? 0);
    performanceTimings.replayedRounds = Number(report.summary?.replayed_rounds ?? 0);
    performanceTimings.roundsPerSecond = performanceTimings.replayedRounds > 0
      ? performanceTimings.replayedRounds / (totalElapsed / 1000)
      : 0;
    report.performance = performanceTimings;
    self.postMessage({
      type: "complete", report,
      elapsedMilliseconds: totalElapsed,
      timings: performanceTimings,
      workerCount: usedParallel ? effectiveWorkerCount : 1,
      shardCount: taskCount,
      parallel: usedParallel,
      fallbackReason,
    });
  } catch (error) {
    self.postMessage({ type: "error", message: error?.message ?? String(error) });
  } finally {
    running = false;
  }
});
