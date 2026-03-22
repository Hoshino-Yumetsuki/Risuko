<template>
  <div style="display: none"></div>
</template>

<script lang="ts">
import { invoke } from '@tauri-apps/api/core'
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow'
import { useAppStore } from '@/store/app'
import { ADD_TASK_TYPE } from '@shared/constants'

export default {
  name: 'mo-dragger',
  data() {
    return {
      unlistenWindowDragDrop: null as null | (() => void),
      dragPreviewOpened: false,
    }
  },
  mounted() {
    this.preventDefault = (ev) => ev.preventDefault()
    this.injectTorrentPaths = async (paths: string[] = []) => {
      const torrentPaths = paths.filter((path) => /\.torrent$/i.test(path))
      if (!torrentPaths.length) {
        this.$msg.error(this.$t('task.select-torrent'))
        return
      }

      const fileList: Array<{ raw: File; name: string }> = []
      for (const path of torrentPaths) {
        try {
          const bytes = await invoke<number[]>('read_binary_file', { path })
          const segs = `${path}`.split(/[/\\]/)
          const name = segs[segs.length - 1] || 'task.torrent'
          const uint8 = new Uint8Array(bytes)
          const file = new File([uint8], name, { type: 'application/x-bittorrent' })
          fileList.push({ raw: file, name: file.name })
        } catch {
          // skip broken path and continue remaining files
        }
      }

      if (!fileList.length) {
        this.$msg.error(this.$t('task.select-torrent'))
        return
      }

      useAppStore().showAddTaskDialog(ADD_TASK_TYPE.TORRENT)
      useAppStore().addTaskAddTorrents({ fileList })
    }
    this.handleFileList = (files) => {
      const fileList = (files || [])
        .map((item) => ({ raw: item, name: item.name }))
        .filter((item) => /\.torrent$/i.test(item.name))
      if (!fileList.length) {
        this.$msg.error(this.$t('task.select-torrent'))
        return
      }
      useAppStore().showAddTaskDialog(ADD_TASK_TYPE.TORRENT)
      useAppStore().addTaskAddTorrents({ fileList })
    }
    let count = 0
    this.onDragEnter = (ev) => {
      if (count === 0) {
        this.dragPreviewOpened = !useAppStore().addTaskVisible
        useAppStore().showAddTaskDialog(ADD_TASK_TYPE.TORRENT)
      }
      count++
    }

    this.onDragLeave = (ev) => {
      count = Math.max(0, count - 1)
      if (count === 0 && this.dragPreviewOpened) {
        useAppStore().hideAddTaskDialog()
        this.dragPreviewOpened = false
      }
    }

    this.onDrop = (ev) => {
      count = 0
      this.dragPreviewOpened = false

      this.handleFileList([...(ev.dataTransfer?.files || [])])
    }

    document.addEventListener('dragover', this.preventDefault)
    document.body.addEventListener('dragenter', this.onDragEnter)
    document.body.addEventListener('dragleave', this.onDragLeave)
    document.body.addEventListener('drop', this.onDrop)

    const webview = getCurrentWebviewWindow()
    webview
      .onDragDropEvent((event: any) => {
        if (event?.payload?.type !== 'drop') {
          return
        }
        count = 0
        this.dragPreviewOpened = false
        const paths = event?.payload?.paths || []
        if (!Array.isArray(paths) || paths.length === 0) {
          return
        }
        this.injectTorrentPaths(paths)
      })
      .then((unlisten) => {
        this.unlistenWindowDragDrop = unlisten
      })
      .catch(() => {})
  },
  beforeUnmount() {
    document.removeEventListener('dragover', this.preventDefault)
    document.body.removeEventListener('dragenter', this.onDragEnter)
    document.body.removeEventListener('dragleave', this.onDragLeave)
    document.body.removeEventListener('drop', this.onDrop)
    this.unlistenWindowDragDrop?.()
    this.unlistenWindowDragDrop = null
  },
}
</script>
