import { createApp } from 'vue'
import { createPinia } from 'pinia'
import { ElAlert } from 'element-plus/es/components/alert/index.mjs'
import { ElButton } from 'element-plus/es/components/button/index.mjs'
import { ElCard } from 'element-plus/es/components/card/index.mjs'
import { ElCheckbox } from 'element-plus/es/components/checkbox/index.mjs'
import { ElConfigProvider } from 'element-plus/es/components/config-provider/index.mjs'
import { ElContainer, ElHeader, ElMain } from 'element-plus/es/components/container/index.mjs'
import { ElDialog } from 'element-plus/es/components/dialog/index.mjs'
import { ElDropdown, ElDropdownItem, ElDropdownMenu } from 'element-plus/es/components/dropdown/index.mjs'
import { ElEmpty } from 'element-plus/es/components/empty/index.mjs'
import { ElForm, ElFormItem } from 'element-plus/es/components/form/index.mjs'
import { ElIcon } from 'element-plus/es/components/icon/index.mjs'
import { ElInput } from 'element-plus/es/components/input/index.mjs'
import { ElLoading } from 'element-plus/es/components/loading/index.mjs'
import { ElOption, ElSelect } from 'element-plus/es/components/select/index.mjs'
import { ElProgress } from 'element-plus/es/components/progress/index.mjs'
import { ElRadio, ElRadioGroup } from 'element-plus/es/components/radio/index.mjs'
import { ElSwitch } from 'element-plus/es/components/switch/index.mjs'
import { ElTag } from 'element-plus/es/components/tag/index.mjs'
import { ElTabs, ElTabPane } from 'element-plus/es/components/tabs/index.mjs'
import { ElTooltip } from 'element-plus/es/components/tooltip/index.mjs'
import 'element-plus/dist/index.css'
// Fluent 主题必须在 element-plus css 之后引入，确保 CSS 变量覆盖生效
import './assets/styles/fluent-theme.css'

import i18n from './i18n'
import App from './App.vue'
import router from './router'

const app = createApp(App)

// 仅注册模板实际使用的 Element Plus 组件，避免整库进入首屏包。
for (const component of [
  ElAlert,
  ElButton,
  ElCard,
  ElCheckbox,
  ElConfigProvider,
  ElContainer,
  ElDialog,
  ElDropdown,
  ElDropdownItem,
  ElDropdownMenu,
  ElEmpty,
  ElForm,
  ElFormItem,
  ElHeader,
  ElIcon,
  ElInput,
  ElMain,
  ElOption,
  ElProgress,
  ElRadio,
  ElRadioGroup,
  ElSelect,
  ElSwitch,
  ElTag,
  ElTabs,
  ElTabPane,
  ElTooltip,
]) {
  app.use(component)
}

app.use(createPinia())
app.use(router)
// i18n 必须在 ElementPlus 之前注册，确保组件内可用 useI18n/$t
app.use(i18n)
app.use(ElLoading)

app.mount('#app')
