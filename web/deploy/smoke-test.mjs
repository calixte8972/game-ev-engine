import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  analyzeBaccaratStrategy,
  initSync,
  replayBaccaratCsv,
} from "../pkg/game_ev_engine.js";

const deployDirectory = dirname(fileURLToPath(import.meta.url));
const webDirectory = resolve(deployDirectory, "..");
const wasmBytes = readFileSync(resolve(webDirectory, "pkg/game_ev_engine_bg.wasm"));
initSync({ module: wasmBytes });

const manual = JSON.parse(
  analyzeBaccaratStrategy("consumed", 8, "", 0.009, 0, 10_000, 0.05, 500, 10_000),
);

if (manual.remaining_card_count !== 416) {
  throw new Error("完整八副牌的剩余张数不正确");
}

const tinyCsv = `__source_pk,table_id,session_id,round_no,started_at,settled_at,raw_cards,result_code
a,1,9001,1,2026-08-20 00:00:12,2026-08-20 00:00:44,"b:24,31,45;p:31,42,47",36
b,1,9001,2,2026-08-20 00:00:54,2026-08-20 00:01:17,"b:73,62,;p:53,8,",322
`;
const tinyReplay = JSON.parse(
  replayBaccaratCsv(tinyCsv, 8, 0.02, 0, 10_000, 0.05, 1_000, 1_000),
);

if (tinyReplay.summary.replayed_rounds !== 2) {
  throw new Error("WASM CSV 回放没有完成两局测试数据");
}

const output = {
  manual: {
    remaining_cards: manual.remaining_card_count,
    candidate: manual.recommendation.candidate_bet,
    action: manual.recommendation.action,
    amount: manual.recommendation.suggested_amount,
  },
  tiny_replay: {
    rounds: tinyReplay.summary.replayed_rounds,
    bets: tinyReplay.summary.placed_bet_count,
    final_bankroll: tinyReplay.summary.final_bankroll,
  },
};

const csvPath = process.argv[2];
if (csvPath) {
  const csvText = readFileSync(resolve(csvPath), "utf8");
  const started = performance.now();
  const replay = JSON.parse(
    replayBaccaratCsv(csvText, 8, 0.009, 0, 10_000, 0.05, 500, 10_000),
  );
  output.full_replay = {
    elapsed_seconds: (performance.now() - started) / 1_000,
    rows: replay.dataset.total_rows,
    complete_sessions: replay.quality.fully_observable_sessions,
    replayed_rounds: replay.summary.replayed_rounds,
    placed_bets: replay.summary.placed_bet_count,
    total_profit: replay.summary.total_profit,
    final_bankroll: replay.summary.final_bankroll,
    omitted_bet_details: replay.omitted_bet_details,
  };
}

console.log(JSON.stringify(output, null, 2));
