import { createI18n } from 'vue-i18n'
import zhCN from './messages/zh-CN'
import en from './messages/en'

/// localStorage key：保存用户语言偏好。
export const LOCALE_STORAGE_KEY = 'p2premote:locale'

/// 支持的语言列表（语言代码 → 显示名，用于语言切换器）。
export const SUPPORTED_LOCALES = [
  { value: 'zh-CN', label: '简体中文' },
  { value: 'en', label: 'English' },
] as const

export type AppLocale = (typeof SUPPORTED_LOCALES)[number]['value']

/// 探测系统语言并映射到支持的语言代码。
/// 中文系（zh-*）→ zh-CN；其余 → en。
export function detectSystemLocale(): AppLocale {
  const lang = (navigator.language || 'zh-CN').toLowerCase()
  if (lang.startsWith('zh')) return 'zh-CN'
  return 'en'
}

/// 从 localStorage 读取已保存的语言偏好；无则返回 null。
export function readStoredLocale(): AppLocale | null {
  try {
    const stored = localStorage.getItem(LOCALE_STORAGE_KEY)
    if (stored === 'zh-CN' || stored === 'en') return stored
  } catch {
    // localStorage 不可用（如隐私模式），忽略
  }
  return null
}

/// 读取初始语言：已保存的偏好 > 系统语言探测。
export function resolveInitialLocale(): AppLocale {
  return readStoredLocale() ?? detectSystemLocale()
}

const i18n = createI18n({
  legacy: false, // 启用 Composition API 模式（与 Vue 3 配合）
  locale: resolveInitialLocale(),
  fallbackLocale: 'zh-CN',
  messages: {
    'zh-CN': zhCN,
    en,
  },
})

export default i18n
