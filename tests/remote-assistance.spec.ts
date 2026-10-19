import { expect, test, type Page } from '@playwright/test'
import { readFileSync } from 'node:fs'

const tauriConfig = JSON.parse(readFileSync(new URL('../src-tauri/tauri.conf.json', import.meta.url), 'utf8'))

type MockCall = {
  cmd: string
  args?: Record<string, unknown>
}

async function installTauriMock(
  page: Page,
  locale = 'zh-CN',
  includeRemoteDevice = false,
  networkFailure: 'dns' | 'service' | null = null,
  delayedAutoLoginFailure = false,
  manualLoginError: string | null = null,
) {
  await page.addInitScript(({ initialLocale, includeRemoteDevice, networkFailure, delayedAutoLoginFailure, manualLoginError, dialogSaveAllowed }) => {
    localStorage.setItem('p2premote:locale', initialLocale)
    const callbacks = new Map<number, (payload: unknown) => void>()
    const eventListeners = new Map<number, { event: string; handler: number }>()
    let nextEventId = 1
    const calls: Array<{ cmd: string, args?: Record<string, unknown> }> = []
    let callbackId = 1
    const state = {
      serviceAvailable: true,
      tunnelRuntime: null as unknown,
      preferences: { prefer_ipv6: false, prefer_tcp: false },
      memberLevel: 'pro',
      autoKeepSupported: false,
      autoKeepStatuses: [] as Array<{ target_device_id: number; enabled: boolean; state: string }>,
      lan: { enabled: false, cidrs: [] as string[] },
      failPreferences: false,
      serviceAccess: { mode: 'all', tcp: [] as Array<{ start: number; end: number }>, udp: [] as Array<{ start: number; end: number }>, allow_ping: false },
      failServiceAccess: false,
      networkFailure,
      delayedAutoLoginFailure,
      manualLoginError,
      logArchiveDestination: 'C:\\Users\\tester\\Downloads\\p2premote-logs.zip' as string | null,
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
      capabilities: ['rustdesk_tiny'],
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
      capabilities: ['rustdesk_tiny'],
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
    ;(window as any).__emitActiveJob = (job: unknown) => {
      const payload = { logged_in: true, ws_connected: true, current_device: localDevice, active_tunnel_jobs: [job] }
      state.tunnelRuntime = payload
      for (const [id, subscription] of eventListeners) {
        if (subscription.event === 'service-status-changed') {
          ;(window as any)[`_${subscription.handler}`]?.({ event: subscription.event, id, payload })
        }
      }
    }
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
          case 'export_logs': return 2
          case 'get_connection_preferences': return { ...state.preferences }
          case 'save_connection_preferences':
            if (state.failPreferences) throw new Error('mock save failed')
            state.preferences = { prefer_ipv6: Boolean(args?.preferIpv6), prefer_tcp: Boolean(args?.preferTcp) }
            return { ...state.preferences }
          case 'get_wgvpn_lan_access_config': return { ...state.lan }
          case 'get_service_access_policy': return { policy: state.serviceAccess, applied: true }
          case 'save_service_access_policy':
            if (state.failServiceAccess) throw new Error('mock policy apply failed')
            state.serviceAccess = args?.policy as typeof state.serviceAccess
            return { policy: state.serviceAccess, applied: true }
          case 'save_wgvpn_lan_access_config':
            state.lan = { enabled: Boolean(args?.enabled), cidrs: args?.cidrs as string[] || [] }
            return { ...state.lan }
          case 'check_update':
            return { mode: 'none', current: '1.0.10', latest: '1.0.10', min_supported: '1.0.0' }
          case 'try_auto_login':
            if (state.delayedAutoLoginFailure) {
              await new Promise(resolve => setTimeout(resolve, 500))
              throw new Error('network is unreachable')
            }
            return 'mock-token'
          case 'login':
            if (state.manualLoginError) throw new Error(state.manualLoginError)
            return {
              code: 0,
              msg: 'ok',
              data: {
                user: {
                  user_id: 2,
                  username: 'manual-user',
                  email: 'manual@example.com',
                },
              },
            }
          case 'get_user_info':
          case 'fetch_user_profile':
            return {
              user_id: 1,
              username: 'tester',
              email: 'tester@example.com',
              status: 'active',
              member_level: state.memberLevel,
              is_pro: state.memberLevel === 'pro',
            }
          case 'refresh_service_network_info':
            if (state.networkFailure === 'dns') {
              throw new Error('error sending request for url https://internal.example/api: dns lookup failed')
            }
            if (state.networkFailure === 'service') {
              throw { code: 'SERVICE_NOT_READY', retryable: true }
            }
          case 'ensure_background_service_session':
          case 'get_service_status':
            return {
              service: { installed: true, running: state.serviceAvailable, enabled: true, raw_state: state.serviceAvailable ? 'running' : 'stopped' },
              runtime: state.serviceAvailable
                ? {
                    logged_in: true,
                    ws_connected: true,
                    invite_temporary_password: null,
                    active_tunnel_jobs: [],
                    current_device: localDevice,
                    auto_keep_statuses: state.autoKeepStatuses,
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
              data: includeRemoteDevice ? [localDevice, { ...remoteDevice, capabilities: state.autoKeepSupported ? ['rustdesk_tiny', 'structured_tunnel_stop_reason'] : remoteDevice.capabilities }] : [localDevice],
            }
          case 'get_auto_keep_statuses': return state.autoKeepStatuses
          case 'set_auto_keep': {
            const status = { target_device_id: Number(args?.targetDeviceId), enabled: Boolean(args?.enabled), state: args?.enabled ? 'active' : 'stopped' }
            state.autoKeepStatuses = state.autoKeepStatuses.filter(item => item.target_device_id !== status.target_device_id)
            if (status.enabled) state.autoKeepStatuses.push(status)
            return status
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
            if ((state as any).holdStop) {
              await new Promise<void>(resolve => { (window as any).__completeTunnelStop = resolve })
            }
            if ((state as any).failStop) throw new Error('mock stop failed')
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
    // The project uses Tauri 1; adapt the mock's async dispatcher to its IPC.
    ;(window as any).__TAURI_METADATA__ = { __windows: [{ label: 'main' }], __currentWindow: { label: 'main' } }
    ;(window as any).__TAURI_IPC__ = (message: any) => {
      let promise: Promise<unknown>
      if (message.cmd === 'tauri') {
        const command = message.message?.cmd
        if (message.__tauriModule === 'Dialog' && command === 'saveDialog') {
          calls.push({ cmd: 'saveDialog', args: message.message.options })
          promise = dialogSaveAllowed
            ? Promise.resolve(state.logArchiveDestination)
            : Promise.reject('dialog > save not in the allowlist')
        } else if (message.__tauriModule === 'Event' && command === 'listen') {
          const id = nextEventId++
          eventListeners.set(id, { event: message.message.event, handler: message.message.handler })
          promise = Promise.resolve(id)
        } else if (message.__tauriModule === 'Event' && command === 'unlisten') {
          eventListeners.delete(message.message.eventId)
          promise = Promise.resolve(null)
        } else {
          promise = Promise.resolve(command === 'listen' ? 1 : command === 'isMaximized' ? false : null)
        }
      } else promise = (window as any).__TAURI_INTERNALS__.invoke(message.cmd, message)
      promise.then(value => (window as any)[`_${message.callback}`]?.(value))
        .catch(error => (window as any)[`_${message.error}`]?.(
          error && typeof error === 'object' && 'code' in error ? error : String(error),
        ))
    }
  }, {
    initialLocale: locale, includeRemoteDevice, networkFailure, delayedAutoLoginFailure, manualLoginError,
    dialogSaveAllowed: Boolean(tauriConfig.tauri.allowlist.dialog?.save),
  })
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

test('日志导出允许打开保存窗口并将选择的路径传给打包命令', async ({ page }) => {
  await installTauriMock(page)
  await page.goto('/')
  await page.getByRole('button', { name: '打开设置', exact: true }).click()
  const settings = page.getByRole('dialog', { name: '设置', exact: true })
  await settings.getByRole('tab', { name: '关于与支持', exact: true }).click()
  await settings.getByRole('button', { name: '打包并导出日志', exact: true }).click()

  await expect(page.getByText(/已将 2 个日志文件保存到/)).toBeVisible()
  const calls = await mockCalls(page)
  expect(calls.find(call => call.cmd === 'saveDialog')?.args).toMatchObject({
    defaultPath: expect.stringMatching(/^p2premote-logs-.*\.zip$/),
    filters: [{ name: 'ZIP', extensions: ['zip'] }],
  })
  expect(calls.find(call => call.cmd === 'export_logs')?.args).toMatchObject({
    destination: 'C:\\Users\\tester\\Downloads\\p2premote-logs.zip',
  })
  await expect(page.getByText(/日志打包失败/)).toHaveCount(0)
})

test('取消日志保存窗口不调用打包命令也不报错', async ({ page }) => {
  await installTauriMock(page)
  await page.goto('/')
  await page.evaluate(() => { (window as any).__p2premoteMockState.logArchiveDestination = null })
  await page.getByRole('button', { name: '打开设置', exact: true }).click()
  const settings = page.getByRole('dialog', { name: '设置', exact: true })
  await settings.getByRole('tab', { name: '关于与支持', exact: true }).click()
  await settings.getByRole('button', { name: '打包并导出日志', exact: true }).click()

  await expect.poll(async () => (await mockCalls(page)).filter(call => call.cmd === 'saveDialog').length).toBe(1)
  expect((await mockCalls(page)).filter(call => call.cmd === 'export_logs')).toHaveLength(0)
  await expect(page.getByText(/日志打包失败|已将 \d+ 个日志文件保存到/)).toHaveCount(0)
})

test('断网启动进入主界面且不显示底层调试信息', async ({ page }) => {
  await installTauriMock(page, 'zh-CN', false, 'dns')
  await page.goto('/')

  await expect(page.getByRole('button', { name: '我的设备', exact: true })).toBeVisible()
  await expect(page.getByText('无法连接服务器，请检查网络或 DNS 设置')).toBeVisible()
  await expect(page.getByText(/internal\.example|dns lookup|error sending request/i)).toHaveCount(0)

  await page.evaluate(() => { (window as any).__p2premoteMockState.networkFailure = null })
  await page.getByRole('button', { name: '立即重试', exact: true }).click()
  await expect(page.getByText('无法连接服务器，请检查网络或 DNS 设置')).toHaveCount(0)
  await expect.poll(async () => (await mockCalls(page)).filter(call => call.cmd === 'refresh_service_network_info').length).toBeGreaterThanOrEqual(2)
})

test('后台服务异常不会误报为断网', async ({ page }) => {
  await installTauriMock(page, 'zh-CN', false, 'service')
  await page.goto('/')

  await expect(page.getByText('后台服务尚未就绪，请稍后重试')).toBeVisible()
  await expect(page.getByText('当前网络不可用。联网后将自动恢复。')).toHaveCount(0)
})

test('手动登录不会被延迟失败的自动恢复清除', async ({ page }) => {
  await installTauriMock(page, 'zh-CN', false, null, true)
  await page.goto('/')

  await page.getByPlaceholder('邮箱或用户名').fill('manual-user')
  await page.getByPlaceholder('密码').fill('manual-password')
  await page.getByRole('button', { name: '登录', exact: true }).click()

  await expect(page.getByRole('button', { name: '我的设备', exact: true })).toBeVisible()
  await page.waitForTimeout(700)
  await expect(page.getByRole('button', { name: '我的设备', exact: true })).toBeVisible()
})

test('登录保留旧接口的业务提示', async ({ page }) => {
  await installTauriMock(page, 'zh-CN', false, null, true, '用户名或密码错误')
  await page.goto('/')

  await page.getByPlaceholder('邮箱或用户名').fill('manual-user')
  await page.getByPlaceholder('密码').fill('manual-password')
  await page.getByRole('button', { name: '登录', exact: true }).click()

  await expect(page.getByText(/登录失败: (?:Error: )?用户名或密码错误/)).toBeVisible()
})

test('连接偏好保存失败回滚，LAN 配置在设置窗口保存', async ({ page }) => {
  await installTauriMock(page)
  await page.goto('/')
  await page.getByRole('button', { name: '打开设置', exact: true }).click()
  const settings = page.getByRole('dialog', { name: '设置', exact: true })
  await expect(settings).toBeVisible()
  await settings.getByRole('tab', { name: '连接', exact: true }).click()
  const tcp = settings.getByRole('switch', { name: '优先使用 TCP', exact: true })
  await expect(tcp).toHaveAttribute('aria-checked', 'false')
  await tcp.locator('..').click()
  await expect(tcp).toHaveAttribute('aria-checked', 'true')
  await page.evaluate(() => { (window as any).__p2premoteMockState.failPreferences = true })
  await tcp.locator('..').click()
  await expect(tcp).toHaveAttribute('aria-checked', 'true')
  await settings.getByRole('tab', { name: '局域网访问', exact: true }).click()
  await settings.locator('#lan-enabled').locator('..').click()
  await settings.locator('#lan-cidrs').fill('192.168.1.0/24')
  await settings.getByRole('button', { name: '保存局域网访问设置', exact: true }).click()
  await expect.poll(() => page.evaluate(() => (window as any).__p2premoteMockState.lan.cidrs)).toEqual(['192.168.1.0/24'])
  await expect(settings.getByRole('button', { name: '保存局域网访问设置', exact: true })).not.toBeVisible()
})

test('服务访问规则支持端口范围、空白名单和实时保存', async ({ page }) => {
  await installTauriMock(page)
  await page.goto('/')
  await page.getByRole('button', { name: '打开设置', exact: true }).click()
  const settings = page.getByRole('dialog', { name: '设置', exact: true })
  await settings.getByRole('tab', { name: '安全', exact: true }).click()
  await expect(settings.getByText('允许访问的服务', { exact: true })).toBeVisible()
  await settings.locator('.service-access-settings .el-select').click()
  await page.getByRole('option', { name: '仅允许指定服务', exact: true }).click()
  await expect(settings.getByText('未填写端口时，所有 TCP/UDP 服务均被禁止；Ping 由独立开关控制。')).toBeVisible()
  await settings.locator('#service-access-tcp').fill('3389, 1000 - 2000')
  await settings.locator('#service-access-udp').fill('53 6000-6100')
  await settings.locator('#service-access-ping').locator('..').click()
  await settings.getByRole('button', { name: '保存', exact: true }).click()
  await expect(settings.getByText('已保存并生效', { exact: true })).toBeVisible()
  await page.screenshot({ path: 'test-results/service-access-settings.png' })
  await page.setViewportSize({ width: 1000, height: 600 })
  await expect.poll(async () => {
    const bounds = await settings.boundingBox()
    return bounds ? bounds.y + bounds.height : Infinity
  }).toBeLessThanOrEqual(600)
  await expect.poll(() => page.evaluate(() => (window as any).__p2premoteMockState.serviceAccess)).toEqual({
    mode: 'allowlist', tcp: [{ start: 3389, end: 3389 }, { start: 1000, end: 2000 }],
    udp: [{ start: 53, end: 53 }, { start: 6000, end: 6100 }], allow_ping: true,
  })
  await settings.locator('#service-access-tcp').fill('')
  await settings.locator('#service-access-udp').fill('')
  await settings.getByRole('button', { name: '保存', exact: true }).click()
  await expect.poll(() => page.evaluate(() => (window as any).__p2premoteMockState.serviceAccess.tcp)).toEqual([])
})

test('服务访问规则拒绝无效端口并保留保存失败的草稿', async ({ page }) => {
  await installTauriMock(page)
  await page.goto('/')
  await page.getByRole('button', { name: '打开设置', exact: true }).click()
  const settings = page.getByRole('dialog', { name: '设置', exact: true })
  await settings.getByRole('tab', { name: '安全', exact: true }).click()
  await settings.locator('.service-access-settings .el-select').click()
  await page.getByRole('option', { name: '仅允许指定服务', exact: true }).click()
  const savesBefore = (await mockCalls(page)).filter(call => call.cmd === 'save_service_access_policy').length
  for (const invalid of ['0', '65536', '2000-1000', 'abc']) {
    await settings.locator('#service-access-tcp').fill(invalid)
    await settings.getByRole('button', { name: '保存', exact: true }).click()
    await expect(settings.getByText(`无效端口或范围：${invalid}`, { exact: true })).toBeVisible()
  }
  expect((await mockCalls(page)).filter(call => call.cmd === 'save_service_access_policy')).toHaveLength(savesBefore)
  await page.evaluate(() => { (window as any).__p2premoteMockState.failServiceAccess = true })
  await settings.locator('#service-access-tcp').fill('3389')
  await settings.getByRole('button', { name: '保存', exact: true }).click()
  await expect(settings.getByText('无法保存或应用规则，请检查后台服务后重试。修改已保留。', { exact: true })).toBeVisible()
  await expect(settings.locator('#service-access-tcp')).toHaveValue('3389')
  await expect.poll(() => page.evaluate(() => (window as any).__p2premoteMockState.serviceAccess.mode)).toBe('all')
  await settings.getByRole('button', { name: '关闭此对话框', exact: true }).click()
  await expect(page.getByText('放弃尚未保存的服务访问规则？', { exact: true })).toBeVisible()
  await page.getByRole('button', { name: '继续编辑', exact: true }).click()
  await expect(settings.locator('#service-access-tcp')).toHaveValue('3389')
})

async function setServiceAvailable(page: Page, available: boolean) {
  await page.evaluate((nextAvailable) => {
    ;(window as any).__p2premoteMockState.serviceAvailable = nextAvailable
  }, available)
}

test('远程协助打洞失败可以关闭 TCP 优先并重新发起', async ({ page }) => {
  await openRemoteAssistance(page)
  await page.getByPlaceholder(/请粘贴对方发来的邀请信息/).fill('设备代码：428279225\n临时密码：123456')
  await page.locator('.connect-card').getByRole('button', { name: /建立远程连接/ }).click()
  await expect(page.getByRole('heading', { name: '对方电脑', exact: true })).toBeVisible()
  await page.evaluate(() => {
    (window as any).__p2premoteMockState.preferences.prefer_tcp = true
    ;(window as any).__emitActiveJob({
      peer_device_id: 22, state: 'failed', attempt: 1, max_attempts: 30,
      message: 'punch_exhausted', tcp_retry_recommended: true,
    })
  })
  await page.getByRole('dialog', { name: '隧道建立失败', exact: true }).getByRole('button', { name: '知道了', exact: true }).click()
  await page.getByRole('button', { name: '关闭 TCP 优先并重试', exact: true }).click()
  await expect.poll(async () => (await mockCalls(page)).filter(call => call.cmd === 'start_service_anonymous_active_tunnel').length).toBe(2)
  const retryCalls = await mockCalls(page)
  expect(retryCalls.filter(call => call.cmd === 'start_service_anonymous_active_tunnel')[1]?.args).toMatchObject({
    connectCode: '428279225', temporaryPassword: '123456',
  })
  expect(retryCalls.findIndex(call => call.cmd === 'stop_service_active_tunnel')).toBeLessThan(
    retryCalls.findLastIndex(call => call.cmd === 'start_service_anonymous_active_tunnel'),
  )
  await expect.poll(() => page.evaluate(() => (window as any).__p2premoteMockState.preferences.prefer_tcp)).toBe(false)
})

test('停止失败时远程协助重试不发起新连接', async ({ page }) => {
  await openRemoteAssistance(page)
  await page.getByPlaceholder(/请粘贴对方发来的邀请信息/).fill('设备代码：428279225\n临时密码：123456')
  await page.locator('.connect-card').getByRole('button', { name: /建立远程连接/ }).click()
  await expect(page.getByRole('heading', { name: '对方电脑', exact: true })).toBeVisible()
  await page.evaluate(() => {
    ;(window as any).__p2premoteMockState.failStop = true
    ;(window as any).__emitActiveJob({
      peer_device_id: 22, state: 'failed', attempt: 1, max_attempts: 30,
      message: 'punch_exhausted', tcp_retry_recommended: true,
    })
  })
  await page.getByRole('dialog', { name: '隧道建立失败', exact: true }).getByRole('button', { name: '知道了', exact: true }).click()
  await page.getByRole('button', { name: '关闭 TCP 优先并重试', exact: true }).click()
  await expect(page.getByText('未知错误', { exact: true })).toBeVisible()
  expect((await mockCalls(page)).filter(call => call.cmd === 'start_service_anonymous_active_tunnel')).toHaveLength(1)
})

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
      ;(window as any).__emitActiveJob({
          peer_device_id: 22,
          peer_device_uuid: 'remote-uuid',
          state: 'waiting',
          attempt: 2,
          max_attempts: 30,
          message: '本次失败，60 秒后重试',
          updated_at: Date.now(),
      })
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
    await page.getByRole('dialog').getByRole('button', { name: /Close this dialog|关闭此对话框/ }).click()

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
    await expect(page.locator('.action-tile.primary')).toContainText('取消')
    await expect(page.locator('.action-tile.primary')).toHaveAttribute(
      'aria-label',
      /取消.*取消当前隧道任务/,
    )

    await page.locator('.action-tile.primary').click()
    await expect.poll(async () => {
      const calls = await mockCalls(page)
      return calls.some(call => call.cmd === 'stop_service_tunnel' && call.args?.sourceDeviceId === 22)
    }).toBe(true)
    const calls = await page.evaluate(() => (window as any).__p2premoteMockCalls as MockCall[])
    expect(calls.some(call => call.cmd === 'start_service_active_tunnel')).toBe(false)
  })

  test('自动保持开关通过本地命令保存，不携带云端 revision', async ({ page }) => {
    await installTauriMock(page, 'zh-CN', true)
    await page.addInitScript(() => {
      const state = (window as any).__p2premoteMockState
      state.autoKeepSupported = true
      state.tunnelRuntime = { tunnel_lifecycles: [{ peer_device_id: 22, role: 'active', state: 'connected', attempt: 1, max_attempts: 30, last_result: 'none', updated_at: 1 }] }
    })
    await page.goto('/', { waitUntil: 'domcontentloaded' })
    await page.locator('.device-list-item').filter({ hasText: '对方电脑' }).click()
    const toggle = page.getByRole('switch', { name: '自动保持隧道', exact: true })
    await page.locator('.auto-keep-setting .el-switch').click()
    await expect(toggle).toBeChecked()
    const args = (await mockCalls(page)).find(call => call.cmd === 'set_auto_keep')?.args
    expect(args).toMatchObject({ targetDeviceId: 22, enabled: true })
    expect(args).not.toHaveProperty('expectedRevision')
    expect(args).not.toHaveProperty('expected_revision')
    await page.locator('.auto-keep-setting .el-switch').click()
    await expect(toggle).not.toBeChecked()
  })

  for (const role of ['passive', 'unestablished']) {
    test(`仅主动端可以新开启自动保持：${role}`, async ({ page }) => {
      await installTauriMock(page, 'zh-CN', true)
      await page.addInitScript((role) => {
        const state = (window as any).__p2premoteMockState
        state.autoKeepSupported = true
        state.tunnelRuntime = { tunnel_lifecycles: [{ peer_device_id: 22, role: role === 'passive' ? 'passive' : 'active', state: role === 'passive' ? 'connected' : 'not_established', attempt: 1, max_attempts: 30, last_result: 'none', updated_at: 1 }] }
      }, role)
      await page.goto('/', { waitUntil: 'domcontentloaded' })
      await page.locator('.device-list-item').filter({ hasText: '对方电脑' }).click()
      await expect(page.getByRole('switch', { name: '自动保持隧道', exact: true })).toBeDisabled()
      await expect(page.getByText('请先由本机建立隧道，再开启自动保持；被动端不能开启。', { exact: true })).toBeVisible()
      expect((await mockCalls(page)).some(call => call.cmd === 'set_auto_keep')).toBe(false)
    })
  }

  for (const enabled of [false, true]) {
    test(`非 Pro 的客户端入口判断：本地设置${enabled ? '已开启仍可继续使用和关闭' : '未开启时不能新开启'}`, async ({ page }) => {
      await installTauriMock(page, 'zh-CN', true)
      await page.addInitScript((enabled) => {
        const state = (window as any).__p2premoteMockState
        state.memberLevel = 'normal'
        state.autoKeepSupported = true
        state.tunnelRuntime = { tunnel_lifecycles: [{ peer_device_id: 22, role: 'active', state: 'connected', attempt: 1, max_attempts: 30, last_result: 'none', updated_at: 1 }] }
        state.autoKeepStatuses = enabled ? [{ target_device_id: 22, enabled: true, state: 'active' }] : []
      }, enabled)
      await page.goto('/', { waitUntil: 'domcontentloaded' })
      await page.locator('.device-list-item').filter({ hasText: '对方电脑' }).click()
      const toggle = page.getByRole('switch', { name: '自动保持隧道', exact: true })
      if (enabled) {
        await expect(toggle).toBeChecked()
        await expect(toggle).toBeEnabled()
        await expect(page.getByText('会员已到期，续费后恢复自动保持。')).toHaveCount(0)
        await page.locator('.auto-keep-setting .el-switch').click()
        await expect(toggle).not.toBeChecked()
      } else {
        await expect(toggle).toBeDisabled()
        expect((await mockCalls(page)).some(call => call.cmd === 'set_auto_keep')).toBe(false)
      }
    })
  }

  test('设备重试等待停止完成响应后才启动，不轮询任务状态', async ({ page }) => {
    await installTauriMock(page, 'zh-CN', true)
    await page.goto('/', { waitUntil: 'domcontentloaded' })
    await page.evaluate(() => {
      ;(window as any).__p2premoteMockState.holdStop = true
      ;(window as any).__p2premoteMockState.tunnelRuntime = {
        active_tunnel_jobs: [{
          peer_device_id: 22, peer_device_uuid: 'remote-uuid', state: 'failed',
          attempt: 1, max_attempts: 30, tcp_retry_recommended: true,
          message: 'punch_exhausted:tcp4,udp4', updated_at: 1,
        }],
      }
    })
    await page.getByRole('button', { name: '刷新', exact: true }).click()
    await page.locator('.device-list-item').filter({ hasText: '对方电脑' }).click()
    await page.getByRole('button', { name: '关闭 TCP 优先并重试', exact: true }).click()
    await expect.poll(() => page.evaluate(() => typeof (window as any).__completeTunnelStop)).toBe('function')
    expect((await mockCalls(page)).some(call => call.cmd === 'start_service_active_tunnel')).toBe(false)
    await page.evaluate(() => (window as any).__completeTunnelStop())
    await expect.poll(async () => (await mockCalls(page)).filter(call => call.cmd === 'start_service_active_tunnel').length).toBe(1)
    const calls = await mockCalls(page)
    expect(calls.find(call => call.cmd === 'start_service_active_tunnel')?.args).toMatchObject({
      peerDeviceId: 22, peerDeviceUuid: 'remote-uuid',
    })
    expect(calls.filter(call => call.cmd === 'stop_service_active_tunnel')).toHaveLength(1)
    expect(calls.find(call => call.cmd === 'save_connection_preferences')?.args?.preferTcp).toBe(false)
  })

  test('设备失败建议仅在主动连接最终失败时出现，且只显示一个操作', async ({ page }) => {
    await installTauriMock(page, 'zh-CN', true)
    await page.goto('/', { waitUntil: 'domcontentloaded' })
    await page.locator('.device-list-item').filter({ hasText: '对方电脑' }).click()
    const tcpRetry = page.getByRole('button', { name: '关闭 TCP 优先并重试', exact: true })
    const networkHelp = page.getByRole('button', { name: '查看网络优化帮助', exact: true })

    await page.evaluate(() => (window as any).__emitActiveJob({
      peer_device_id: 22, state: 'waiting', attempt: 1, max_attempts: 30,
      message: '本次未能建立直连，等待自动重试',
      tcp_retry_recommended: true, symmetric_nat_help_recommended: true,
    }))
    await expect(page.locator('.tunnel-lifecycle-strip')).toContainText('等待自动重试')
    await expect(tcpRetry).toHaveCount(0)
    await expect(networkHelp).toHaveCount(0)

    await page.evaluate(() => (window as any).__emitActiveJob({
      peer_device_id: 22, state: 'failed', attempt: 1, max_attempts: 30,
      error_code: 'traversal.punch.timeout',
      message: '本次未能建立直连。本次已开启 TCP 优先，建议关闭后重试。',
      tcp_retry_recommended: true, symmetric_nat_help_recommended: true,
    }))
    await page.getByRole('dialog', { name: '隧道建立失败', exact: true }).getByRole('button', { name: '知道了', exact: true }).click()
    await expect(page.locator('.tunnel-lifecycle-strip')).toContainText('建议关闭后重试')
    await expect(tcpRetry).toBeVisible()
    await expect(networkHelp).toHaveCount(0)

    await page.evaluate(() => (window as any).__emitActiveJob({
      peer_device_id: 22, state: 'succeeded', attempt: 2, max_attempts: 30,
      message: '已连接', result: { success: true },
      tcp_retry_recommended: true, symmetric_nat_help_recommended: true,
    }))
    await expect(tcpRetry).toHaveCount(0)
    await expect(networkHelp).toHaveCount(0)
  })

  test('被动连接失败显示服务给出的原因和建议，不显示主动端设置操作', async ({ page }) => {
    await installTauriMock(page, 'zh-CN', true)
    await page.goto('/', { waitUntil: 'domcontentloaded' })
    await page.evaluate(() => {
      ;(window as any).__p2premoteMockState.tunnelRuntime = {
        tunnel_lifecycles: [{
          peer_device_id: 22, role: 'passive', state: 'not_established',
          attempt: 1, max_attempts: 30, last_result: 'attempt_failed',
          error_code: 'connection.approval.denied',
          message: '本次连接请求未获允许。请确认本次访问授权，再由发起方重试。',
          updated_at: 1,
        }],
      }
    })
    await page.getByRole('button', { name: '刷新', exact: true }).click()
    await page.locator('.device-list-item').filter({ hasText: '对方电脑' }).click()
    await expect(page.locator('.tunnel-lifecycle-strip')).toContainText('本次连接请求未获允许')
    await expect(page.locator('.tunnel-lifecycle-strip')).toContainText('由发起方重试')
    await expect(page.getByRole('button', { name: '关闭 TCP 优先并重试', exact: true })).toHaveCount(0)
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
    await expect(page.getByText('客厅 PC 5700').first()).toBeVisible()
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
    await expect(page.getByRole('heading', { name: '对方电脑', exact: true })).toBeVisible()
    await page.evaluate(() => {
      ;(window as any).__emitActiveJob({
          peer_device_id: 22,
          peer_device_uuid: 'remote-uuid',
          state: 'waiting',
          attempt: 2,
          max_attempts: 30,
          message: '本次失败，60 秒后重试',
          updated_at: Date.now(),
      })
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
    await expect(page.getByRole('heading', { name: '对方电脑', exact: true })).toBeVisible()

    await page.evaluate(() => {
      ;(window as any).__emitActiveJob({
          peer_device_id: 22,
          peer_device_uuid: 'remote-uuid',
          state: 'succeeded',
          attempt: 1,
          max_attempts: 30,
          message: '隧道已自动建立成功',
          updated_at: Date.now(),
          result: { success: true, local_port: 0, remote_protocol: 'rdp', rdp_address: '100.99.71.43:3390' },
      })
    })

    await expect(page.locator('.result-section code')).toHaveText('100.99.71.43:3390')
    const hint = page.locator('.result-section .result-hint').nth(1)
    await expect(hint).toContainText('把端口 3390 改成对应服务端口，例如 RustDesk 直连端口为 21121')
    await expect(hint).not.toContainText('gonc.cc')
  })
})
