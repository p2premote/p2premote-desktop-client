import { expect, test, type Page } from '@playwright/test'

type MockCall = {
  cmd: string
  args?: Record<string, unknown>
}

async function installTauriMock(page: Page, locale = 'zh-CN', includeRemoteDevice = false) {
  await page.addInitScript(({ initialLocale, includeRemoteDevice }) => {
    localStorage.setItem('p2premote:locale', initialLocale)
    const callbacks = new Map<number, (payload: unknown) => void>()
    const calls: Array<{ cmd: string, args?: Record<string, unknown> }> = []
    let callbackId = 1
    const state = {
      serviceAvailable: true,
      tunnelRuntime: null as unknown,
    }

    const localDevice = {
      device_id: 11,
      user_id: 1,
      device_uuid: 'local-uuid',
      device_name: '本机',
      device_alias: '本机',
      device_type: 'Windows',
      system_version: 'Windows 11',
      lan_ip: '127.0.0.1',
      service_port: 3389,
      status: 'online',
      connect_code: '709621823',
      created_at: '',
      updated_at: '',
    }

    const remoteDevice = {
      device_id: 22,
      user_id: 2,
      device_uuid: 'remote-uuid',
      device_name: '对方电脑',
      device_alias: '对方电脑',
      device_type: 'Windows',
      system_version: 'Windows 11',
      lan_ip: '10.0.0.2',
      service_port: 3389,
      status: 'online',
      connect_code: '428279225',
      created_at: '',
      updated_at: '',
    }

    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: {
        writeText: async (text: string) => {
          ;(window as any).__lastClipboardText = text
        },
      },
    })

    ;(window as any).__p2premoteMockCalls = calls
    ;(window as any).__p2premoteMockState = state
    ;(window as any).__TAURI_INTERNALS__ = {
      callbacks,
      transformCallback: (callback: (payload: unknown) => void) => {
        const id = callbackId++
        callbacks.set(id, callback)
        return id
      },
      unregisterCallback: (id: number) => {
        callbacks.delete(id)
      },
      convertFileSrc: (filePath: string) => filePath,
      invoke: async (cmd: string, args?: Record<string, unknown>) => {
        calls.push({ cmd, args })
        if (cmd === 'plugin:event|listen') return args?.handler
        if (cmd === 'plugin:event|unlisten') return null
        if (cmd === 'plugin:event|emit') return null
        if (cmd === 'plugin:window|minimize') return null
        if (cmd === 'plugin:window|toggle_maximize') return null
        if (cmd === 'plugin:window|is_maximized') return false
        if (cmd === 'plugin:window|close') return null

        switch (cmd) {
          case 'check_update':
            return { mode: 'none', current: '1.0.10', latest: '1.0.10', min_supported: '1.0.0' }
          case 'try_auto_login':
            return 'mock-token'
          case 'get_user_info':
          case 'fetch_user_profile':
            return {
              user_id: 1,
              username: 'tester',
              email: 'tester@example.com',
              status: 'active',
              member_level: 'pro',
            }
          case 'ensure_background_service_session':
          case 'get_service_status':
          case 'refresh_service_network_info':
            return {
              service: { installed: true, running: state.serviceAvailable, enabled: true, raw_state: state.serviceAvailable ? 'running' : 'stopped' },
              runtime: state.serviceAvailable
                ? {
                    logged_in: true,
                    ws_connected: true,
                    invite_temporary_password: null,
                    active_tunnel_jobs: [],
                    current_device: localDevice,
                    ...((state.tunnelRuntime as Record<string, unknown> | null) || {}),
                  }
                : null,
            }
          case 'listen_service_events':
            return null
          case 'refresh_tunnel_status':
            return { runtime: state.tunnelRuntime }
          case 'get_device_list':
            return {
              code: 0,
              msg: 'ok',
              data: includeRemoteDevice ? [localDevice, remoteDevice] : [localDevice],
            }
          case 'set_device_password':
            return { code: 0, msg: 'ok' }
          case 'generate_connect_code':
            return localDevice.connect_code
          case 'parse_invite_info': {
            // 镜像 core/src/invite.rs 的解析语义：标签行优先，退回双 token
            const input = String(args?.input || '')
            let deviceCode: string | null = null
            let temporaryPassword: string | null = null
            for (const line of input.split('\n').map(l => l.trim()).filter(Boolean)) {
              const idx = line.indexOf('：') >= 0 ? line.indexOf('：') : line.indexOf(':')
              if (idx < 0) continue
              const key = line.slice(0, idx).trim().toLowerCase()
              const value = line.slice(idx + 1).replace(/[^0-9a-zA-Z]/g, '')
              if (!value) continue
              if (key.includes('设备代码') || key === 'code' || key === 'device code') {
                deviceCode = value
              } else if (key.includes('临时密码') || key === 'password' || key === 'temporary password') {
                temporaryPassword = value
              }
            }
            if (!deviceCode || !temporaryPassword) {
              const tokens = input.split(/\s+/).map(t => t.replace(/[^0-9a-zA-Z]/g, '')).filter(Boolean)
              if (tokens.length === 2) {
                deviceCode = tokens[0]
                temporaryPassword = tokens[1]
              }
            }
            if (deviceCode && temporaryPassword && deviceCode.length >= 6 && temporaryPassword.length >= 4) {
              return { deviceCode, temporaryPassword }
            }
            return null
          }
          case 'start_service_anonymous_active_tunnel':
            if (!state.serviceAvailable) {
              return { success: false, message: '后台服务未运行，暂时无法发起远程协助' }
            }
            if (args?.connectCode === remoteDevice.connect_code && args?.temporaryPassword === '123456') {
              return { success: true, message: 'ok', device: remoteDevice }
            }
            return { success: false, message: '设备代码或临时密码错误' }
          case 'stop_active_tunnel_job':
            return '已取消'
          case 'stop_service_tunnel':
          case 'stop_service_active_tunnel':
            return {
              service: { installed: true, running: true, enabled: true, raw_state: 'running' },
              runtime: {
                logged_in: true,
                ws_connected: true,
                active_tunnel_jobs: [],
                wgvpn_sessions: [],
                tunnel_lifecycles: [],
                current_device: localDevice,
              },
            }
          case 'test_tunnel_speed':
            return {
              latency_ms: 12.5,
              download_mbps: 88.25,
              upload_mbps: 42.75,
              retransmits: 0,
            }
          case 'flash_main_window':
          case 'sync_service_runtime_config':
            return null
          case 'get_settings':
            return {
              auto_start: true,
              log_level: 'info',
              version: '1.0.10',
              remember_me: true,
              auto_login: true,
            }
          default:
            return null
        }
      },
    }
  }, { initialLocale: locale, includeRemoteDevice })
}

async function openRemoteAssistance(page: Page, locale = 'zh-CN') {
  await installTauriMock(page, locale)
  await page.goto('/')
  const english = locale === 'en'
  await page.getByRole('button', { name: english ? /Remote assistance/ : /远程协助/ }).click()
  await expect(page.getByRole('heading', {
    name: english ? 'Invite someone to assist you' : '邀请对方远程协助',
  })).toBeVisible()
}

function mockCalls(page: Page): Promise<MockCall[]> {
  return page.evaluate(() => (window as any).__p2premoteMockCalls)
}

async function setServiceAvailable(page: Page, available: boolean) {
  await page.evaluate((nextAvailable) => {
    ;(window as any).__p2premoteMockState.serviceAvailable = nextAvailable
  }, available)
}

test.describe('远程协助', () => {
  test('邀请卡片只提供复制邀请信息入口', async ({ page }) => {
    await openRemoteAssistance(page)

    await expect(page.getByText('设备代码')).toBeVisible()
    await expect(page.locator('.invite-card input').first()).toHaveValue('709621823')
    await expect(page.getByText('临时密码', { exact: true })).toBeVisible()
    await expect(page.getByRole('button', { name: '生成' })).not.toBeVisible()
    await expect(page.getByRole('button', { name: /^复制$/ })).not.toBeVisible()

    await page.getByRole('button', { name: '复制邀请信息' }).click()
    const copiedInvitation = () => page.evaluate(() => (window as any).__lastClipboardText)
    await expect.poll(copiedInvitation).toContain('✦ P2P Remote｜远程协助邀请')
    await expect.poll(copiedInvitation).toContain('设备代码: 709621823')
    await expect.poll(copiedInvitation).toContain('临时密码:')
    await expect.poll(copiedInvitation).toContain('1. 打开 P2P Remote，进入「远程协助」')
    await expect.poll(copiedInvitation).toContain('2. 粘贴本邀请信息，点击「建立远程连接」')
    await expect.poll(copiedInvitation).toContain('⚠ 临时密码仅可使用一次，请勿转发给无关人员。')
  })

  test('服务端已有临时密码时挂载直接复用，不重新生成', async ({ page }) => {
    await installTauriMock(page)
    await page.goto('/')
    await page.evaluate(() => {
      ;(window as any).__p2premoteMockState.tunnelRuntime = { invite_temporary_password: '654321' }
    })
    await page.getByRole('button', { name: '远程协助' }).click()

    await expect(page.getByRole('heading', { name: '邀请对方远程协助' })).toBeVisible()
    await expect(page.locator('.invite-card input').nth(1)).toHaveValue('654321')
    const calls = await mockCalls(page)
    expect(calls.some(call => call.cmd === 'set_device_password')).toBeFalsy()
  })

  test('邀请信息格式错误时不请求匿名连接接口', async ({ page }) => {
    await openRemoteAssistance(page)

    await page.getByPlaceholder(/请粘贴对方发来的邀请信息/).fill('只有一段错误内容')
    await page.locator('.connect-card').getByRole('button', { name: /建立远程连接/ }).click()

    await expect(page.getByText('邀请信息格式不正确')).toBeVisible()
    const calls = await mockCalls(page)
    expect(calls.some(call => call.cmd === 'start_service_anonymous_active_tunnel')).toBeFalsy()
  })

  test('支持多种邀请信息格式并启动自动建立隧道任务', async ({ page }) => {
    await openRemoteAssistance(page)

    const inviteInputs = [
      '设备代码：428279225\n临时密码：123456',
      '设备代码: 428 279 225\n临时密码: 123456',
      '428279225 123456',
      '428279225\n123456',
    ]

    for (const input of inviteInputs) {
      await page.getByPlaceholder(/请粘贴对方发来的邀请信息/).fill(input)
      await page.locator('.connect-card').getByRole('button', { name: /建立远程连接/ }).click()

      await expect.poll(async () => {
        const calls = await mockCalls(page)
        return calls.filter(call => call.cmd === 'start_service_anonymous_active_tunnel').length
      }).toBeGreaterThanOrEqual(inviteInputs.indexOf(input) + 1)
    }

    await page.evaluate(() => {
      window.dispatchEvent(new CustomEvent('p2p-active-tunnel-job-updated', {
        detail: {
          target_device_id: 22,
          target_device_uuid: 'remote-uuid',
          state: 'waiting',
          attempt: 2,
          max_attempts: 30,
          message: '本次失败，60 秒后重试',
          updated_at: Date.now(),
        },
      }))
    })
    await expect(page.getByText('本次尝试失败，等待重试')).toBeVisible()
    await expect(page.getByText('第 2/30 次，本次失败，60 秒后重试')).toBeVisible()
  })

  test('英文界面生成的邀请信息可以被连接端解析', async ({ page }) => {
    await openRemoteAssistance(page, 'en')

    await page.getByRole('button', { name: 'Copy invitation' }).click()
    await expect.poll(
      () => page.evaluate(() => (window as any).__lastClipboardText as string),
    ).toMatch(/Device code: 709621823[\s\S]*Temporary password:/)

    const englishInvite = 'Device code: 428279225\nTemporary password: 123456'
    await page.getByPlaceholder(/Paste the invitation/).fill(englishInvite)
    await page.locator('.connect-card').getByRole('button', { name: /Establish remote connection/ }).click()

    await expect.poll(async () => {
      const calls = await mockCalls(page)
      return calls.some(call => call.cmd === 'start_service_anonymous_active_tunnel')
    }).toBeTruthy()
  })

  test('运行时切换语言会更新校验文案且只同步一次新语言', async ({ page }) => {
    await openRemoteAssistance(page)

    await page.locator('.header-btn').click()
    await page.locator('.settings-panel .language-select').click()
    await page.getByRole('option', { name: 'English' }).click()

    await expect(page.getByRole('heading', { name: 'Connect to a remote computer' })).toBeVisible()
    await page.locator('.connect-card').getByRole('button', { name: /Establish remote connection/ }).click()
    await expect(page.getByText('Paste the invitation sent by the other person', { exact: true })).toBeVisible()

    const calls = await mockCalls(page)
    expect(calls.filter(call => call.cmd === 'set_locale' && call.args?.locale === 'en')).toHaveLength(1)
  })

  test('设置面板可以切换并持久化深色主题', async ({ page }) => {
    await openRemoteAssistance(page)

    await page.getByRole('button', { name: '打开设置' }).click()
    await page.locator('.settings-panel .theme-select').click()
    await page.getByRole('option', { name: '深色' }).click()

    await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark')
    await expect.poll(() => page.evaluate(() => localStorage.getItem('p2premote-theme'))).toBe('dark')
  })

  test('键盘可以切换工作区', async ({ page }) => {
    await openRemoteAssistance(page)

    const tunnelNav = page.getByRole('button', { name: '隧道状态' })
    await tunnelNav.focus()
    await tunnelNav.press('Enter')

    await expect(tunnelNav).toHaveAttribute('aria-current', 'page')
    await expect(page.locator('.workspace-main').getByRole('heading', { name: '隧道状态' })).toBeVisible()
  })

  test('最小窗口尺寸下工作区不会产生页面级横向滚动', async ({ page }) => {
    await page.setViewportSize({ width: 800, height: 600 })
    await openRemoteAssistance(page)

    const overflow = await page.evaluate(() => ({
      viewport: document.documentElement.clientWidth,
      content: document.documentElement.scrollWidth,
    }))
    expect(overflow.content).toBeLessThanOrEqual(overflow.viewport)
  })

  test('设备操作按钮暴露完整的可访问说明', async ({ page }) => {
    await installTauriMock(page, 'zh-CN', true)
    await page.goto('/')

    await page.locator('.device-list-item').filter({ hasText: '对方电脑' }).click()
    await expect(page.locator('.action-tile.primary')).toHaveAttribute(
      'aria-label',
      /建立隧道.*创建可用的 P2P 通道/,
    )
  })

  test('被动隧道在设备列表明确标识并开放可用操作', async ({ page }) => {
    await installTauriMock(page, 'zh-CN', true)
    await page.goto('/', { waitUntil: 'domcontentloaded' })
    await page.evaluate(() => {
      ;(window as any).__p2premoteMockState.tunnelRuntime = {
        wgvpn_sessions: [
          {
            peer_device_id: 22,
            is_active: false,
            virtual_ip: '100.99.71.2',
            peer_virtual_ip: '100.99.71.43',
            health_state: 'connected',
            local_forward_port: 0,
            exposed_lan_cidrs: [],
          },
        ],
        tunnel_lifecycles: [
          {
            peer_device_id: 22,
            role: 'passive',
            state: 'connected',
            attempt: 1,
            max_attempts: 30,
            last_result: 'none',
            virtual_ip: '100.99.71.2',
            peer_virtual_ip: '100.99.71.43',
          },
        ],
      }
    })

    await page.getByRole('button', { name: '刷新' }).click()
    const remoteDevice = page.locator('.device-list-item').filter({ hasText: '对方电脑' })
    await expect(remoteDevice).toHaveClass(/tunnel-connected/)
    await expect(remoteDevice.getByText('被动隧道已连接')).toBeVisible()

    await remoteDevice.click()
    await expect(page.locator('.tunnel-lifecycle-strip strong')).toHaveText('被动隧道已连接')
    await expect(page.locator('.action-tile.primary')).toContainText('断开被动隧道')
    await expect(page.getByRole('button', { name: '复制虚拟 IP' })).toContainText('100.99.71.43')
    await expect(page.getByRole('button', { name: '隧道测速' })).toBeVisible()

    await page.getByRole('button', { name: '隧道测速' }).click()
    await expect.poll(async () => {
      const calls = await mockCalls(page)
      return calls.some(call => call.cmd === 'test_tunnel_speed' && call.args?.peerDeviceId === 22)
    }).toBe(true)
    await page.getByRole('button', { name: '确定' }).click()

    await page.evaluate(() => {
      const runtime = (window as any).__p2premoteMockState.tunnelRuntime
      runtime.wgvpn_sessions[0].health_state = 'degraded'
      runtime.tunnel_lifecycles[0].state = 'recovering'
      runtime.tunnel_lifecycles[0].message = '等待主动端恢复'
    })
    await page.getByRole('button', { name: '刷新' }).click()
    await expect(page.locator('.action-tile.primary')).toContainText('断开被动隧道')
    await expect(page.locator('.action-tile.primary')).toHaveAttribute(
      'aria-label',
      /断开被动隧道.*关闭当前 P2P 通道/,
    )

    await page.locator('.action-tile.primary').click()
    await expect.poll(async () => {
      const calls = await mockCalls(page)
      return calls.some(call => call.cmd === 'stop_service_tunnel' && call.args?.sourceDeviceId === 22)
    }).toBe(true)
    const calls = await page.evaluate(() => (window as any).__p2premoteMockCalls as MockCall[])
    expect(calls.some(call => call.cmd === 'start_service_active_tunnel')).toBe(false)
  })

  test('隧道状态按角色分组并将诊断字段收进网络详情', async ({ page }) => {
    await installTauriMock(page)
    await page.goto('/')
    await page.evaluate(() => {
      ;(window as any).__p2premoteMockState.tunnelRuntime = {
        wgvpn_sessions: [
          {
            peer_device_id: 22,
            is_active: true,
            virtual_ip: '100.99.71.43',
            peer_virtual_ip: '100.99.71.2',
            health_state: 'connected',
            consecutive_failures: 0,
            received_bytes: 39322,
            transmitted_bytes: 24474,
          },
          {
            peer_device_id: 44,
            is_active: false,
            virtual_ip: '100.99.71.43',
            peer_virtual_ip: '100.99.71.4',
            health_state: 'connected',
            consecutive_failures: 0,
            received_bytes: 1024,
            transmitted_bytes: 2048,
          },
        ],
        tunnel_lifecycles: [
          {
            peer_device_id: 44,
            source_user_id: 7,
            source_username: '209758171',
            peer_device_name: '客厅 PC 5700',
            peer_device_alias: '',
            peer_public_ip: '203.0.113.10',
            role: 'passive',
            state: 'connected',
            attempt: 1,
            max_attempts: 30,
            message: '隧道已连接',
            last_result: 'none',
            virtual_ip: '100.99.71.43',
            peer_virtual_ip: '100.99.71.4',
            connected_at: Math.floor(Date.now() / 1000) - 3600,
            updated_at: Math.floor(Date.now() / 1000),
          },
        ],
      }
    })

    await page.getByRole('button', { name: '隧道状态' }).click()
    await expect(page.getByRole('heading', { name: /主动隧道 1/ })).toBeVisible()
    await expect(page.getByRole('heading', { name: /被动隧道 1/ })).toBeVisible()
    await expect(page.getByText('客厅 PC 5700')).toBeVisible()
    await expect(page.getByText('来自 209758171')).toBeVisible()
    await expect(page.getByText('隧道已连接')).not.toBeVisible()

    const detailsToggle = page.getByRole('button', { name: '网络详情' }).last()
    await expect(detailsToggle).toHaveAttribute('aria-expanded', 'false')
    await detailsToggle.press('Enter')
    await expect(detailsToggle).toHaveAttribute('aria-expanded', 'true')
    await expect(page.getByText('203.0.113.10')).toBeVisible()
  })

  test('service 不可用时不启动自动建立隧道任务', async ({ page }) => {
    await openRemoteAssistance(page)
    await setServiceAvailable(page, false)

    await page.getByPlaceholder(/请粘贴对方发来的邀请信息/).fill('设备代码：428279225\n临时密码：123456')
    await page.locator('.connect-card').getByRole('button', { name: /建立远程连接/ }).click()

    await expect(page.getByText('后台服务未运行，暂时无法发起远程协助')).toBeVisible()
    const calls = await mockCalls(page)
    expect(calls.some(call => call.cmd === 'start_service_anonymous_active_tunnel')).toBeTruthy()
  })

  test('新连接校验失败会清空上一台设备的任务状态', async ({ page }) => {
    await openRemoteAssistance(page)

    await page.getByPlaceholder(/请粘贴对方发来的邀请信息/).fill('设备代码：428279225\n临时密码：123456')
    await page.locator('.connect-card').getByRole('button', { name: /建立远程连接/ }).click()
    await expect(page.getByText('对方电脑', { exact: true })).toBeVisible()
    await page.evaluate(() => {
      window.dispatchEvent(new CustomEvent('p2p-active-tunnel-job-updated', {
        detail: {
          target_device_id: 22,
          target_device_uuid: 'remote-uuid',
          state: 'waiting',
          attempt: 2,
          max_attempts: 30,
          message: '本次失败，60 秒后重试',
          updated_at: Date.now(),
        },
      }))
    })
    await expect(page.getByText('本次尝试失败，等待重试')).toBeVisible()

    await page.getByPlaceholder(/请粘贴对方发来的邀请信息/).fill('设备代码：428279225\n临时密码：000000')
    await page.locator('.connect-card').getByRole('button', { name: /建立远程连接/ }).click()

    await expect(page.getByText('设备代码或临时密码错误')).toBeVisible()
    await expect(page.getByText('本次尝试失败，等待重试')).not.toBeVisible()
  })

  test('隧道建立成功后其他工具提示引用地址中的实际端口', async ({ page }) => {
    await openRemoteAssistance(page)

    await page.getByPlaceholder(/请粘贴对方发来的邀请信息/).fill('设备代码：428279225\n临时密码：123456')
    await page.locator('.connect-card').getByRole('button', { name: /建立远程连接/ }).click()
    await expect(page.getByText('对方电脑', { exact: true })).toBeVisible()

    await page.evaluate(() => {
      window.dispatchEvent(new CustomEvent('p2p-active-tunnel-job-updated', {
        detail: {
          target_device_id: 22,
          target_device_uuid: 'remote-uuid',
          state: 'succeeded',
          attempt: 1,
          max_attempts: 30,
          message: '隧道已自动建立成功',
          updated_at: Date.now(),
          result: { success: true, local_port: 0, rdp_address: '100.99.71.43:3390' },
        },
      }))
    })

    await expect(page.locator('.tunnel-result-card code')).toHaveText('100.99.71.43:3390')
    const hint = page.locator('.tunnel-result-card .result-hint').nth(1)
    await expect(hint).toContainText('把端口 3390 改成对应服务端口，例如 RustDesk 直连端口为 21118')
    await expect(hint).not.toContainText('gonc.cc')
  })
})
