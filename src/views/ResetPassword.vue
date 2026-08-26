<template>
  <div class="reset-container">
    <el-card class="reset-card fluent-card">
      <h2>{{ $t('reset_password.title') }}</h2>
      <el-form ref="formRef" :model="form" :rules="rules" @submit.prevent="submit">
        <el-form-item prop="email"><el-input v-model="form.email" :disabled="seconds > 0" :placeholder="$t('reset_password.email_placeholder')" /></el-form-item>
        <el-form-item prop="code">
          <div class="code-row"><el-input v-model="form.code" inputmode="numeric" maxlength="6" :placeholder="$t('reset_password.code_placeholder')" />
            <el-button :loading="sending" :disabled="seconds > 0" @click="send">{{ seconds > 0 ? `${seconds}s` : $t('reset_password.send_code') }}</el-button></div>
        </el-form-item>
        <el-form-item prop="password"><el-input v-model="form.password" type="password" show-password :placeholder="$t('reset_password.password_placeholder')" /></el-form-item>
        <el-form-item prop="confirmPassword"><el-input v-model="form.confirmPassword" type="password" show-password :placeholder="$t('reset_password.confirm_password_placeholder')" /></el-form-item>
        <el-form-item><el-button native-type="submit" type="primary" :loading="loading" style="width:100%">{{ $t('reset_password.submit') }}</el-button></el-form-item>
      </el-form>
      <router-link to="/login">{{ $t('reset_password.back_to_login') }}</router-link>
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { reactive, ref, computed, onBeforeUnmount } from 'vue'
import { invoke } from '../runtime/bridge'
import { ElMessage } from 'element-plus/es/components/message/index.mjs'
import type { FormRules } from 'element-plus/es/components/form/index.mjs'
import { useRouter } from 'vue-router'
import { errorMessage } from '../utils/errorMessage'
import { useI18n } from 'vue-i18n'

const { t } = useI18n()
const router = useRouter(); const formRef = ref(); const sending = ref(false); const loading = ref(false); const seconds = ref(0)
let timer: number | undefined
const form = reactive({ email: '', code: '', password: '', confirmPassword: '' })
const validateConfirmPassword = (_rule: unknown, value: string, callback: (error?: Error) => void) => {
  if (value !== form.password) callback(new Error(t('reset_password.password_mismatch')))
  else callback()
}
const rules = computed<FormRules>(() => ({
  email: [{ required: true, type: 'email', message: t('reset_password.email_invalid'), trigger: 'blur' }],
  code: [{ required: true, pattern: /^\d{6}$/, message: t('reset_password.code_invalid'), trigger: 'blur' }],
  password: [{ required: true, pattern: /^(?=.*[A-Za-z])(?=.*\d).{6,}$/, message: t('reset_password.password_invalid'), trigger: 'blur' }],
  confirmPassword: [
    { required: true, message: t('reset_password.confirm_required'), trigger: 'blur' },
    { validator: validateConfirmPassword, trigger: 'blur' },
  ],
}))
async function send() {
  if (!await formRef.value.validateField('email').catch(() => false)) return
  sending.value = true
  try {
    await invoke('send_reset_password_verification_code', { email: form.email })
    ElMessage.success(t('reset_password.code_sent'))
    seconds.value = 60
    timer = window.setInterval(() => {
      if (--seconds.value <= 0 && timer) { clearInterval(timer); timer = undefined }
    }, 1000)
  } catch (error) {
    ElMessage.error(errorMessage(error, t('reset_password.code_send_failed')))
  } finally {
    sending.value = false
  }
}
async function submit() {
  if (!await formRef.value.validate().catch(() => false)) return
  loading.value = true
  try {
    await invoke('reset_password_by_email_code', { email: form.email, verificationCode: form.code, newPassword: form.password })
    ElMessage.success(t('reset_password.success'))
    router.push('/login')
  } catch (error) {
    ElMessage.error(errorMessage(error, t('reset_password.failed')))
  } finally {
    loading.value = false
  }
}
onBeforeUnmount(() => { if (timer) clearInterval(timer) })
</script>

<style scoped>.reset-container{display:flex;align-items:center;justify-content:center;width:100%;height:100%;padding:24px}.reset-card{width:min(500px,100%)}.code-row{display:flex;width:100%;gap:8px}</style>
