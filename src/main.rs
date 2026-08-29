//! 命令行可执行程序入口。
//!
//! 核心计算都在库中，本文件只负责连接以下步骤：
//!
//! `读取参数 -> 解析参数 -> 构造牌靴 -> 调用分析 API -> 打印结果`

use game_ev_engine::cli;
use game_ev_engine::{BetMetrics, MainBetAnalysis, MainBetRules, analyze_main_bets};

/// 二进制程序入口。
///
/// 实际计算逻辑放在 `lib.rs` 暴露的库中；二进制程序只读取输入和展示结果。
fn main() {
    // env::args() 的第一项是程序自身路径。skip(1) 跳过它，collect() 把后续
    // 迭代器内容收集成 Vec<String>，方便 parse_args 按下标读取。
    let args: Vec<String> = std::env::args().skip(1).collect();

    // 每一层 match 对应一个可能失败的边界：参数、牌靴、概率分析。
    // 成功值位于 Ok(...)，失败值位于 Err(...)。
    match cli::parse_args(&args) {
        Ok(cli::Command::Analyze(input)) => match cli::build_shoe(&input) {
            Ok(shoe) => match analyze_main_bets(&shoe, MainBetRules::standard()) {
                Ok(analysis) => {
                    print_analysis(analysis);
                }
                Err(error) => {
                    eprintln!("概率计算失败：{error}");
                    // 非零退出码表示程序没有正常完成，便于脚本和 Python 检测失败。
                    std::process::exit(2);
                }
            },
            Err(error) => {
                eprintln!("构造牌靴失败：{error:?}");
                std::process::exit(2);
            }
        },
        Err(error) => {
            eprintln!("参数错误：{error:?}");
            std::process::exit(2);
        }
    }
}

/// 打印一个下注方向的概率、EV、赌场优势和 RTP。
///
/// 抽成独立函数后，Player、Banker、Tie 可以复用同一套输出格式。
fn print_metrics(name: &str, metrics: BetMetrics) {
    println!("{name}:");
    println!("  概率：{:.6}", metrics.probability());
    println!("  EV：{:.6}", metrics.ev());
    println!("  House Edge：{:.6}", metrics.house_edge());
    println!("  RTP：{:.6}", metrics.rtp());
    println!();
}

/// 按固定顺序打印一份完整主注分析。
fn print_analysis(analysis: MainBetAnalysis) {
    println!("=== 分析结果 ===");
    println!();

    print_metrics("Player", analysis.player());
    print_metrics("Banker", analysis.banker());
    print_metrics("Tie", analysis.tie());

    println!("最优下注：{}", analysis.optimal_bet().as_str());
    println!("最优 EV：{:.6}", analysis.optimal_ev());
}
