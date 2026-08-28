use game_ev_engine::cli;
use game_ev_engine::{BetMetrics, MainBetAnalysis, MainBetRules, analyze_main_bets};

/// 二进制程序入口。
///
/// 当前 CLI 尚未实现，所以这里只显示工程已经成功启动；实际计算逻辑放在
/// `lib.rs` 暴露的库中，后续 CLI 只负责读取输入和展示结果。

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match cli::parse_args(&args) {
        Ok(cli::Command::Analyze(input)) => match cli::build_shoe(&input) {
            Ok(shoe) => match analyze_main_bets(&shoe, MainBetRules::standard()) {
                Ok(analysis) => {
                    print_analysis(analysis);
                }
                Err(error) => {
                    eprintln!("概率计算失败：{error}");
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
fn print_metrics(name: &str, metrics: BetMetrics) {
    println!("{name}:");
    println!("  概率：{:.6}", metrics.probability());
    println!("  EV：{:.6}", metrics.ev());
    println!("  House Edge：{:.6}", metrics.house_edge());
    println!("  RTP：{:.6}", metrics.rtp());
    println!();
}
fn print_analysis(analysis: MainBetAnalysis) {
    println!("=== 分析结果 ===");
    println!();

    print_metrics("Player", analysis.player());
    print_metrics("Banker", analysis.banker());
    print_metrics("Tie", analysis.tie());

    println!("最优下注：{}", analysis.optimal_bet().as_str());
    println!("最优 EV：{:.6}", analysis.optimal_ev());
}
