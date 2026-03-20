import { ref, createApp, h } from 'vue'
import ConfirmDialog from './ConfirmDialog.vue'

export interface ConfirmOptions {
  title?: string
  message: string
  kind?: 'info' | 'warning'
  confirmText?: string
  cancelText?: string
  checkboxLabel?: string
  checkboxChecked?: boolean
}

export interface ConfirmResult {
  confirmed: boolean
  checkboxChecked: boolean
}

export function confirm(options: ConfirmOptions): Promise<ConfirmResult> {
  return new Promise((resolve) => {
    const container = document.createElement('div')
    document.body.appendChild(container)

    const open = ref(true)

    function cleanup() {
      open.value = false
      setTimeout(() => {
        app.unmount()
        container.remove()
      }, 200)
    }

    const app = createApp({
      setup() {
        return () =>
          h(ConfirmDialog, {
            open: open.value,
            'onUpdate:open': (val: boolean) => {
              open.value = val
            },
            title: options.title ?? '',
            message: options.message,
            kind: options.kind ?? 'info',
            confirmText: options.confirmText,
            cancelText: options.cancelText,
            checkboxLabel: options.checkboxLabel ?? '',
            checkboxChecked: options.checkboxChecked ?? false,
            onConfirm: (checkboxChecked: boolean) => {
              resolve({ confirmed: true, checkboxChecked })
              cleanup()
            },
            onCancel: () => {
              resolve({ confirmed: false, checkboxChecked: false })
              cleanup()
            },
          })
      },
    })

    app.mount(container)
  })
}
