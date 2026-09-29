import { createRouter, createWebHashHistory } from 'vue-router'

const routes = [
  {
    // 根路径：未登录去登录页，已登录留在根路径（App.vue 会渲染主工作区）
    path: '/',
    name: 'Root',
    component: () => import('../views/Login.vue')
  },
  {
    path: '/login',
    name: 'Login',
    component: () => import('../views/Login.vue')
  },
  {
    path: '/register',
    name: 'Register',
    component: () => import('../views/Register.vue')
  },
  {
    path: '/reset-password',
    name: 'ResetPassword',
    component: () => import('../views/ResetPassword.vue')
  }
]

const router = createRouter({
  // 使用 hash 模式，适配 Tauri 的 file:// 和 IPC 协议
  history: createWebHashHistory(),
  routes
})

export default router
