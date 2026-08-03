import { createRouter, createWebHashHistory } from 'vue-router'
import { useAuthStore } from '../stores/auth'

const routes = [
  {
    // 根路径：未登录去登录页，已登录留在根路径（App.vue 会渲染主工作区）
    path: '/',
    name: 'Root',
    component: () => import('../views/Login.vue'),
    meta: { requiresAuth: false }
  },
  {
    path: '/login',
    name: 'Login',
    component: () => import('../views/Login.vue'),
    meta: { requiresAuth: false }
  },
  {
    path: '/register',
    name: 'Register',
    component: () => import('../views/Register.vue'),
    meta: { requiresAuth: false }
  },
  {
    path: '/reset-password',
    name: 'ResetPassword',
    component: () => import('../views/ResetPassword.vue'),
    meta: { requiresAuth: false }
  }
]

const router = createRouter({
  // 使用 hash 模式，适配 Tauri 的 file:// 和 IPC 协议
  history: createWebHashHistory(),
  routes
})

router.beforeEach((to, _from, next) => {
  const authStore = useAuthStore()

  // 未登录用户访问非登录/注册页 → 去登录页
  if (to.meta.requiresAuth !== false && !authStore.isLoggedIn) {
    next('/login')
  } else if (to.path === '/login' && authStore.isLoggedIn) {
    // 已登录用户访问登录页 → 留在这里（App.vue 会显示主工作区）
    next()
  } else {
    next()
  }
})

export default router
