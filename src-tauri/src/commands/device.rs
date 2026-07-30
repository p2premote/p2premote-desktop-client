//! 设备管理命令模块（IPC 转发层）
//!
//! 所有需要服务器令牌的设备类命令统一通过 IPC 委托后台 service 处理
//! （service 持有令牌、负责续期）。本模块只做命令转发。
//!
//! 本地能力（系统信息）见 local_diagnostics.rs。

use tracing::{debug, error, info};

use crate::commands::service::send_command_responsive;
use p2premote_core::control::Data;

/// 将 service 响应转为 Result：ok=true → Ok(())；ok=false → Err(message)；
/// 非 CommandResponse（意外响应）→ Err 并记录日志（与 get_device_list/register 等一致，
/// 避免格式错误的响应被静默当成功）。
fn ensure_ok(resp: &Data) -> Result<(), String> {
    match resp {
        Data::CommandResponse { ok: true, .. } => Ok(()),
        Data::CommandResponse {
            ok: false, message, ..
        } => Err(message.clone()),
        other => {
            error!("[device] unexpected service response: {:?}", other);
            Err("unexpected service response".to_string())
        }
    }
}

/// Tauri 命令：获取设备列表
#[tauri::command]
pub async fn get_device_list() -> serde_json::Value {
    debug!("[DeviceList] 通过 service 获取设备列表");
    match send_command_responsive(Data::GetDeviceList).await {
        Ok(Data::CommandResponse {
            ok: true,
            data: Some(d),
            ..
        }) => {
            info!("[DeviceList] 设备列表获取成功");
            d
        }
        Ok(Data::CommandResponse {
            ok: false, message, ..
        }) => {
            error!("[DeviceList] service 返回失败: {}", message);
            serde_json::json!({"code": 500, "msg": message, "data": []})
        }
        Ok(other) => {
            error!("[DeviceList] 意外的 service 响应: {:?}", other);
            serde_json::json!({"code": 500, "msg": "unexpected service response", "data": []})
        }
        Err(e) => {
            error!("[DeviceList] IPC 失败: {}", e);
            serde_json::json!({"code": 500, "msg": e, "data": []})
        }
    }
}

/// Tauri 命令：更新设备别名
#[tauri::command]
pub async fn update_device_alias(device_id: i64, alias: String) -> Result<(), String> {
    debug!(
        "Update device alias via service: {} -> {}",
        device_id, alias
    );
    let resp = send_command_responsive(Data::UpdateDeviceAlias { device_id, alias }).await?;
    ensure_ok(&resp)
}

/// Tauri 命令：删除设备
#[tauri::command]
pub async fn delete_device(device_id: i64) -> Result<(), String> {
    debug!("Delete device via service: {}", device_id);
    let resp = send_command_responsive(Data::DeleteDevice { device_id }).await?;
    ensure_ok(&resp)
}

// 注：设备状态上报统一由后台 service 负责，启动/变化时上报并每 5 分钟兜底。

/// Tauri 命令：标记当前设备为离线
#[tauri::command]
pub async fn mark_current_device_offline() -> Result<(), String> {
    info!("Mark current device offline via service");
    let resp = send_command_responsive(Data::MarkCurrentDeviceOffline).await?;
    ensure_ok(&resp)
}

/// Tauri 命令：设置设备连接密码
#[tauri::command]
pub async fn set_device_password(device_id: i64, password: String) -> Result<(), String> {
    debug!("Set device password via service: device_id={}", device_id);
    let resp = send_command_responsive(Data::SetDevicePassword {
        device_id,
        password,
    })
    .await?;
    ensure_ok(&resp)
}

/// Tauri 命令：生成本设备连接码
#[tauri::command]
pub async fn generate_connect_code(device_id: i64) -> Result<String, String> {
    debug!("Generate connect code via service: device_id={}", device_id);
    let resp = send_command_responsive(Data::GenerateConnectCode { device_id }).await?;
    match resp {
        Data::CommandResponse {
            ok: true,
            data: Some(d),
            ..
        } => d
            .get("connect_code")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| crate::commands::localized("errors.no_connect_code_data", &[])),
        Data::CommandResponse {
            ok: false, message, ..
        } => Err(message),
        other => Err(format!("unexpected service response: {:?}", other)),
    }
}
