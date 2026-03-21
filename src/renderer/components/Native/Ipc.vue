<template>
  <div style="display: none"></div>
</template>

<script lang="ts">
import { listen } from '@tauri-apps/api/event'
import type { UnlistenFn } from '@tauri-apps/api/event'
import { commands } from '@/components/CommandManager/instance'

export default {
  name: 'mo-ipc',
  data() {
    return {
      unlisten: null as UnlistenFn | null,
      alive: true,
      bindToken: 0,
    }
  },
  methods: {
    async bindIpcEvents() {
      const token = ++this.bindToken
      const unlisten = await listen('command', (event) => {
        const payload = event.payload as any
        if (payload && payload.command) {
          commands.execute(payload.command, ...(payload.args || []))
        } else if (typeof payload === 'string') {
          commands.execute(payload)
        }
      })

      if (!this.alive || token !== this.bindToken) {
        unlisten()
        return
      }

      this.unlisten = unlisten
    },
    unbindIpcEvents() {
      if (this.unlisten) {
        this.unlisten()
        this.unlisten = null
      }
    },
  },
  created() {
    this.bindIpcEvents().catch(() => {})
  },
  beforeUnmount() {
    this.alive = false
    this.bindToken += 1
    this.unbindIpcEvents()
  },
}
</script>
