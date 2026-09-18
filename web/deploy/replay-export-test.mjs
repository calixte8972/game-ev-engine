import assert from "node:assert/strict";

import { buildBetDetailExport, BET_DETAIL_EXPORT_COLUMNS } from "../replay-export.mjs";

const bets = [{
  started_at: "2026-09-18 12:00:00",
  table_id: 1,
  session_id: 42,
  round_no: 7,
  bet: "banker",
  outcome: "banker",
  result: "win",
  player_cards: "AS 10H",
  player_total: 1,
  banker_cards: "8C 2D",
  banker_total: 0,
  effective_ev: 0.0123,
  amount: 100,
  base_game_profit: 95,
  rebate_income: 0.9,
  actual_profit: 95.9,
  bankroll_after: 10_095.9,
}];

for (const format of ["csv", "tsv", "json", "xls"]) {
  const file = buildBetDetailExport(format, bets, { placed_bet_count: 1 }, new Date("2026-09-18T12:00:00Z"));
  assert.equal(file.count, 1);
  assert.match(file.filename, new RegExp(`\\.${format}$`));
  const contents = await file.blob.text();
  assert.ok(contents.length > 0);
  assert.ok(contents.includes("开局时间"));
}

const json = await buildBetDetailExport("json", bets).blob.text();
assert.deepEqual(JSON.parse(json).bets, bets);
const csvFile = buildBetDetailExport("csv", bets);
const csv = await csvFile.blob.text();
const csvBytes = new Uint8Array(await csvFile.blob.arrayBuffer());
assert.deepEqual([...csvBytes.slice(0, 3)], [0xef, 0xbb, 0xbf]);
assert.match(csv, /庄/);
assert.equal(BET_DETAIL_EXPORT_COLUMNS.length, 17);

console.log("PASS: bet detail CSV, TSV, JSON and Excel-compatible XLS exports");
