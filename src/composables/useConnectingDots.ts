import { computed, onScopeDispose, ref } from 'vue'

/// 打洞进行中的省略号动画：'.' → '..' → '...' 每秒一帧循环。
/// 打洞一次尝试可能长达十几秒且界面无其他变化，滚动省略号
/// 让用户确认客户端没有卡死。
/// 每个消费者各持一个 1Hz 定时器（审计 G-6：全项目仅两处互斥渲染的
/// 消费者，不值得为此维护全局引用计数共享机制），卸载自动清理。
const FRAME_MS = 1000
const FRAMES = ['.', '..', '...'] as const

export function useConnectingDots() {
  const frame = ref(0)
  const timer = setInterval(() => {
    frame.value = (frame.value + 1) % FRAMES.length
  }, FRAME_MS)
  onScopeDispose(() => clearInterval(timer))
  return computed(() => FRAMES[frame.value])
}
