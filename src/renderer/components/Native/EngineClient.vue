<template>
  <div style="display: none"></div>
</template>

<script lang="ts">
import { invoke } from '@tauri-apps/api/core'
import { block, NoSleepType, unblock } from 'tauri-plugin-nosleep-api'
import logger from '@shared/utils/logger'
import { useAppStore } from '@/store/app'
import { useTaskStore } from '@/store/task'
import { usePreferenceStore } from '@/store/preference'
import api from '@/api'
import { finalizeCompletedDownloadPath, showItemInFolder } from '@/utils/native'
import { checkTaskIsBT, getTaskName, parseBooleanConfig } from '@shared/utils'
import { TASK_STATUS } from '@shared/constants'

const RETRY_STRATEGY_STATIC = 'static'
const RETRY_STRATEGY_EXPONENTIAL = 'exponential'
const AUTO_RETRY_MAX_DELAY_MS = 15 * 60 * 1000
const LOW_SPEED_STRIKE_THRESHOLD = 3
const LOW_SPEED_RECOVERY_COOLDOWN_MS = 30 * 1000
const LOW_SPEED_RESTART_WAIT_MS = 500

const normalizePositiveNumber = (
  value: any,
  fallback: number,
  min = 0,
  max = Number.MAX_SAFE_INTEGER,
) => {
  const parsed = Number(value)
  if (!Number.isFinite(parsed)) {
    return fallback
  }
  return Math.min(Math.max(parsed, min), max)
}

export default {
  name: 'mo-engine-client',
  data() {
    return {
      timer: null,
      initTimer: null,
      isPolling: false,
      isDestroyed: false,
      noSleepDesired: false,
      noSleepApplied: false,
      noSleepSource: null as null | 'plugin' | 'rust',
      noSleepSyncing: false,
      noSleepResyncNeeded: false,
      startupAutoResumeHandled: false,
      autoRetryTimerMap: {} as Record<string, ReturnType<typeof setTimeout>>,
      autoRetryAttemptMap: {} as Record<string, number>,
      lowSpeedStrikeMap: {} as Record<string, number>,
      lowSpeedRecoverAtMap: {} as Record<string, number>,
      lowSpeedRecoveringMap: {} as Record<string, boolean>,
    }
  },
  computed: {
    uploadSpeed() {
      return useAppStore().stat.uploadSpeed
    },
    downloadSpeed() {
      return useAppStore().stat.downloadSpeed
    },
    speed() {
      return useAppStore().stat.uploadSpeed + useAppStore().stat.downloadSpeed
    },
    interval() {
      return useAppStore().interval
    },
    downloading() {
      return useAppStore().stat.numActive > 0
    },
    progress() {
      return useAppStore().progress
    },
    messages() {
      return (useTaskStore() as any).messages
    },
    seedingList() {
      return useTaskStore().seedingList
    },
    taskDetailVisible() {
      return useTaskStore().taskDetailVisible
    },
    enabledFetchPeers() {
      return useTaskStore().enabledFetchPeers
    },
    currentTaskGid() {
      return useTaskStore().currentTaskGid
    },
    currentTaskItem() {
      return useTaskStore().currentTaskItem
    },
    taskNotification() {
      return (usePreferenceStore().config as any).taskNotification
    },
    traySpeedometer() {
      return parseBooleanConfig((usePreferenceStore().config as any).traySpeedometer)
    },
    showProgressBar() {
      return parseBooleanConfig((usePreferenceStore().config as any).showProgressBar)
    },
    resumeAllWhenAppLaunched() {
      return parseBooleanConfig((usePreferenceStore().config as any).resumeAllWhenAppLaunched)
    },
    autoRetryEnabled() {
      return parseBooleanConfig((usePreferenceStore().config as any).autoRetry)
    },
    autoRetryStrategy() {
      const value = (usePreferenceStore().config as any).autoRetryStrategy
      return value === RETRY_STRATEGY_EXPONENTIAL
        ? RETRY_STRATEGY_EXPONENTIAL
        : RETRY_STRATEGY_STATIC
    },
    autoRetryIntervalSeconds() {
      const raw = (usePreferenceStore().config as any).autoRetryInterval
      return Math.floor(normalizePositiveNumber(raw, 5, 1, 300))
    },
    autoDetectLowSpeedTasks() {
      return parseBooleanConfig((usePreferenceStore().config as any).autoDetectLowSpeedTasks)
    },
    lowSpeedThresholdBytes() {
      const raw = (usePreferenceStore().config as any).lowSpeedThreshold
      const kb = normalizePositiveNumber(raw, 20, 1, 10240)
      return Math.floor(kb * 1024)
    },
    currentTaskIsBT() {
      return checkTaskIsBT(this.currentTaskItem)
    },
  },
  watch: {
    speed() {
      this.syncTraySpeedTooltip()
    },
    traySpeedometer(val) {
      this.syncTraySpeedTooltip(!!val)
    },
    downloading(val, oldVal) {
      if (val !== oldVal) {
        this.noSleepDesired = !!val
        this.syncNoSleepState()
      }
    },
    progress(val) {
      invoke('on_progress_change', {
        progress: val,
        showProgressBar: this.showProgressBar,
      }).catch(() => {})
    },
    showProgressBar(val) {
      invoke('on_progress_change', {
        progress: this.progress,
        showProgressBar: parseBooleanConfig(val),
      }).catch(() => {})
    },
    interval() {
      if (this.timer) {
        this.startPolling()
      }
    },
    autoRetryEnabled(val) {
      if (val) {
        return
      }
      this.clearAllAutoRetryTimers()
    },
    autoDetectLowSpeedTasks(val) {
      if (val) {
        return
      }
      this.lowSpeedStrikeMap = {}
      this.lowSpeedRecoverAtMap = {}
      this.lowSpeedRecoveringMap = {}
    },
  },
  methods: {
    clearAutoRetryTimer(gid: string) {
      const timer = this.autoRetryTimerMap[gid]
      if (timer) {
        clearTimeout(timer)
      }
      delete this.autoRetryTimerMap[gid]
    },
    clearAutoRetryState(gid: string) {
      this.clearAutoRetryTimer(gid)
      delete this.autoRetryAttemptMap[gid]
    },
    clearAllAutoRetryTimers() {
      for (const gid of Object.keys(this.autoRetryTimerMap)) {
        this.clearAutoRetryTimer(gid)
      }
      this.autoRetryAttemptMap = {}
    },
    scheduleAutoRetry(gid: string) {
      if (!this.autoRetryEnabled || !gid) {
        return
      }

      this.clearAutoRetryTimer(gid)

      const nextAttempt = (this.autoRetryAttemptMap[gid] || 0) + 1
      this.autoRetryAttemptMap[gid] = nextAttempt

      const baseDelayMs = this.autoRetryIntervalSeconds * 1000
      const computedDelayMs =
        this.autoRetryStrategy === RETRY_STRATEGY_EXPONENTIAL
          ? baseDelayMs * 2 ** (nextAttempt - 1)
          : baseDelayMs
      const delayMs = Math.min(Math.max(computedDelayMs, 1000), AUTO_RETRY_MAX_DELAY_MS)

      this.autoRetryTimerMap[gid] = setTimeout(() => {
        const taskStore = useTaskStore()
        taskStore
          .resumeTask({ gid })
          .then(() => {
            logger.info(
              `[Motrix] auto retry resume requested for gid: ${gid}, attempt: ${nextAttempt}`,
            )
          })
          .catch((err) => {
            logger.warn(
              `[Motrix] auto retry resume failed for gid: ${gid}, attempt: ${nextAttempt}, ${err?.message || err}`,
            )
          })
          .finally(() => {
            delete this.autoRetryTimerMap[gid]
          })
      }, delayMs)
    },
    resetLowSpeedStateForGid(gid: string) {
      delete this.lowSpeedStrikeMap[gid]
      delete this.lowSpeedRecoverAtMap[gid]
      delete this.lowSpeedRecoveringMap[gid]
    },
    async recoverLowSpeedTask(gid: string) {
      if (!gid || this.lowSpeedRecoveringMap[gid]) {
        return
      }

      this.lowSpeedRecoveringMap[gid] = true
      try {
        await api.forcePauseTask({ gid })
        await new Promise((resolve) => setTimeout(resolve, LOW_SPEED_RESTART_WAIT_MS))
        await api.resumeTask({ gid })
        useTaskStore().saveSession()
        logger.info(`[Motrix] auto recovered low speed task: ${gid}`)
      } catch (err) {
        logger.warn(`[Motrix] auto recover low speed task failed: ${gid}, ${err?.message || err}`)
      } finally {
        this.lowSpeedRecoveringMap[gid] = false
      }
    },
    handleLowSpeedTasks(tasks: any[] = []) {
      if (!this.autoDetectLowSpeedTasks) {
        return
      }

      const now = Date.now()
      const activeGids = new Set<string>()

      for (const task of tasks) {
        const gid = `${task?.gid || ''}`
        if (!gid || task?.status !== TASK_STATUS.ACTIVE) {
          continue
        }

        activeGids.add(gid)

        const speed = normalizePositiveNumber(task?.downloadSpeed, 0, 0)
        if (speed >= this.lowSpeedThresholdBytes) {
          this.resetLowSpeedStateForGid(gid)
          continue
        }

        const strike = (this.lowSpeedStrikeMap[gid] || 0) + 1
        this.lowSpeedStrikeMap[gid] = strike

        if (strike < LOW_SPEED_STRIKE_THRESHOLD) {
          continue
        }
        if ((this.lowSpeedRecoverAtMap[gid] || 0) > now) {
          continue
        }

        this.lowSpeedStrikeMap[gid] = 0
        this.lowSpeedRecoverAtMap[gid] = now + LOW_SPEED_RECOVERY_COOLDOWN_MS
        this.recoverLowSpeedTask(gid)
      }

      const knownGids = new Set([
        ...Object.keys(this.lowSpeedStrikeMap),
        ...Object.keys(this.lowSpeedRecoverAtMap),
      ])
      for (const gid of knownGids) {
        if (!activeGids.has(gid)) {
          this.resetLowSpeedStateForGid(gid)
        }
      }
    },
    getTraySpeedLabelPayload() {
      return {
        appName: this.$t('menu.app'),
        downloadLabel: this.$t('task.task-download-speed'),
        uploadLabel: this.$t('task.task-upload-speed'),
      }
    },
    syncTraySpeedTooltip(showTraySpeed = this.traySpeedometer) {
      const { appName, downloadLabel, uploadLabel } = this.getTraySpeedLabelPayload()
      invoke('on_speed_change', {
        uploadSpeed: this.uploadSpeed,
        downloadSpeed: this.downloadSpeed,
        showTraySpeed: !!showTraySpeed,
        appName,
        downloadLabel,
        uploadLabel,
      }).catch(() => {})
    },
    async setNoSleepState(downloading: boolean) {
      if (!downloading) {
        if (this.noSleepSource === 'plugin') {
          try {
            await unblock()
            this.noSleepSource = null
            return true
          } catch {
            return false
          }
        }

        if (this.noSleepSource === 'rust') {
          try {
            await invoke('on_download_status_change', { downloading: false })
            this.noSleepSource = null
            return true
          } catch {
            return false
          }
        }

        return true
      }

      try {
        await block(NoSleepType.PreventUserIdleSystemSleep)
        this.noSleepSource = 'plugin'
        return true
      } catch {
        // Keep Rust-side fallback for environments where the plugin command is unavailable.
        try {
          await invoke('on_download_status_change', { downloading })
          this.noSleepSource = 'rust'
          return true
        } catch {
          return false
        }
      }
    },
    async syncNoSleepState() {
      if (this.noSleepSyncing) {
        this.noSleepResyncNeeded = true
        return
      }

      this.noSleepSyncing = true
      do {
        this.noSleepResyncNeeded = false
        const target = this.noSleepDesired
        if (target === this.noSleepApplied) continue

        const ok = await this.setNoSleepState(target)
        if (ok) {
          this.noSleepApplied = target
        } else {
          // Stop retry loop on hard failure; next state transition will retry.
          break
        }
      } while (this.noSleepResyncNeeded)
      this.noSleepSyncing = false

      if (this.noSleepResyncNeeded) {
        // Catch race where another update arrived right after loop exit.
        this.syncNoSleepState()
      }
    },
    async fetchTaskItem({ gid }) {
      return api.fetchTaskItem({ gid }).catch((e) => {
        logger.warn(`fetchTaskItem fail: ${e.message}`)
      })
    },
    onDownloadStart(event) {
      const taskStore = useTaskStore()
      taskStore.fetchList()
      useAppStore().resetInterval()
      taskStore.saveSession()
      const [{ gid }] = event
      this.clearAutoRetryState(gid)
      const { seedingList } = this
      if (seedingList.includes(gid)) return

      this.fetchTaskItem({ gid }).then((task) => {
        if (!task) return
        const { dir } = task
        usePreferenceStore().recordHistoryDirectory(dir)
        const taskName = getTaskName(task)
        const message = this.$t('task.download-start-message', { taskName })
        this.$msg.info(message)
      })
    },
    onDownloadPause(event) {
      const [{ gid }] = event
      this.clearAutoRetryState(gid)
      const { seedingList } = this
      if (seedingList.includes(gid)) return

      this.fetchTaskItem({ gid }).then((task) => {
        if (!task) return
        const taskName = getTaskName(task)
        const message = this.$t('task.download-pause-message', { taskName })
        this.$msg.info(message)
      })
    },
    onDownloadStop(event) {
      const [{ gid }] = event
      this.clearAutoRetryState(gid)
      this.fetchTaskItem({ gid }).then((task) => {
        if (!task) return
        const taskName = getTaskName(task)
        const message = this.$t('task.download-stop-message', { taskName })
        this.$msg.info(message)
      })
    },
    onDownloadError(event) {
      const [{ gid }] = event
      this.scheduleAutoRetry(gid)
      this.fetchTaskItem({ gid }).then((task) => {
        if (!task) return
        const taskName = getTaskName(task)
        const { errorCode, errorMessage } = task
        logger.error(`[Motrix] download error gid: ${gid}, #${errorCode}, ${errorMessage}`)
        const message = this.$t('task.download-error-message', { taskName })
        const link = `https://github.com/agalwood/Motrix/wiki/Error#${errorCode}`
        this.$msg.error({
          duration: 5000,
          message: `${message} (${errorCode}) ${link}`,
        })
      })
    },
    onDownloadComplete(event) {
      const taskStore = useTaskStore()
      taskStore.fetchList()
      const [{ gid }] = event
      this.clearAutoRetryState(gid)
      taskStore.removeFromSeedingList(gid)

      this.fetchTaskItem({ gid }).then((task) => {
        if (!task) return
        this.handleDownloadComplete(task, false)
      })
    },
    onBtDownloadComplete(event) {
      const taskStore = useTaskStore()
      taskStore.fetchList()
      const [{ gid }] = event
      this.clearAutoRetryState(gid)
      const { seedingList } = this
      if (seedingList.includes(gid)) return

      taskStore.addToSeedingList(gid)

      this.fetchTaskItem({ gid }).then((task) => {
        if (!task) return
        this.handleDownloadComplete(task, true)
      })
    },
    async handleDownloadComplete(task, isBT) {
      useTaskStore().saveSession()

      const path = await finalizeCompletedDownloadPath(task)
      this.showTaskCompleteNotify(task, isBT, path)
      invoke('on_task_download_complete', { path }).catch(() => {})
    },
    showTaskCompleteNotify(task, isBT, path) {
      const taskName = getTaskName(task)
      const message = isBT
        ? this.$t('task.bt-download-complete-message', { taskName })
        : this.$t('task.download-complete-message', { taskName })
      const tips = isBT ? '\n' + this.$t('task.bt-download-complete-tips') : ''

      this.$msg.success(`${message}${tips}`)

      if (!this.taskNotification) return

      const notifyMessage = isBT
        ? this.$t('task.bt-download-complete-notify')
        : this.$t('task.download-complete-notify')

      const notify = new Notification(notifyMessage, {
        body: `${taskName}${tips}`,
      })
      notify.onclick = () => {
        showItemInFolder(path, {
          errorMsg: this.$t('task.file-not-exist'),
        })
      }
    },
    showTaskErrorNotify(task) {
      const taskName = getTaskName(task)
      const message = this.$t('task.download-fail-message', { taskName })
      this.$msg.success(message)

      if (!this.taskNotification) return

      // eslint-disable-next-line no-new
      new Notification(this.$t('task.download-fail-notify'), {
        body: taskName,
      })
    },
    bindEngineEvents() {
      if (!api.client) return
      api.client.on('onDownloadStart', this.onDownloadStart)
      api.client.on('onDownloadStop', this.onDownloadStop)
      api.client.on('onDownloadComplete', this.onDownloadComplete)
      api.client.on('onDownloadError', this.onDownloadError)
      api.client.on('onBtDownloadComplete', this.onBtDownloadComplete)
    },
    unbindEngineEvents() {
      if (!api.client) return
      api.client.removeListener('onDownloadStart', this.onDownloadStart)
      api.client.removeListener('onDownloadStop', this.onDownloadStop)
      api.client.removeListener('onDownloadComplete', this.onDownloadComplete)
      api.client.removeListener('onDownloadError', this.onDownloadError)
      api.client.removeListener('onBtDownloadComplete', this.onBtDownloadComplete)
    },
    startPolling() {
      this.stopPolling()

      const loop = async () => {
        await this.polling()
        if (!this.isDestroyed) {
          this.timer = setTimeout(loop, this.interval)
        }
      }

      this.timer = setTimeout(loop, this.interval)
    },
    async polling() {
      if (this.isPolling) return
      this.isPolling = true

      try {
        const jobs: Array<Promise<any>> = [
          useAppStore().fetchGlobalStat(),
          useAppStore().fetchProgress(),
        ]
        let activeTasksForLowSpeedCheck: any[] = []

        if (!document.hidden || this.taskDetailVisible) {
          jobs.push(useTaskStore().fetchList())
        }

        if (this.autoDetectLowSpeedTasks) {
          jobs.push(
            api
              .fetchActiveTaskList({
                keys: ['gid', 'status', 'downloadSpeed'],
              })
              .then((tasks) => {
                activeTasksForLowSpeedCheck = Array.isArray(tasks) ? tasks : []
              })
              .catch((err) => {
                logger.warn(
                  '[Motrix] low speed detection fetch active tasks failed:',
                  err?.message || err,
                )
              }),
          )
        }

        if (this.taskDetailVisible && this.currentTaskGid) {
          if (this.currentTaskIsBT && this.enabledFetchPeers) {
            jobs.push(useTaskStore().fetchItemWithPeers(this.currentTaskGid))
          } else {
            jobs.push(useTaskStore().fetchItem(this.currentTaskGid))
          }
        }

        await Promise.allSettled(jobs)
        if (this.autoDetectLowSpeedTasks) {
          this.handleLowSpeedTasks(activeTasksForLowSpeedCheck)
        }
      } finally {
        this.isPolling = false
      }
    },
    stopPolling() {
      clearTimeout(this.timer)
      this.timer = null
    },
    autoResumeUnfinishedTasksOnLaunch() {
      if (this.startupAutoResumeHandled) {
        return
      }
      this.startupAutoResumeHandled = true

      if (!this.resumeAllWhenAppLaunched) {
        return
      }

      useTaskStore()
        .resumeAllTask()
        .catch((err) => {
          logger.warn('[Motrix] auto resume unfinished tasks failed:', err?.message || err)
        })
    },
  },
  created() {
    api
      .ensureReady()
      .then(() => {
        this.bindEngineEvents()
      })
      .catch((err) => {
        logger.warn('[Motrix] bindEngineEvents failed:', err.message)
      })
  },
  mounted() {
    this.initTimer = setTimeout(() => {
      const appStore = useAppStore()
      appStore.fetchEngineInfo()
      appStore.fetchEngineOptions()
      this.autoResumeUnfinishedTasksOnLaunch()
      this.syncTraySpeedTooltip()

      this.startPolling()
    }, 100)
  },
  beforeUnmount() {
    this.isDestroyed = true
    useTaskStore().saveSession()
    clearTimeout(this.initTimer)
    this.initTimer = null

    this.unbindEngineEvents()
    this.stopPolling()
    this.clearAllAutoRetryTimers()

    // Best effort release in case component is torn down while downloads were active.
    this.noSleepDesired = false
    this.syncNoSleepState()
  },
}
</script>
