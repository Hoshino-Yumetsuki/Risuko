import { ADD_TASK_TYPE } from "@shared/constants";
import type { EngineInfo } from "@shared/types/task";
import logger from "@shared/utils/logger";
import { defineStore } from "pinia";
import api from "@/api";
import { useTaskStore } from "@/store/task";
import { getSystemTheme } from "@/utils/native";

const BASE_INTERVAL = 1500;
const PER_INTERVAL = 80;
const MIN_INTERVAL = 800;
const MAX_INTERVAL = 6000;

const normalizeNonNegativeNumber = (value: unknown): number => {
	const parsed = Number(value);
	if (!Number.isFinite(parsed) || parsed <= 0) {
		return 0;
	}
	return parsed;
};

const calcRendererProgress = (
	tasks: Pick<
		import("@shared/types/task").DownloadTask,
		"totalLength" | "completedLength"
	>[] = [],
) => {
	if (tasks.length === 0) {
		return -1;
	}

	let total = 0;
	let completed = 0;
	for (const task of tasks) {
		const totalLength = normalizeNonNegativeNumber(task?.totalLength);
		if (totalLength === 0) {
			continue;
		}

		total += totalLength;
		completed += normalizeNonNegativeNumber(task?.completedLength);
	}

	if (total === 0) {
		return 2;
	}

	return completed / total;
};

export const useAppStore = defineStore("app", {
	state: () => ({
		systemTheme: getSystemTheme(),
		trayFocused: false,
		aboutPanelVisible: false,
		engineInfo: {
			version: "",
			enabledFeatures: [],
		},
		engineOptions: {},
		interval: BASE_INTERVAL,
		stat: {
			downloadSpeed: 0,
			uploadSpeed: 0,
			numActive: 0,
			numWaiting: 0,
			numStopped: 0,
		},
		addTaskVisible: false,
		addTaskType: ADD_TASK_TYPE.URI,
		addTaskUrl: "",
		addTaskTorrents: [],
		addTaskOptions: {},
		progress: 0,
	}),
	actions: {
		updateSystemTheme(theme: string) {
			this.systemTheme = theme;
		},
		updateTrayFocused(focused: boolean) {
			this.trayFocused = focused;
		},
		showAboutPanel() {
			this.aboutPanelVisible = true;
		},
		hideAboutPanel() {
			this.aboutPanelVisible = false;
		},
		async fetchEngineInfo() {
			try {
				const data = (await api.getVersion()) as unknown as EngineInfo;
				this.engineInfo = { ...this.engineInfo, ...data };
				return data;
			} catch (err: unknown) {
				logger.warn("[Risuko] fetchEngineInfo failed:", (err as Error).message);
				return null;
			}
		},
		async fetchEngineOptions() {
			try {
				const data = await api.getGlobalOption();
				this.engineOptions = { ...this.engineOptions, ...data };
				return data;
			} catch (err: unknown) {
				logger.warn(
					"[Risuko] fetchEngineOptions failed:",
					(err as Error).message,
				);
				return null;
			}
		},
		async fetchGlobalStat() {
			try {
				const data = await api.getGlobalStat();
				const stat: Record<string, number> = {};
				Object.keys(data).forEach((key) => {
					stat[key] = Number(data[key]);
				});

				const { numActive } = stat;
				if (numActive > 0) {
					const interval = BASE_INTERVAL - PER_INTERVAL * numActive;
					this.updateInterval(interval);
				} else {
					stat.downloadSpeed = 0;
					this.increaseInterval();
				}
				this.stat = stat;
				useTaskStore().updateTaskCountsFromStat(stat);
			} catch (err: unknown) {
				logger.warn("[Risuko] fetchGlobalStat failed:", (err as Error).message);
			}
		},
		increaseInterval(millisecond = 100) {
			if (this.interval < MAX_INTERVAL) {
				this.interval += millisecond;
			}
		},
		showAddTaskDialog(taskType: string) {
			this.addTaskType = taskType;
			this.addTaskVisible = true;
		},
		hideAddTaskDialog() {
			this.addTaskVisible = false;
			this.addTaskUrl = "";
			this.addTaskTorrents = [];
		},
		changeAddTaskType(taskType: string) {
			this.addTaskType = taskType;
		},
		updateAddTaskUrl(uri = "") {
			this.addTaskUrl = uri;
		},
		addTaskAddTorrents({
			fileList,
		}: {
			fileList: { name: string; path: string }[];
		}) {
			this.addTaskTorrents = [...fileList];
		},
		updateAddTaskOptions(options = {}) {
			this.addTaskOptions = { ...options };
		},
		updateInterval(millisecond: number) {
			let interval = millisecond;
			if (millisecond > MAX_INTERVAL) {
				interval = MAX_INTERVAL;
			}
			if (millisecond < MIN_INTERVAL) {
				interval = MIN_INTERVAL;
			}
			if (this.interval === interval) {
				return;
			}
			this.interval = interval;
		},
		resetInterval() {
			this.interval = BASE_INTERVAL;
		},
		async fetchProgress() {
			try {
				const data = await api.fetchActiveTaskList({
					keys: ["totalLength", "completedLength"],
				});
				const tasks = Array.isArray(data) ? data : [];
				let progress = -1;

				if (tasks.length === 0) {
					this.progress = -1;
					return;
				}

				try {
					const nativeProgress = Number(
						await api.calculateActiveTaskProgress({ tasks }),
					);
					const normalizedNativeProgress =
						nativeProgress === 2 ? -1 : nativeProgress;
					progress = Number.isFinite(normalizedNativeProgress)
						? normalizedNativeProgress
						: calcRendererProgress(tasks);
				} catch (nativeErr) {
					logger.warn(
						"[Risuko] calculateActiveTaskProgress failed, fallback to renderer:",
						(nativeErr as Error)?.message || nativeErr,
					);
					progress = calcRendererProgress(tasks);
				}

				this.progress = progress === 2 ? -1 : progress;
			} catch (err: unknown) {
				logger.warn("[Risuko] fetchProgress failed:", (err as Error).message);
			}
		},
	},
});
