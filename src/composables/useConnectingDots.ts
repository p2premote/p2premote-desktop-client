import { computed, onScopeDispose, ref } from 'vue'

/// 打洞进行中的省略号动画：'.' → '..' → '...' 每秒一帧循环。
/// 打洞一次尝试可能长达十几秒且界面无其他变化，滚动省略号
/// 让用户确认客户端没有卡死。
const FRAME_MS = 1000
const FRAMES = ['.', '..', '...'] as const

const frame = ref(0)
let timer: ReturnType<typeof setInterval> | null = null
let consumers = 0

function acquireTicker() {
  consumers += 1
  if (timer === null) {
    timer = setInterval(() => {
      frame.value = (frame.value + 1) % FRAMES.length
    }, FRAME_MS)
  }
}

function releaseTicker() {
  consumers = Math.max(0, consumers - 1)
  if (consumers === 0 && timer !== null) {
    clearInterval(timer)
    timer = null
  }
}

/// 返回当前帧的省略号字符串。多处同时使用时共享同一个定时器，
/// 组件卸载后自动停止，不残留后台计时。
export function useConnectingDots() {
  acquireTicker()
  onScopeDispose(releaseTicker)
  return computed(() => FRAMES[frame.value])
}
