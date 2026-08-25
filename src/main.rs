/// 二进制程序入口。
///
/// 当前 CLI 尚未实现，所以这里只显示工程已经成功启动；实际计算逻辑放在
/// `lib.rs` 暴露的库中，后续 CLI 只负责读取输入和展示结果。
fn main() {
    println!("{}：Cargo 工程初始化完成。", game_ev_engine::APP_NAME);
}
