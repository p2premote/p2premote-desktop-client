<template>
  <div class="reset-container">
    <el-card class="reset-card fluent-card">
      <h2>重置密码</h2>
      <el-form ref="formRef" :model="form" :rules="rules" @submit.prevent="submit">
        <el-form-item prop="email"><el-input v-model="form.email" :disabled="seconds > 0" placeholder="注册邮箱" /></el-form-item>
        <el-form-item prop="code">
          <div class="code-row"><el-input v-model="form.code" inputmode="numeric" maxlength="6" placeholder="6 位邮件验证码" />
            <el-button :loading="sending" :disabled="seconds > 0" @click="send">{{ seconds > 0 ? `${seconds}s` : '发送验证码' }}</el-button></div>
        </el-form-item>
        <el-form-item prop="password"><el-input v-model="form.password" type="password" show-password placeholder="新密码" /></el-form-item>
        <el-form-item prop="confirmPassword"><el-input v-model="form.confirmPassword" type="password" show-password placeholder="再次输入新密码" /></el-form-item>
        <el-form-item><el-button native-type="submit" type="primary" :loading="loading" style="width:100%">重置密码</el-button></el-form-item>
      </el-form>
      <router-link to="/login">返回登录</router-link>
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { reactive, ref, onBeforeUnmount } from 'vue'
import { invoke } from '../runtime/bridge'
import { ElMessage } from 'element-plus/es/components/message/index.mjs'
import { useRouter } from 'vue-router'
import { errorMessage } from '../utils/errorMessage'

const router = useRouter(); const formRef = ref(); const sending = ref(false); const loading = ref(false); const seconds = ref(0)
let timer: number | undefined
const form = reactive({ email: '', code: '', password: '', confirmPassword: '' })
const validateConfirmPassword = (_rule: unknown, value: string, callback: (error?: Error) => void) => {
  if (value !== form.password) callback(new Error('两次输入的密码不一致'))
  else callback()
}
const rules = {
  email: [{ required: true, type: 'email', message: '请输入有效邮箱', trigger: 'blur' }],
  code: [{ required: true, pattern: /^\d{6}$/, message: '请输入 6 位数字验证码', trigger: 'blur' }],
  password: [{ required: true, min: 6, message: '密码至少 6 位', trigger: 'blur' }],
  confirmPassword: [
    { required: true, message: '请再次输入新密码', trigger: 'blur' },
    { validator: validateConfirmPassword, trigger: 'blur' },
  ],
}
async function send() {
  if (!await formRef.value.validateField('email').catch(() => false)) return
  sending.value = true
  try {
    await invoke('send_reset_password_verification_code', { email: form.email })
    ElMessage.success('验证码已发送')
    seconds.value = 60
    timer = window.setInterval(() => {
      if (--seconds.value <= 0 && timer) { clearInterval(timer); timer = undefined }
    }, 1000)
  } catch (error) {
    ElMessage.error(errorMessage(error, '验证码发送失败，请稍后重试'))
  } finally {
    sending.value = false
  }
}
async function submit() {
  if (!await formRef.value.validate().catch(() => false)) return
  loading.value = true
  try {
    await invoke('reset_password_by_email_code', { email: form.email, verificationCode: form.code, newPassword: form.password })
    ElMessage.success('密码重置成功')
    router.push('/login')
  } catch (error) {
    ElMessage.error(errorMessage(error, '密码重置失败，请稍后重试'))
  } finally {
    loading.value = false
  }
}
onBeforeUnmount(() => { if (timer) clearInterval(timer) })
</script>

<style scoped>.reset-container{display:flex;align-items:center;justify-content:center;width:100%;height:100%;padding:24px}.reset-card{width:min(500px,100%)}.code-row{display:flex;width:100%;gap:8px}</style>
