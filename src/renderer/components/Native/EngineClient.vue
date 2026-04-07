<template>
  <div style="display: none"></div>
</template>

<script lang="ts">
import type { DownloadTask } from "@shared/types/task";
import { checkTaskIsBT, getTaskName, parseBooleanConfig } from "@shared/utils";
import logger from "@shared/utils/logger";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { block, NoSleepType, unblock } from "tauri-plugin-nosleep-api";
import api from "@/api";
import { useAppStore } from "@/store/app";
import { usePreferenceStore } from "@/store/preference";
import { useTaskStore } from "@/store/task";
import {
	finalizeCompletedDownloadPath,
	protectDownloadFile,
	showItemInFolder,
	unprotectDownloadFile,
} from "@/utils/native";

const RETRY_STRATEGY_STATIC = "static";
const RETRY_STRATEGY_EXPONENTIAL = "exponential";
const AUTO_RETRY_MAX_DELAY_MS = 15 * 60 * 1000;
const LOW_SPEED_STRIKE_THRESHOLD = 3;
const LOW_SPEED_RECOVERY_COOLDOWN_MS = 30 * 1000;
const LOW_SPEED_RESTART_WAIT_MS = 500;

const normalizePositiveNumber = (
	value: unknown,
	fallback: number,
	min = 0,
	max = Number.MAX_SAFE_INTEGER,
) => {
	const parsed = Number(value);
	if (!Number.isFinite(parsed)) {
		return fallback;
	}
	return Math.min(Math.max(parsed, min), max);
};

export default {
	name: "mo-engine-client",
	data() {
		return {
			timer: null,
			initTimer: null,
			isPolling: false,
			isDestroyed: false,
			noSleepDesired: false,
			noSleepApplied: false,
			noSleepSource: null as null | "plugin" | "rust",
			noSleepSyncing: false,
			noSleepResyncNeeded: false,
			startupAutoResumeHandled: false,
			autoRetryTimerMap: {} as Record<string, ReturnType<typeof setTimeout>>,
			autoRetryAttemptMap: {} as Record<string, number>,
			autoRetryPlanTicketMap: {} as Record<string, number>,
			lowSpeedStrikeMap: {} as Record<string, number>,
			lowSpeedRecoverAtMap: {} as Record<string, number>,
			lowSpeedRecoveringMap: {} as Record<string, boolean>,
			eventUnlisteners: [] as UnlistenFn[],
		};
	},
	computed: {
		uploadSpeed() {
			return useAppStore().stat.uploadSpeed;
		},
		downloadSpeed() {
			return useAppStore().stat.downloadSpeed;
		},
		speed() {
			return useAppStore().stat.uploadSpeed + useAppStore().stat.downloadSpeed;
		},
		interval() {
			return useAppStore().interval;
		},
		downloading() {
			return useAppStore().stat.numActive > 0;
		},
		progress() {
			return useAppStore().progress;
		},
		messages() {
			return (useTaskStore() as unknown as Record<string, unknown>).messages;
		},
		seedingList() {
			return useTaskStore().seedingList;
		},
		taskDetailVisible() {
			return useTaskStore().taskDetailVisible;
		},
		enabledFetchPeers() {
			return useTaskStore().enabledFetchPeers;
		},
		currentTaskGid() {
			return useTaskStore().currentTaskGid;
		},
		currentTaskItem() {
			return useTaskStore().currentTaskItem;
		},
		taskNotification() {
			return usePreferenceStore().config.taskNotification;
		},
		traySpeedometer() {
			return parseBooleanConfig(usePreferenceStore().config.traySpeedometer);
		},
		showProgressBar() {
			return parseBooleanConfig(usePreferenceStore().config.showProgressBar);
		},
		resumeAllWhenAppLaunched() {
			return parseBooleanConfig(
				usePreferenceStore().config.resumeAllWhenAppLaunched,
			);
		},
		autoRetryEnabled() {
			return parseBooleanConfig(usePreferenceStore().config.autoRetry);
		},
		autoRetryStrategy() {
			const value = usePreferenceStore().config.autoRetryStrategy;
			return value === RETRY_STRATEGY_EXPONENTIAL
				? RETRY_STRATEGY_EXPONENTIAL
				: RETRY_STRATEGY_STATIC;
		},
		autoRetryIntervalSeconds() {
			const raw = usePreferenceStore().config.autoRetryInterval;
			return Math.floor(normalizePositiveNumber(raw, 5, 1, 300));
		},
		autoDetectLowSpeedTasks() {
			return parseBooleanConfig(
				usePreferenceStore().config.autoDetectLowSpeedTasks,
			);
		},
		lowSpeedThresholdBytes() {
			const raw = usePreferenceStore().config.lowSpeedThreshold;
			const kb = normalizePositiveNumber(raw, 20, 1, 10240);
			return Math.floor(kb * 1024);
		},
		currentTaskIsBT() {
			return checkTaskIsBT(this.currentTaskItem);
		},
	},
	watch: {
		speed() {
			this.syncTraySpeedTooltip();
		},
		traySpeedometer(val) {
			this.syncTraySpeedTooltip(!!val);
		},
		downloading(val, oldVal) {
			if (val !== oldVal) {
				this.noSleepDesired = !!val;
				this.syncNoSleepState();
			}
		},
		progress(val) {
			invoke("on_progress_change", {
				progress: val,
				showProgressBar: this.showProgressBar,
			}).catch(() => {
				/* noop */
			});
		},
		showProgressBar(val) {
			invoke("on_progress_change", {
				progress: this.progress,
				showProgressBar: parseBooleanConfig(val),
			}).catch(() => {
				/* noop */
			});
		},
		interval() {
			if (this.timer) {
				this.startPolling();
			}
		},
		autoRetryEnabled(val) {
			if (val) {
				return;
			}
			this.clearAllAutoRetryTimers();
		},
		autoDetectLowSpeedTasks(val) {
			if (val) {
				return;
			}
			this.lowSpeedStrikeMap = {};
			this.lowSpeedRecoverAtMap = {};
			this.lowSpeedRecoveringMap = {};
		},
	},
	methods: {
		clearAutoRetryTimer(gid: string) {
			const timer = this.autoRetryTimerMap[gid];
			if (timer) {
				clearTimeout(timer);
			}
			delete this.autoRetryTimerMap[gid];
		},
		clearAutoRetryState(gid: string) {
			this.clearAutoRetryTimer(gid);
			delete this.autoRetryAttemptMap[gid];
			delete this.autoRetryPlanTicketMap[gid];
		},
		clearAllAutoRetryTimers() {
			for (const gid of Object.keys(this.autoRetryTimerMap)) {
				this.clearAutoRetryTimer(gid);
			}
			this.autoRetryAttemptMap = {};
			this.autoRetryPlanTicketMap = {};
		},
		normalizeAutoRetryAttemptMap(raw: unknown = {}) {
			const result: Record<string, number> = {};
			if (!raw || typeof raw !== "object") {
				return result;
			}

			for (const [gid, value] of Object.entries(raw)) {
				const key = `${gid || ""}`.trim();
				if (!key) {
					continue;
				}

				result[key] = Math.floor(
					normalizePositiveNumber(value, 0, 0, Number.MAX_SAFE_INTEGER),
				);
			}
			return result;
		},
		async scheduleAutoRetry(gid: string) {
			const taskGid = `${gid || ""}`.trim();
			if (!this.autoRetryEnabled || !taskGid) {
				return;
			}

			this.clearAutoRetryTimer(taskGid);
			const ticket = (this.autoRetryPlanTicketMap[taskGid] || 0) + 1;
			this.autoRetryPlanTicketMap[taskGid] = ticket;

			let nextAttempt = 1;
			let delayMs = 1000;

			try {
				const result = await api.planAutoRetry({
					gid: taskGid,
					strategy: this.autoRetryStrategy,
					intervalSeconds: this.autoRetryIntervalSeconds,
					maxDelayMs: AUTO_RETRY_MAX_DELAY_MS,
					attemptMap: this.autoRetryAttemptMap,
				});

				if (this.autoRetryPlanTicketMap[taskGid] !== ticket) {
					return;
				}

				const plannedAttemptMap = this.normalizeAutoRetryAttemptMap(
					result?.attemptMap,
				);
				const plannedAttempt = Number(result?.nextAttempt);
				const plannedDelay = Number(result?.delayMs);
				if (
					!Number.isFinite(plannedAttempt) ||
					plannedAttempt <= 0 ||
					!Number.isFinite(plannedDelay) ||
					plannedDelay <= 0
				) {
					if (this.autoRetryPlanTicketMap[taskGid] === ticket) {
						delete this.autoRetryPlanTicketMap[taskGid];
					}
					logger.warn(
						"[Motrix] planAutoRetry produced invalid plan, skip scheduling",
						result,
					);
					return;
				}

				const safeAttempt = Math.floor(
					normalizePositiveNumber(
						plannedAttemptMap[taskGid],
						plannedAttempt,
						1,
						Number.MAX_SAFE_INTEGER,
					),
				);
				// Only update the planned gid to avoid clobbering concurrent retry updates for other tasks.
				this.autoRetryAttemptMap = {
					...this.autoRetryAttemptMap,
					[taskGid]: safeAttempt,
				};
				nextAttempt = safeAttempt;
				delayMs = Math.min(
					Math.max(Math.floor(plannedDelay), 1000),
					AUTO_RETRY_MAX_DELAY_MS,
				);
			} catch (err) {
				if (this.autoRetryPlanTicketMap[taskGid] !== ticket) {
					return;
				}

				delete this.autoRetryPlanTicketMap[taskGid];
				logger.warn("[Motrix] planAutoRetry failed:", err?.message || err);
				return;
			}

			if (
				!this.autoRetryEnabled ||
				this.autoRetryPlanTicketMap[taskGid] !== ticket
			) {
				return;
			}

			this.autoRetryTimerMap[taskGid] = setTimeout(() => {
				const taskStore = useTaskStore();
				taskStore
					.resumeTask({ gid: taskGid } as DownloadTask)
					.then(() => {
						logger.info(
							`[Motrix] auto retry resume requested for gid: ${taskGid}, attempt: ${nextAttempt}`,
						);
					})
					.catch((err) => {
						logger.warn(
							`[Motrix] auto retry resume failed for gid: ${taskGid}, attempt: ${nextAttempt}, ${err?.message || err}`,
						);
					})
					.finally(() => {
						delete this.autoRetryTimerMap[taskGid];
						if (this.autoRetryPlanTicketMap[taskGid] === ticket) {
							delete this.autoRetryPlanTicketMap[taskGid];
						}
					});
			}, delayMs);
		},
		syncLowSpeedRecoveringMapWithState() {
			const stateGids = new Set([
				...Object.keys(this.lowSpeedStrikeMap),
				...Object.keys(this.lowSpeedRecoverAtMap),
			]);
			for (const gid of Object.keys(this.lowSpeedRecoveringMap)) {
				if (!stateGids.has(gid)) {
					delete this.lowSpeedRecoveringMap[gid];
				}
			}
		},
		async recoverLowSpeedTask(gid: string) {
			if (!gid || this.lowSpeedRecoveringMap[gid]) {
				return;
			}

			this.lowSpeedRecoveringMap[gid] = true;
			try {
				await api.forcePauseTask({ gid });
				await new Promise((resolve) =>
					setTimeout(resolve, LOW_SPEED_RESTART_WAIT_MS),
				);
				await api.resumeTask({ gid });
				useTaskStore().saveSession();
				logger.info(`[Motrix] auto recovered low speed task: ${gid}`);
			} catch (err) {
				logger.warn(
					`[Motrix] auto recover low speed task failed: ${gid}, ${err?.message || err}`,
				);
			} finally {
				this.lowSpeedRecoveringMap[gid] = false;
			}
		},
		async handleLowSpeedTasks(
			tasks: Pick<
				DownloadTask,
				"gid" | "status" | "downloadSpeed" | "seeder"
			>[] = [],
		) {
			if (!this.autoDetectLowSpeedTasks) {
				return;
			}

			try {
				const result = await api.evaluateLowSpeedTasks({
					tasks: tasks.map((task) => {
						const isSeedingTask =
							task?.seeder === "true" || String(task?.seeder) === "true";
						return {
							gid: `${task?.gid || ""}`,
							status: isSeedingTask ? "seeding" : `${task?.status || ""}`,
							downloadSpeed: task?.downloadSpeed,
						};
					}) as DownloadTask[],
					thresholdBytes: this.lowSpeedThresholdBytes,
					strikeThreshold: LOW_SPEED_STRIKE_THRESHOLD,
					cooldownMs: LOW_SPEED_RECOVERY_COOLDOWN_MS,
					nowMs: Date.now(),
					strikeMap: this.lowSpeedStrikeMap,
					recoverAtMap: this.lowSpeedRecoverAtMap,
				});

				const strikeMap = result?.strikeMap;
				const recoverAtMap = result?.recoverAtMap;
				this.lowSpeedStrikeMap =
					strikeMap && typeof strikeMap === "object" ? strikeMap : {};
				this.lowSpeedRecoverAtMap =
					recoverAtMap && typeof recoverAtMap === "object" ? recoverAtMap : {};
				this.syncLowSpeedRecoveringMapWithState();

				const recoverGids = Array.isArray(result?.recoverGids)
					? result.recoverGids
					: [];
				await Promise.allSettled(
					recoverGids.map((gid) => this.recoverLowSpeedTask(`${gid || ""}`)),
				);
			} catch (err) {
				logger.warn(
					"[Motrix] evaluateLowSpeedTasks failed:",
					err?.message || err,
				);
			}
		},
		getTraySpeedLabelPayload() {
			return {
				appName: this.$t("menu.app"),
				downloadLabel: this.$t("task.task-download-speed"),
				uploadLabel: this.$t("task.task-upload-speed"),
			};
		},
		syncTraySpeedTooltip(showTraySpeed = this.traySpeedometer) {
			const { appName, downloadLabel, uploadLabel } =
				this.getTraySpeedLabelPayload();
			invoke("on_speed_change", {
				uploadSpeed: this.uploadSpeed,
				downloadSpeed: this.downloadSpeed,
				showTraySpeed: !!showTraySpeed,
				appName,
				downloadLabel,
				uploadLabel,
			}).catch(() => {
				/* noop */
			});
		},
		async setNoSleepState(downloading: boolean) {
			if (!downloading) {
				if (this.noSleepSource === "plugin") {
					try {
						await unblock();
						this.noSleepSource = null;
						return true;
					} catch {
						return false;
					}
				}

				if (this.noSleepSource === "rust") {
					try {
						await invoke("on_download_status_change", { downloading: false });
						this.noSleepSource = null;
						return true;
					} catch {
						return false;
					}
				}

				return true;
			}

			try {
				await block(NoSleepType.PreventUserIdleSystemSleep);
				this.noSleepSource = "plugin";
				return true;
			} catch {
				// Keep Rust-side fallback for environments where the plugin command is unavailable.
				try {
					await invoke("on_download_status_change", { downloading });
					this.noSleepSource = "rust";
					return true;
				} catch {
					return false;
				}
			}
		},
		async syncNoSleepState() {
			if (this.noSleepSyncing) {
				this.noSleepResyncNeeded = true;
				return;
			}

			this.noSleepSyncing = true;
			do {
				this.noSleepResyncNeeded = false;
				const target = this.noSleepDesired;
				if (target === this.noSleepApplied) {
					continue;
				}

				const ok = await this.setNoSleepState(target);
				if (ok) {
					this.noSleepApplied = target;
				} else {
					// Stop retry loop on hard failure; next state transition will retry.
					break;
				}
			} while (this.noSleepResyncNeeded);
			this.noSleepSyncing = false;

			if (this.noSleepResyncNeeded) {
				// Catch race where another update arrived right after loop exit.
				this.syncNoSleepState();
			}
		},
		async fetchTaskItem({ gid }) {
			return api.fetchTaskItem({ gid }).catch((e) => {
				logger.warn(`fetchTaskItem fail: ${e.message}`);
			});
		},
		onDownloadStart(payload: { gid: string }) {
			const taskStore = useTaskStore();
			taskStore.fetchList();
			useAppStore().resetInterval();
			taskStore.saveSession();
			const { gid } = payload;
			this.clearAutoRetryState(gid);
			const { seedingList } = this;
			if (seedingList.includes(gid)) {
				return;
			}

			this.fetchTaskItem({ gid }).then((task) => {
				if (!task) {
					return;
				}
				const { dir } = task;
				usePreferenceStore().recordHistoryDirectory(dir);
				const taskName = getTaskName(task);
				const message = this.$t("task.download-start-message", { taskName });
				this.$msg.info(message);

				protectDownloadFile(task);
			});
		},
		onDownloadPause(payload: { gid: string }) {
			const { gid } = payload;
			this.clearAutoRetryState(gid);
			const { seedingList } = this;
			if (seedingList.includes(gid)) {
				return;
			}

			this.fetchTaskItem({ gid }).then((task) => {
				if (!task) {
					return;
				}
				unprotectDownloadFile(task);
				const taskName = getTaskName(task);
				const message = this.$t("task.download-pause-message", { taskName });
				this.$msg.info(message);
			});
		},
		onDownloadStop(payload: { gid: string }) {
			const { gid } = payload;
			this.clearAutoRetryState(gid);
			this.fetchTaskItem({ gid }).then((task) => {
				if (!task) {
					return;
				}
				unprotectDownloadFile(task);
				const taskName = getTaskName(task);
				const message = this.$t("task.download-stop-message", { taskName });
				this.$msg.info(message);
			});
		},
		onDownloadError(payload: { gid: string }) {
			const { gid } = payload;
			this.scheduleAutoRetry(gid);
			this.fetchTaskItem({ gid }).then((task) => {
				if (!task) {
					return;
				}
				unprotectDownloadFile(task);
				const taskName = getTaskName(task);
				const { errorCode, errorMessage } = task;
				logger.error(
					`[Motrix] download error gid: ${gid}, #${errorCode}, ${errorMessage}`,
				);
				const message = this.$t("task.download-error-message", { taskName });
				const link = `https://github.com/agalwood/Motrix/wiki/Error#${errorCode}`;
				this.$msg.error({
					duration: 5000,
					message: `${message} (${errorCode}) ${link}`,
				});
			});
		},
		onDownloadComplete(payload: { gid: string }) {
			const taskStore = useTaskStore();
			taskStore.fetchList();
			const { gid } = payload;
			this.clearAutoRetryState(gid);
			taskStore.removeFromSeedingList(gid);

			this.fetchTaskItem({ gid }).then((task) => {
				if (!task) {
					return;
				}
				this.handleDownloadComplete(task, false);
			});
		},
		onBtDownloadComplete(payload: { gid: string }) {
			const taskStore = useTaskStore();
			taskStore.fetchList();
			const { gid } = payload;
			this.clearAutoRetryState(gid);
			const { seedingList } = this;
			if (seedingList.includes(gid)) {
				return;
			}

			taskStore.addToSeedingList(gid);

			this.fetchTaskItem({ gid }).then((task) => {
				if (!task) {
					return;
				}
				this.handleDownloadComplete(task, true);
			});
		},
		async handleDownloadComplete(task, isBT) {
			useTaskStore().saveSession();

			// Remove file protection before renaming .part → final name
			await unprotectDownloadFile(task);

			const path = await finalizeCompletedDownloadPath(task);
			this.showTaskCompleteNotify(task, isBT, path);
			invoke("on_task_download_complete", { path }).catch(() => {
				/* noop */
			});
		},
		showTaskCompleteNotify(task, isBT, path) {
			const taskName = getTaskName(task);
			const message = isBT
				? this.$t("task.bt-download-complete-message", { taskName })
				: this.$t("task.download-complete-message", { taskName });
			const tips = isBT ? `\n${this.$t("task.bt-download-complete-tips")}` : "";

			this.$msg.success(`${message}${tips}`);

			if (!this.taskNotification) {
				return;
			}

			const notifyMessage = isBT
				? this.$t("task.bt-download-complete-notify")
				: this.$t("task.download-complete-notify");

			const notify = new Notification(notifyMessage, {
				body: `${taskName}${tips}`,
			});
			notify.onclick = () => {
				showItemInFolder(path, {
					errorMsg: this.$t("task.file-not-exist"),
				});
			};
		},
		showTaskErrorNotify(task) {
			const taskName = getTaskName(task);
			const message = this.$t("task.download-fail-message", { taskName });
			this.$msg.success(message);

			if (!this.taskNotification) {
				return;
			}

			new Notification(this.$t("task.download-fail-notify"), {
				body: taskName,
			});
		},
		async bindEngineEvents() {
			const handlers: [string, (payload: { gid: string }) => void][] = [
				["engine:download-start", this.onDownloadStart],
				["engine:download-stop", this.onDownloadStop],
				["engine:download-complete", this.onDownloadComplete],
				["engine:download-error", this.onDownloadError],
				["engine:bt-download-complete", this.onBtDownloadComplete],
			];
			for (const [event, handler] of handlers) {
				const unlisten = await listen<{ gid: string }>(event, (e) =>
					handler(e.payload),
				);
				this.eventUnlisteners.push(unlisten);
			}
		},
		unbindEngineEvents() {
			for (const unlisten of this.eventUnlisteners) {
				unlisten();
			}
			this.eventUnlisteners = [];
		},
		startPolling() {
			this.stopPolling();

			const loop = async () => {
				await this.polling();
				if (!this.isDestroyed) {
					this.timer = setTimeout(loop, this.interval);
				}
			};

			this.timer = setTimeout(loop, this.interval);
		},
		async polling() {
			if (this.isPolling) {
				return;
			}
			this.isPolling = true;

			try {
				const jobs: Array<Promise<unknown>> = [
					useAppStore().fetchGlobalStat(),
					useAppStore().fetchProgress(),
				];
				let activeTasksForLowSpeedCheck: Pick<
					DownloadTask,
					"gid" | "status" | "downloadSpeed" | "seeder"
				>[] = [];

				if (!document.hidden || this.taskDetailVisible) {
					jobs.push(useTaskStore().fetchList());
				}

				if (this.autoDetectLowSpeedTasks) {
					jobs.push(
						api
							.fetchActiveTaskList({
								keys: ["gid", "status", "downloadSpeed", "seeder"],
							})
							.then((tasks) => {
								activeTasksForLowSpeedCheck = Array.isArray(tasks) ? tasks : [];
							})
							.catch((err) => {
								logger.warn(
									"[Motrix] low speed detection fetch active tasks failed:",
									err?.message || err,
								);
							}),
					);
				}

				if (this.taskDetailVisible && this.currentTaskGid) {
					if (this.currentTaskIsBT && this.enabledFetchPeers) {
						jobs.push(useTaskStore().fetchItemWithPeers(this.currentTaskGid));
					} else {
						jobs.push(useTaskStore().fetchItem(this.currentTaskGid));
					}
				}

				await Promise.allSettled(jobs);
				if (this.autoDetectLowSpeedTasks) {
					await this.handleLowSpeedTasks(activeTasksForLowSpeedCheck);
				}
			} finally {
				this.isPolling = false;
			}
		},
		stopPolling() {
			clearTimeout(this.timer);
			this.timer = null;
		},
		autoResumeUnfinishedTasksOnLaunch() {
			if (this.startupAutoResumeHandled) {
				return;
			}
			this.startupAutoResumeHandled = true;

			if (!this.resumeAllWhenAppLaunched) {
				return;
			}

			useTaskStore()
				.resumeAllTask()
				.catch((err) => {
					logger.warn(
						"[Motrix] auto resume unfinished tasks failed:",
						err?.message || err,
					);
				});
		},
	},
	created() {
		this.bindEngineEvents().catch((err) => {
			logger.warn("[Motrix] bindEngineEvents failed:", err.message);
		});
	},
	mounted() {
		this.initTimer = setTimeout(() => {
			const appStore = useAppStore();
			appStore.fetchEngineInfo();
			appStore.fetchEngineOptions();
			this.autoResumeUnfinishedTasksOnLaunch();
			this.syncTraySpeedTooltip();

			this.startPolling();
		}, 100);
	},
	beforeUnmount() {
		this.isDestroyed = true;
		useTaskStore().saveSession();
		clearTimeout(this.initTimer);
		this.initTimer = null;

		this.unbindEngineEvents();
		this.stopPolling();
		this.clearAllAutoRetryTimers();

		// Best effort release in case component is torn down while downloads were active.
		this.noSleepDesired = false;
		this.syncNoSleepState();
	},
};
</script>
