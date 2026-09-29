<template>
  <div class="login-container">
    <el-card class="login-card fluent-card">
      <template #header>
        <div class="card-header">
          <AppLogo :size="36" />
          <div class="header-text">
            <h2>p2pRemote</h2>
            <p>{{ modeTitle }}</p>
          </div>
        </div>
      </template>

      <el-form
        ref="loginFormRef"
        :model="loginForm"
        :rules="loginRules"
        @submit.prevent="handleLogin"
      >
        <el-form-item prop="identifier">
          <el-input
            v-model="loginForm.identifier"
            :placeholder="$t('login.username_placeholder')"
            prefix-icon="User"
          />
        </el-form-item>

        <el-form-item prop="password">
          <el-input
            v-model="loginForm.password"
            :type="isSavedPasswordSentinel ? 'text' : 'password'"
            :placeholder="$t('login.password_placeholder')"
            prefix-icon="Lock"
            :show-password="!isSavedPasswordSentinel"
            :class="{ 'saved-password-sentinel': isSavedPasswordSentinel }"
            @focus="clearSavedPasswordSentinel"
            @blur="restoreSavedPasswordSentinel"
          />
        </el-form-item>

        <el-form-item>
          <el-checkbox v-model="loginForm.rememberMe">{{ $t('login.remember_password') }}</el-checkbox>
          <el-checkbox
            v-model="loginForm.autoLogin"
            :disabled="!loginForm.rememberMe"
            style="margin-left: 16px;"
          >
            {{ $t('login.auto_login') }}
          </el-checkbox>
        </el-form-item>

        <el-form-item>
          <el-button type="primary" :loading="loading" native-type="submit" style="width: 100%">
            {{ $t('login.submit') }}
          </el-button>
        </el-form-item>
      </el-form>


      <div class="footer">
        <router-link to="/register">{{ $t('login.no_account') }}</router-link>
        <span class="separator">|</span>
        <router-link to="/reset-password">{{ $t('login.forgot_password') }}</router-link>
      </div>
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type { FormInstance, FormRules } from 'element-plus/es/components/form/index.mjs'
import { invoke, openClientDownloadPage } from '../runtime/bridge'
import type { UpdateCheckResponse } from '../runtime/bridge'
import { useRouter } from 'vue-router'
import { ElMessage } from 'element-plus/es/components/message/index.mjs'
import { useAuthStore } from '../stores/auth'
import AppLogo from '../components/AppLogo.vue'
const { t } = useI18n()

const router = useRouter()
const authStore = useAuthStore()

const loginFormRef = ref<FormInstance>()
const loading = ref(false)
const hasSavedCredential = ref(false)
const SAVED_PASSWORD_SENTINEL = '●'.repeat(12)

const loginForm = reactive({
  identifier: '',
  password: '',
  rememberMe: false,
  autoLogin: false
})

const modeTitle = computed(() => t('login.subtitle'))

const isSavedPasswordSentinel = computed(() => loginForm.password === SAVED_PASSWORD_SENTINEL)

const loginRules = computed<FormRules>(() => ({
  identifier: [{ required: true, message: t('login.validation.username_required'), trigger: 'blur' }],
  password: [{ required: true, message: t('login.validation.password_required'), trigger: 'blur' }]
}))

function clearSavedPasswordSentinel() {
  if (isSavedPasswordSentinel.value) {
    loginForm.password = ''
  }
}

function restoreSavedPasswordSentinel() {
  if (hasSavedCredential.value && loginForm.rememberMe && !loginForm.password) {
    loginForm.password = SAVED_PASSWORD_SENTINEL
  }
}

// 加载已有设置时不重复写回；仅响应用户主动修改勾选状态。
let hydratingPreferences = true

watch(() => loginForm.rememberMe, (rememberMe) => {
  if (!rememberMe && isSavedPasswordSentinel.value) {
    loginForm.password = ''
  }
  persistLoginPreferences()
})

watch(() => loginForm.autoLogin, () => {
  persistLoginPreferences()
})

async function loadSavedLogin() {
  try {
    const saved = await invoke<{ identifier: string; auto_login: boolean } | null>('get_saved_login')
    if (saved) {
      loginForm.identifier = saved.identifier
      loginForm.rememberMe = true
      loginForm.autoLogin = saved.auto_login
      hasSavedCredential.value = true
      loginForm.password = SAVED_PASSWORD_SENTINEL
    } else {
      // 无已保存凭据（如曾登出）：按已持久化的偏好回填勾选状态。
      const prefs = await invoke<{ remember_me: boolean; auto_login: boolean }>('get_settings')
      loginForm.rememberMe = prefs.remember_me
      loginForm.autoLogin = prefs.remember_me && prefs.auto_login
    }
  } catch (error) {
    console.error('Failed to load saved login settings:', error)
  } finally {
    hydratingPreferences = false
  }
}

// 勾选项属于偏好设置，用户修改后立即持久化。这里刻意不传表单中的账号或
// 密码；密码从未进入此命令，登录凭据只会在服务端确认登录成功后更新。
function persistLoginPreferences() {
  if (hydratingPreferences) {
    return
  }
  const rememberMe = loginForm.rememberMe
  const autoLogin = rememberMe && loginForm.autoLogin
  invoke('save_login_settings', {
    identifier: '',
    rememberMe,
    autoLogin
  }).catch(error => {
    console.warn('[Login] 保存登录偏好失败:', error)
  })
}

async function canLoginWithCurrentVersion() {
  const result = await invoke<UpdateCheckResponse>('check_update')
  if (result.mode === 'force') {
    ElMessage.warning(t('login.message.version_outdated'))
    try {
      await openClientDownloadPage()
    } catch (error) {
      ElMessage.error(t('app.actions.open_update_failed', { error }))
    }
    return false
  }
  return true
}

async function handleLogin() {
  const valid = await loginFormRef.value?.validate().catch(() => false)
  if (!valid) {
    return
  }

  loading.value = true
  // 登录成功会立即切换 App 的渲染分支并卸载登录组件。提前快照表单值，
  // 避免登录网络请求期间的异步回填或组件卸载影响随后保存的偏好。
  const useSavedSession = isSavedPasswordSentinel.value
  const loginSettings = {
    identifier: loginForm.identifier,
    rememberMe: loginForm.rememberMe,
    autoLogin: loginForm.rememberMe && loginForm.autoLogin,
  }
  try {
    const canContinue = await canLoginWithCurrentVersion()
    if (!canContinue) {
      return
    }

    if (useSavedSession) {
      await authStore.resumeSavedSession(loginSettings.autoLogin)
    } else {
      const success = await authStore.login(loginForm.identifier, loginForm.password)
      if (!success) {
        ElMessage.error(t('login.message.failed'))
        return
      }

    }

    // 勾选状态已由 watch 即时保存；这里只在登录确认成功后把账号标识与
    // 设置关联（identifier 为空时后端保留已有标识）。密码不会传入。
    await invoke('save_login_settings', {
      identifier: loginSettings.identifier,
      rememberMe: loginSettings.rememberMe,
      autoLogin: loginSettings.autoLogin
    })
    ElMessage.success(t('login.message.success'))
    router.push('/')
  } catch (error) {
    ElMessage.error(t('login.message.failed_with_error', { error }))
  } finally {
    loading.value = false
  }
}

onMounted(() => {
  loadSavedLogin()
})
</script>

<style scoped>
.login-container {
  -webkit-app-region: no-drag;
  display: flex;
  justify-content: center;
  align-items: center;
  width: 100%;
  height: 100%;
  min-height: 0;
  padding: 24px;
  overflow: hidden;
  background:
    radial-gradient(circle at 20% 18%, rgba(0, 103, 192, 0.08), transparent 38%),
    var(--fluent-bg);
}

.login-container :deep(*) {
  -webkit-app-region: no-drag;
}

/* 卡片视觉由 .fluent-card 工具类提供，此处仅保留尺寸 */
.login-card {
  width: min(500px, 100%);
}

.login-card :deep(.el-card__header) {
  padding: 26px 34px 18px;
  border-bottom: 0;
}

.login-card :deep(.el-card__body) {
  padding: 10px 42px 34px;
}

.login-card :deep(.el-input__wrapper) {
  min-height: 42px;
}

.login-card :deep(.saved-password-sentinel .el-input__inner) {
  color: var(--fluent-text-tertiary);
}

.login-card :deep(.el-button) {
  min-height: 42px;
}

.card-header {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 10px;
}

.header-text h2 {
  margin: 0;
  color: var(--fluent-text);
  font-weight: 600;
}

.header-text p {
  margin: 4px 0 0;
  color: var(--fluent-text-secondary);
  font-size: 14px;
}

.footer {
  margin-top: 18px;
  text-align: center;
  color: var(--fluent-text-secondary);
}

.footer a {
  color: var(--fluent-accent);
  text-decoration: none;
}

.footer a:hover {
  text-decoration: underline;
}

.separator {
  margin: 0 10px;
  color: var(--fluent-text-tertiary);
}
</style>
