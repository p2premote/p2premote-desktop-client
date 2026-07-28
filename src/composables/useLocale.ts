import { computed } from 'vue'
import i18n, { LOCALE_STORAGE_KEY } from '../i18n'
import type { AppLocale } from '../i18n'
import { invoke } from '../runtime/bridge'

/// 语言切换 composable。
///
/// 职责：
///   1. 提供响应式的当前语言（computed，绑定 i18n.global.locale）
///   2. 切换语言时同步三处：i18n 实例、localStorage、后端 MachineConfig（via set_locale）
///
/// 后端同步说明：set_locale command 由批次 3 实现（Data::SetLocale → service
/// 持久化 + RuntimeStatus.locale 缓存）。失败静默：service 未启动或 IPC 不通
/// 时不阻塞前端语言切换。
export function useLocale() {
  const locale = computed<AppLocale>({
    get: () => i18n.global.locale.value as AppLocale,
    set: (val) => {
      i18n.global.locale.value = val
    },
  })

  /// 同步语言偏好到后端 service 进程（让 Rust 侧 message 也能本地化）。
  /// 失败静默：service 未启动或 IPC 不通时不阻塞前端。
  async function syncLocaleToBackend(target: AppLocale): Promise<void> {
    try {
      await invoke('set_locale', { locale: target })
    } catch {
      // service 未启动或 IPC 不通，前端独立工作，忽略
    }
  }

  /// 切换语言。立即生效（前端），后端异步同步。
  async function setLocale(target: AppLocale): Promise<void> {
    locale.value = target
    try {
      localStorage.setItem(LOCALE_STORAGE_KEY, target)
    } catch {
      // localStorage 不可用，忽略
    }
    await syncLocaleToBackend(target)
  }

  return {
    locale,
    setLocale,
  }
}
