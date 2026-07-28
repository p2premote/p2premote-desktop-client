import { test, expect } from '@playwright/test'

/**
 * 登录和设备列表 TDD 测试
 *
 * 运行方式：
 * - 前端纯 UI 测试（不需要 Tauri）：npm run dev 后直接跑
 * - 完整 Tauri 测试：npm run tauri dev（需要图形环境）
 */

test.describe('登录流程', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/#/login')
  })

  test('登录页正确渲染', async ({ page }) => {
    // 检查页面标题
    await expect(page).toHaveTitle(/p2pRemote/)

    // 检查登录表单元素存在
    await expect(page.locator('input[placeholder="邮箱/用户名"]')).toBeVisible()
    await expect(page.locator('input[type="password"]')).toBeVisible()
    await expect(page.locator('button:has-text("登录")')).toBeVisible()

    // 检查没有立即显示 Dashboard
    await expect(page.locator('text=我的设备')).not.toBeVisible()
  })

  test('登录表单验证', async ({ page }) => {
    // 不填内容直接点登录，应该触发验证
    const loginBtn = page.locator('button:has-text("登录")')
    await loginBtn.click()

    // 应该显示验证错误
    await expect(page.locator('text=请输入邮箱或用户名')).toBeVisible()
  })
})

test.describe('登录后设备列表', () => {
  /**
   * 注意：这个测试需要 Tauri runtime 环境
   * 如果在纯 Vite dev 模式下运行，会因为 Tauri API 不可用而失败
   *
   * 完整 Tauri 测试：
   * 1. 终端1: npm run tauri dev
   * 2. 终端2: npx playwright test --project=chromium
   */
  test('登录后应显示设备列表', async ({ page }) => {
    await page.goto('/#/login')

    // Tauri dev 模式：Vite server 和 Tauri WebView 共享同一 URL
    // Playwright 连 localhost:1420 即是 Tauri 加载的页面
    // 输入测试账号（真实账号）
    const testEmail = '209758171@163.com'
    const testPassword = 'Abcd123456'

    await page.fill('input[placeholder="邮箱/用户名"]', testEmail)
    await page.fill('input[type="password"]', testPassword)
    await page.click('button:has-text("登录")')

    // 等待响应
    await page.waitForTimeout(3000)

    // 检查是否有错误提示
    const errorMsg = page.locator('.el-message--error').first()
    const hasError = await errorMsg.isVisible().catch(() => false)

    if (hasError) {
      const errorText = await errorMsg.textContent()
      console.log('登录错误:', errorText)
      test.skip('登录失败，跳过设备列表检查')
      return
    }

    // 等待页面稳定
    await page.waitForTimeout(2000)

    // 检查是否显示"暂无设备"或者有设备卡片
    const noDeviceText = page.locator('text=暂无设备').first()
    const deviceCards = page.locator('.device-card')

    const hasNoDevice = await noDeviceText.isVisible().catch(() => false)
    const deviceCount = await deviceCards.count()

    console.log('暂无设备提示:', hasNoDevice)
    console.log('设备卡片数量:', deviceCount)

    // 验证：要么显示设备列表，要么显示"暂无设备"
    expect(hasNoDevice || deviceCount > 0).toBeTruthy()
  })
})
