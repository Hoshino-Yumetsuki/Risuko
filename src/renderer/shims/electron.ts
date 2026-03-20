import { invoke } from '@tauri-apps/api/core'
import { listen as tauriListen } from '@tauri-apps/api/event'
import type { UnlistenFn } from '@tauri-apps/api/event'

type Callback = (...args: any[]) => void

const listeners: Map<string, UnlistenFn> = new Map()

const ipcRenderer = {
  send(channel: string, ...args: any[]) {
    if (channel === 'command') {
      const [command, ...rest] = args
      invoke(command.replace(/:/g, '_').replace('application_', ''), rest[0]).catch((err) => {
        console.warn('[Motrix] invoke failed:', command, err)
      })
    } else if (channel === 'event') {
      const [eventName, ...rest] = args
      const cmdName = eventName.replace(/-/g, '_')
      invoke(`on_${cmdName}`, rest[0]).catch((err) => {
        console.warn('[Motrix] event invoke failed:', eventName, err)
      })
    }
  },
  async invoke(channel: string, args?: Record<string, unknown>) {
    return invoke(channel.replace(/-/g, '_'), args)
  },
  on(channel: string, callback: Callback) {
    tauriListen(channel, (event) => {
      callback(event, ...(Array.isArray(event.payload) ? event.payload : [event.payload]))
    }).then((unlisten) => {
      listeners.set(`${channel}_${callback.toString()}`, unlisten)
    })
  },
  once(channel: string, callback: Callback) {
    let fired = false
    let unlistenFn: UnlistenFn | null = null
    tauriListen(channel, (event) => {
      if (fired) return
      fired = true
      callback(event, ...(Array.isArray(event.payload) ? event.payload : [event.payload]))
      if (unlistenFn) {
        unlistenFn()
        listeners.delete(`${channel}_once`)
      }
    }).then((unlisten) => {
      unlistenFn = unlisten
      if (fired) {
        unlisten()
      } else {
        listeners.set(`${channel}_once`, unlisten)
      }
    })
  },
  removeListener(channel: string, _callback: Callback) {
    for (const [key, unlisten] of listeners) {
      if (key.startsWith(channel)) {
        unlisten()
        listeners.delete(key)
      }
    }
  },
}

export { ipcRenderer }
export default { ipcRenderer }
