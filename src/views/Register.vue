<template>
  <div class="register-container">
    <el-card class="register-card fluent-card">
      <template #header>
        <div class="card-header">
          <AppLogo :size="36" />
          <div class="header-text">
            <h2>p2pRemote</h2>
            <p>{{ $t('register.title') }}</p>
          </div>
        </div>
      </template>

      <el-form :model="form" :rules="rules" ref="formRef" @submit.prevent="handleRegister" label-position="left" label-width="80px">
        <el-form-item :label="$t('register.username_label')" prop="username">
          <el-input v-model="form.username" :placeholder="$t('register.username_placeholder')" />
        </el-form-item>

        <el-form-item :label="$t('register.email_label')" prop="email">
          <el-input v-model="form.email" :disabled="sendCountdown > 0" :placeholder="$t('register.email_placeholder')" />
        </el-form-item>

        <el-form-item :label="$t('register.validation.verification_code_label')" prop="verificationCode">
          <div class="captcha-row">
            <el-input v-model="form.verificationCode" inputmode="numeric" maxlength="6" :placeholder="$t('register.validation.verification_code_placeholder')" />
            <el-button :loading="sendingCode" :disabled="sendCountdown > 0" @click="sendVerificationCode">
              {{ sendCountdown > 0 ? `${sendCountdown}s` : $t('register.message.send_code') }}
            </el-button>
          </div>
        </el-form-item>

        <el-form-item :label="$t('register.invite_code_label')" prop="inviteCode">
          <el-input v-model="form.inviteCode" :placeholder="$t('register.invite_code_placeholder')" />
        </el-form-item>

        <el-form-item :label="$t('register.password_label')" prop="password">
          <el-input v-model="form.password" type="password" :placeholder="$t('register.password_placeholder')" show-password />
        </el-form-item>

        <el-form-item :label="$t('register.confirm_password_label')" prop="confirmPassword">
          <el-input v-model="form.confirmPassword" type="password" :placeholder="$t('register.confirm_password_placeholder')" show-password />
        </el-form-item>

        <el-form-item :label="$t('register.captcha_label')" prop="captcha">
          <div class="captcha-row">
            <el-input v-model="form.captcha" :placeholder="$t('register.captcha_placeholder')" style="flex: 1" />
            <canvas ref="captchaCanvas" class="captcha-image" @click="refreshCaptcha" :title="$t('register.captcha_refresh_title')" />
          </div>
        </el-form-item>

        <el-form-item>
          <el-button type="primary" :loading="loading" native-type="submit" style="width: 100%">
            {{ $t('register.submit') }}
          </el-button>
        </el-form-item>
      </el-form>

      <div class="footer">
        <router-link to="/login">{{ $t('register.has_account') }}</router-link>
      </div>
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { computed, ref, reactive, onMounted, onBeforeUnmount } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'
import { ElMessage } from 'element-plus/es/components/message/index.mjs'
import type { FormInstance, FormRules } from 'element-plus/es/components/form/index.mjs'
import { invoke } from '../runtime/bridge'
import { errorMessage } from '../utils/errorMessage'
import AppLogo from '../components/AppLogo.vue'

const { t } = useI18n()
const router = useRouter()
const formRef = ref<FormInstance>()
const loading = ref(false)
const captchaCanvas = ref<HTMLCanvasElement | null>(null)
const currentCaptcha = ref('')
const sendingCode = ref(false)
const sendCountdown = ref(0)
let sendCountdownTimer: number | undefined

// 验证码字符集（避免易混淆字符）
const CAPTCHA_CHARS = 'ABCDEFGHJKLMNPQRSTUVWXYZabcdefghjkmnpqrstuvwxyz23456789'

// 生成随机验证码
function generateCaptchaText(length: number = 4): string {
  let result = ''
  for (let i = 0; i < length; i++) {
    result += CAPTCHA_CHARS.charAt(Math.floor(Math.random() * CAPTCHA_CHARS.length))
  }
  return result
}

// 绘制验证码图片
function drawCaptcha(text: string) {
  const canvas = captchaCanvas.value
  if (!canvas) return

  const ctx = canvas.getContext('2d')
  if (!ctx) return

  canvas.width = 100
  canvas.height = 36

  // 白色背景
  ctx.fillStyle = '#f0f0f0'
  ctx.fillRect(0, 0, canvas.width, canvas.height)

  // 绘制干扰线
  for (let i = 0; i < 3; i++) {
    ctx.strokeStyle = `rgba(${Math.random() * 200 + 50}, ${Math.random() * 200 + 50}, ${Math.random() * 200 + 50}, 0.5)`
    ctx.beginPath()
    ctx.moveTo(Math.random() * canvas.width, Math.random() * canvas.height)
    ctx.lineTo(Math.random() * canvas.width, Math.random() * canvas.height)
    ctx.stroke()
  }

  // 绘制验证码文字
  ctx.font = 'bold 24px Arial'
  ctx.textAlign = 'center'
  ctx.textBaseline = 'middle'

  const chars = text.split('')
  const perCharWidth = canvas.width / (chars.length + 1)

  chars.forEach((char, index) => {
    const x = perCharWidth * (index + 0.5) + (Math.random() - 0.5) * 10
    const y = canvas.height / 2 + (Math.random() - 0.5) * 10
    const rotation = (Math.random() - 0.5) * 0.4

    ctx.save()
    ctx.translate(x, y)
    ctx.rotate(rotation)
    ctx.fillStyle = `rgb(${Math.random() * 80 + 50}, ${Math.random() * 80 + 50}, ${Math.random() * 80 + 150})`
    ctx.fillText(char, 0, 0)
    ctx.restore()
  })
}

// 刷新验证码
function refreshCaptcha() {
  currentCaptcha.value = generateCaptchaText()
  drawCaptcha(currentCaptcha.value)
}

const form = reactive({
  username: '',
  email: '',
  verificationCode: '',
  inviteCode: '',
  password: '',
  confirmPassword: '',
  captcha: ''
})

const validateConfirmPassword = (_rule: any, value: string, callback: any) => {
  if (value !== form.password) {
    callback(new Error(t('register.validation.password_mismatch')))
  } else {
    callback()
  }
}

const validateCaptcha = (_rule: any, value: string, callback: any) => {
  if (!value) {
    callback(new Error(t('register.validation.captcha_required')))
  } else if (value.toLowerCase() !== currentCaptcha.value.toLowerCase()) {
    callback(new Error(t('register.validation.captcha_wrong')))
    refreshCaptcha()
  } else {
    callback()
  }
}

const rules = computed<FormRules>(() => ({
  username: [
    { required: true, message: t('register.validation.username_required'), trigger: 'blur' },
    { min: 3, max: 30, message: t('register.validation.username_length'), trigger: 'blur' }
  ],
  email: [
    { required: true, message: t('register.validation.email_required'), trigger: 'blur' },
    { type: 'email', message: t('register.validation.email_format'), trigger: 'blur' }
  ],
  password: [
    { required: true, message: t('register.validation.password_required'), trigger: 'blur' },
    { min: 6, message: t('register.validation.password_length'), trigger: 'blur' }
  ],
  verificationCode: [
    { required: true, message: t('register.validation.verification_code_required'), trigger: 'blur' },
    { pattern: /^\d{6}$/, message: t('register.validation.verification_code_format'), trigger: 'blur' }
  ],
  confirmPassword: [
    { required: true, message: t('register.validation.confirm_password_required'), trigger: 'blur' },
    { validator: validateConfirmPassword, trigger: 'blur' }
  ],
  captcha: [
    { required: true, message: t('register.validation.captcha_required'), trigger: 'blur' },
    { validator: validateCaptcha, trigger: 'blur' }
  ]
}))

async function handleRegister() {
  const valid = await formRef.value?.validate().catch(() => false)
  if (!valid) return

  loading.value = true
  try {
    const response = await invoke<{ code: number; msg: string }>('register_by_email_code', {
      username: form.username,
      email: form.email,
      password: form.password,
      verificationCode: form.verificationCode,
      inviteCode: form.inviteCode || undefined
    })

    if (response.code === 0) {
      ElMessage.success({
        message: t('register.message.success'),
        duration: 3000
      })
      setTimeout(() => {
        router.push('/login')
      }, 3000)
    } else {
      // Tauri 已优先按结构化错误键本地化；旧服务端兼容回退到 msg。
      ElMessage.error(response.msg || t('register.message.failed'))
      refreshCaptcha()
    }
  } catch (e) {
    ElMessage.error(errorMessage(e, t('register.message.failed')))
    refreshCaptcha()
  } finally {
    loading.value = false
  }
}

async function sendVerificationCode() {
  const valid = await formRef.value?.validateField('email').catch(() => false)
  if (!valid) return
  sendingCode.value = true
  try {
    await invoke('send_registration_verification_code', { email: form.email })
    ElMessage.success(t('register.message.code_sent'))
    sendCountdown.value = 60
    sendCountdownTimer = window.setInterval(() => {
      sendCountdown.value -= 1
      if (sendCountdown.value <= 0 && sendCountdownTimer) {
        window.clearInterval(sendCountdownTimer)
        sendCountdownTimer = undefined
      }
    }, 1000)
  } catch (error) {
    ElMessage.error(errorMessage(error, t('register.message.code_send_failed')))
  } finally {
    sendingCode.value = false
  }
}

onMounted(() => {
  refreshCaptcha()
})

onBeforeUnmount(() => {
  if (sendCountdownTimer) window.clearInterval(sendCountdownTimer)
})
</script>

<style scoped>
.register-container {
  -webkit-app-region: no-drag;
  display: flex;
  justify-content: center;
  align-items: center;
  width: 100%;
  height: 100%;
  min-height: 0;
  padding: 24px;
  overflow-y: auto;
  background:
    radial-gradient(circle at 20% 18%, rgba(0, 103, 192, 0.08), transparent 38%),
    var(--fluent-bg);
}

.register-container :deep(*) {
  -webkit-app-region: no-drag;
}

/* 卡片视觉由 .fluent-card 工具类提供，此处仅保留尺寸 */
.register-card {
  width: min(500px, 100%);
}

.register-card :deep(.el-card__header) {
  padding: 26px 34px 18px;
  border-bottom: 0;
}

.register-card :deep(.el-card__body) {
  padding: 10px 42px 34px;
}

.register-card :deep(.el-input__wrapper) {
  min-height: 42px;
}

.register-card :deep(.el-button) {
  min-height: 42px;
}

.card-header {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 10px;
}

.header-text {
  text-align: center;
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

.captcha-row {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
}

.captcha-image {
  cursor: pointer;
  border-radius: var(--fluent-radius-sm);
  flex-shrink: 0;
}

.footer {
  text-align: center;
  margin-top: 16px;
  color: var(--fluent-text-secondary);
}

.footer a {
  color: var(--fluent-accent);
  text-decoration: none;
}

.footer a:hover {
  text-decoration: underline;
}
</style>
