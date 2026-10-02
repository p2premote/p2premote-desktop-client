// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // 构建脚本以 `--version` 校验产物内嵌版本：版本串可能被优化器拆进
    // 指令立即数，对二进制做字节扫描不可靠，必须运行取真实值。
    // 必须放在任何初始化之前，保证无窗口、无副作用、秒退。
    if std::env::args().skip(1).any(|arg| arg == "--version" || arg == "-V") {
        println!("p2premote {}", p2premote_lib::APP_VERSION);
        return;
    }

    #[cfg(target_os = "linux")]
    {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
    }

    p2premote_lib::run()
}
