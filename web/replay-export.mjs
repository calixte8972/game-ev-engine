const betLabels = {
  player: "闲",
  banker: "庄",
  tie: "和",
  any_pair: "任意对子",
  banker_pair: "庄对",
  player_pair: "闲对",
  perfect_pair: "完美对子",
  big: "大",
  small: "小",
  lucky_six: "幸运 6",
  lucky_seven: "幸运 7",
  super_lucky_seven: "超级幸运 7",
  banker_dragon_bonus: "庄龙宝",
  player_dragon_bonus: "闲龙宝",
};

const resultLabels = { win: "赢", loss: "输", push: "和局退回" };

export const BET_DETAIL_EXPORT_COLUMNS = Object.freeze([
  ["started_at", "开局时间"], ["table_id", "桌台"], ["session_id", "牌靴"],
  ["round_no", "局号"], ["bet", "下注"], ["outcome", "结果方"],
  ["result", "结算"], ["player_cards", "闲牌"], ["player_total", "闲点数"],
  ["banker_cards", "庄牌"], ["banker_total", "庄点数"], ["effective_ev", "有效 EV"],
  ["amount", "金额"], ["base_game_profit", "基础输赢"], ["rebate_income", "返水"],
  ["actual_profit", "本局净输赢"], ["bankroll_after", "结算后本金"],
]);

const numberValue = value => value == null || value === "" ? "" : String(value);

export function betDetailRow(bet) {
  return [
    bet?.started_at ?? "", numberValue(bet?.table_id), numberValue(bet?.session_id),
    numberValue(bet?.round_no), betLabels[bet?.bet] ?? bet?.bet ?? "",
    betLabels[bet?.outcome] ?? bet?.outcome ?? "", resultLabels[bet?.result] ?? bet?.result ?? "",
    bet?.player_cards ?? "", numberValue(bet?.player_total), bet?.banker_cards ?? "",
    numberValue(bet?.banker_total), numberValue(bet?.effective_ev), numberValue(bet?.amount),
    numberValue(bet?.base_game_profit), numberValue(bet?.rebate_income),
    numberValue(bet?.actual_profit), numberValue(bet?.bankroll_after),
  ];
}

function escapeDelimited(value, delimiter) {
  const text = value == null ? "" : String(value);
  return /["\r\n]/.test(text) || text.includes(delimiter)
    ? `"${text.replaceAll('"', '""')}"` : text;
}

function escapeHtml(value) {
  return String(value ?? "")
    .replaceAll("&", "&amp;").replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;").replaceAll('"', "&quot;").replaceAll("'", "&#39;");
}

function htmlTable(bets) {
  const headers = BET_DETAIL_EXPORT_COLUMNS
    .map(([, label]) => `<th>${escapeHtml(label)}</th>`).join("");
  const rows = bets.map(bet => `<tr>${betDetailRow(bet)
    .map(value => `<td>${escapeHtml(value)}</td>`).join("")}</tr>`).join("");
  return `\uFEFF<!DOCTYPE html><html><head><meta charset="utf-8"><style>`
    + "table{border-collapse:collapse}th,td{border:1px solid #cbd5d1;padding:4px 8px;white-space:nowrap}th{background:#e8f2ed}"
    + `</style></head><body><table><thead><tr>${headers}</tr></thead><tbody>${rows}</tbody></table></body></html>`;
}

export const EXPORT_FORMATS = Object.freeze({
  csv: { label: "CSV（Excel 可打开）", extension: "csv", mime: "text/csv;charset=utf-8" },
  tsv: { label: "TSV（制表符）", extension: "tsv", mime: "text/tab-separated-values;charset=utf-8" },
  json: { label: "JSON（原始字段）", extension: "json", mime: "application/json;charset=utf-8" },
  xls: { label: "Excel 网页表格（.xls）", extension: "xls", mime: "application/vnd.ms-excel;charset=utf-8" },
});

export function buildBetDetailExport(format, bets, summary = {}, now = new Date()) {
  const definition = EXPORT_FORMATS[format];
  if (!definition) throw new Error(`不支持的导出格式：${format}`);
  const rows = Array.isArray(bets) ? bets : [];
  let contents;

  if (format === "json") {
    contents = JSON.stringify({
      exported_at: now.toISOString(), summary,
      columns: Object.fromEntries(BET_DETAIL_EXPORT_COLUMNS), bets: rows,
    }, null, 2);
  } else if (format === "xls") {
    contents = htmlTable(rows);
  } else {
    const delimiter = format === "tsv" ? "\t" : ",";
    const header = BET_DETAIL_EXPORT_COLUMNS
      .map(([, label]) => escapeDelimited(label, delimiter)).join(delimiter);
    const body = rows.map(bet => betDetailRow(bet)
      .map(value => escapeDelimited(value, delimiter)).join(delimiter)).join("\r\n");
    contents = `\uFEFF${header}${body ? `\r\n${body}` : ""}\r\n`;
  }

  return {
    blob: new Blob([contents], { type: definition.mime }),
    filename: `baccarat-bet-details-${now.toISOString().slice(0, 19).replaceAll(/[:T]/g, "-")}.${definition.extension}`,
    count: rows.length,
  };
}
