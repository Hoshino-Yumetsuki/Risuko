import { toast } from 'vue-sonner'

const queue: Array<() => void> = []
const maxLength = 5
let activeToasts = 0

export default {
  install: function (target, defaultOption = {}) {
    const globals = target?.config?.globalProperties || target?.prototype
    if (!globals) {
      return
    }

    const handler = {
      success: (arg) => showToast('success', arg, defaultOption),
      error: (arg) => showToast('error', arg, defaultOption),
      warning: (arg) => showToast('warning', arg, defaultOption),
      info: (arg) => showToast('info', arg, defaultOption),
    }

    globals.$msg = new Proxy(handler, {
      get(obj, prop: string) {
        if (prop in obj) {
          return obj[prop]
        }
        return (arg) => showToast('info', arg, defaultOption)
      },
    })
  },
}

function showToast(type: string, arg, defaultOption) {
  if (!(arg instanceof Object)) {
    arg = { message: arg }
  }

  const merged = { ...defaultOption, ...arg }
  const message = merged.message || ''
  const duration = merged.duration || 3000

  const task = () => {
    switch (type) {
      case 'success':
        toast.success(message, { duration })
        break
      case 'error':
        toast.error(message, { duration })
        break
      case 'warning':
        toast.warning(message, { duration })
        break
      default:
        toast.info(message, { duration })
    }

    const wait = Number(duration) || 3000
    setTimeout(() => {
      activeToasts = Math.max(0, activeToasts - 1)
      flushQueue()
    }, wait)
  }

  if (queue.length >= maxLength) {
    queue.shift()
  }
  queue.push(task)

  flushQueue()
}

function flushQueue() {
  while (activeToasts < maxLength && queue.length > 0) {
    const task = queue.shift()
    if (!task) {
      break
    }
    activeToasts += 1
    task()
  }
}
