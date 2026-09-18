/**
 * 一次回测一个 IndexedDB。牌局索引、临时权重、完整下注明细落盘，内存只读小页。
 * 不持久化私密数据到服务器；存储失败必须中止，不能悄悄丢弃明细后显示成功。
 */
const PREFIX = "baccarat-replay-";
const requestValue = request => new Promise((resolve, reject) => {
  request.onsuccess = () => resolve(request.result);
  request.onerror = () => reject(request.error);
});
const committed = tx => new Promise((resolve, reject) => {
  tx.oncomplete = resolve;
  tx.onabort = () => reject(tx.error ?? new Error("本地存储事务被取消"));
  tx.onerror = () => {}; // 由 onabort 统一报告，避免重复拒绝。
});

export async function openReplayStore(runId) {
  if (!/^[a-zA-Z0-9-]+$/.test(runId)) throw new Error("无效的回测标识");
  const request = indexedDB.open(PREFIX + runId, 1);
  request.onupgradeneeded = () => {
    const db = request.result;
    const rows = db.createObjectStore("rows", { keyPath: "id" });
    rows.createIndex("shoe", "shoe");
    rows.createIndex("roundKey", "roundKey", { unique: true });
    db.createObjectStore("shoes", { keyPath: "shoe" });
    db.createObjectStore("sourceKeys");
    const timeline = db.createObjectStore("timeline", { keyPath: "id" });
    timeline.createIndex("time", "timeKey");
    db.createObjectStore("prepared", { keyPath: "id" });
    db.createObjectStore("details", { keyPath: "id" });
  };
  const db = await requestValue(request);
  db.onversionchange = () => db.close();
  return {
    close: () => db.close(),
    async stage(rows) {
      const tx = db.transaction(["rows", "shoes", "sourceKeys"], "readwrite");
      const done = committed(tx);
      for (const row of rows) {
        tx.objectStore("rows").add(row);
        tx.objectStore("shoes").put({ shoe: row.shoe });
        if (row.sourcePk) tx.objectStore("sourceKeys").add(true, row.sourcePk);
      }
      try { await done; } catch (e) {
        if (e?.name === "ConstraintError") throw new Error("CSV 存在重复来源主键或重复桌台/牌靴/局号，请先去重");
        throw e;
      }
    },
    async put(store, rows) {
      if (!rows.length) return;
      const tx = db.transaction(store, "readwrite"), done = committed(tx);
      // 同一事务内反复使用一个 ObjectStore 句柄。大回测的一批明细可达
      // 2048 笔，无需每笔重新从事务查询同一张表。
      const target = tx.objectStore(store);
      for (const row of rows) target.put(row);
      await done;
    },
    async get(store, key) {
      return requestValue(db.transaction(store).objectStore(store).get(key));
    },
    async getMany(store, keys) {
      const tx = db.transaction(store);
      return Promise.all(keys.map(key => requestValue(tx.objectStore(store).get(key))));
    },
    async shoeRows(shoe) {
      // 正常八副牌最多 104 局；给脏数据留诊断余量，但禁止单靴无限装入内存。
      const rows = await requestValue(db.transaction("rows").objectStore("rows")
        .index("shoe").getAll(IDBKeyRange.only(shoe), 417));
      if (rows.length > 416) throw new Error("单個牌靴超过 416 行，请检查牌靴编号是否重复使用");
      return rows;
    },
    async page(store, after, limit, index = null) {
      const target = db.transaction(store).objectStore(store);
      const source = index ? target.index(index) : target;
      return new Promise((resolve, reject) => {
        const rows = [];
        const request = source.openCursor(after == null ? null : IDBKeyRange.lowerBound(after, true));
        request.onerror = () => reject(request.error);
        request.onsuccess = () => {
          const cursor = request.result;
          if (!cursor || rows.length >= limit) { resolve(rows); return; }
          rows.push({ key: cursor.key, value: cursor.value });
          cursor.continue();
        };
      });
    },
    async remove(store, keys) {
      if (!keys.length) return;
      const tx = db.transaction(store, "readwrite"), done = committed(tx);
      for (const key of keys) tx.objectStore(store).delete(key);
      await done;
    },
    async clearTemporary() {
      // 回放流水线失败后会从头重跑；明细序号也会从 0 开始，旧批次必须一起清掉，
      // 否则新报告较短时，分页会读到上一次失败尝试留下的尾部行。
      const names = ["rows", "shoes", "sourceKeys", "timeline", "prepared", "details"];
      const tx = db.transaction(names, "readwrite"), done = committed(tx);
      for (const name of names) tx.objectStore(name).clear();
      await done;
    },
  };
}

export async function readReplayDetails(runId, offset, limit) {
  const store = await openReplayStore(runId);
  try {
    const after = offset > 0 ? offset - 1 : null;
    return (await store.page("details", after, limit)).map(row => row.value.bet ?? row.value);
  } finally { store.close(); }
}

/** 按数字明细序号读取一页；页面永远只创建当前页的 DOM 行。 */
export async function readReplayDetailPage(runId, offset, limit) {
  const store = await openReplayStore(runId);
  try {
    return (await store.page("details", offset === 0 ? null : offset - 1, limit))
      .map(row => row.value.bet ?? row.value);
  } finally { store.close(); }
}

/** 只删除明确属于本次应用、已经不再展示的单次回测，不扫描或清空其他数据库。 */
export async function deleteReplayStore(runId) {
  if (!/^[a-zA-Z0-9-]+$/.test(runId)) return;
  await requestValue(indexedDB.deleteDatabase(PREFIX + runId));
}
