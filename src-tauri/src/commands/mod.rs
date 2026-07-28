//! P2PRemote Tauri Commands Module
//! 对应原 Wails 的 Go 绑定方法

pub mod auth;
pub mod config;
pub mod device;
pub mod service;

/// 读取当前 UI 语言并生成面向用户的本地化消息。
///
/// src-tauri 进程与 service 进程独立，locale 缓存在 service 内存里；
/// 这里直接读持久化的 MachineConfig.locale（文件），避免每次 IPC 往返。
/// 读文件失败或 locale 未设置时按默认语言（zh-CN）处理。
pub(crate) fn localized(key: &str, params: &[(&str, &str)]) -> String {
    let locale = p2premote_core::config::load_machine_config()
        .ok()
        .and_then(|c| c.locale);
    p2premote_core::i18n::localized_message(locale.as_deref(), key, params)
}
