import { EMPTY_STRING, TASK_STATUS } from "@shared/constants";
import type {
	DownloadFile,
	DownloadTask,
	PeerInfo,
	SyncOrderResult,
} from "@shared/types/task";
import { checkTaskIsBT, getTaskName, intersection } from "@shared/utils";
import logger from "@shared/utils/logger";
import { defineStore } from "pinia";
import api from "@/api";
import { useAppStore } from "@/store/app";

const DEFAULT_TASKS_PER_PAGE = 20;
const TASKS_PER_PAGE_OPTIONS = [10, 20, 30, 40, 50];
const TASKS_PER_PAGE_STORAGE_KEY = "risuko.tasks-per-page";
const SORT_BY_STORAGE_KEY = "risuko.task-sort-by";
const SORT_ORDER_STORAGE_KEY = "risuko.task-sort-order";

/** Maximum number of speed samples retained per task */
export const SPEED_HISTORY_LIMIT = 60;

export type SpeedSample = { download: number; upload: number };

/** Module cache: gid -> speed samples. */
// please work please work please work
const speedHistoryCache = new Map<string, SpeedSample[]>();

export function getSpeedHistory(gid: string): SpeedSample[] {
	return speedHistoryCache.get(gid) || [];
}

export function deleteSpeedHistory(gid: string): void {
	speedHistoryCache.delete(gid);
}

function sampleSpeedsFromTasks(tasks: DownloadTask[]): boolean {
	let changed = false;
	for (const task of tasks) {
		const status = task.status;
		const isActive =
			status === TASK_STATUS.ACTIVE || status === TASK_STATUS.SEEDING;
		if (!isActive || !task.gid) {
			continue;
		}
		const isBT = checkTaskIsBT(task);
		const isSeeder = isBT && task.seeder === "true";
		const download = isSeeder
			? 0
			: Math.max(0, Number(task.downloadSpeed || 0));
		const upload = isBT ? Math.max(0, Number(task.uploadSpeed || 0)) : 0;
		const sample: SpeedSample = { download, upload };
		const prev = speedHistoryCache.get(task.gid) || [];
		const next = [...prev, sample].slice(-SPEED_HISTORY_LIMIT);
		speedHistoryCache.set(task.gid, next);
		changed = true;
	}
	return changed;
}

type TaskSortBy = "default" | "name" | "size" | "time";
type TaskSortOrder = "asc" | "desc";
type DisplayTask = DownloadTask & {
	_displayKey: string;
	_isFileEntry?: boolean;
};

const clampTasksPerPage = (value: number) => {
	const normalized = Number(value);
	if (!Number.isFinite(normalized)) {
		return DEFAULT_TASKS_PER_PAGE;
	}

	const intValue = Math.floor(normalized);
	if (TASKS_PER_PAGE_OPTIONS.includes(intValue)) {
		return intValue;
	}

	return DEFAULT_TASKS_PER_PAGE;
};

const loadTasksPerPage = () => {
	if (typeof window === "undefined" || !window.localStorage) {
		return DEFAULT_TASKS_PER_PAGE;
	}
	const saved = window.localStorage.getItem(TASKS_PER_PAGE_STORAGE_KEY);
	if (saved === null) {
		return DEFAULT_TASKS_PER_PAGE;
	}
	return clampTasksPerPage(Number(saved));
};

const loadSortBy = (): TaskSortBy => {
	if (typeof window === "undefined" || !window.localStorage) {
		return "default";
	}
	const saved = window.localStorage.getItem(SORT_BY_STORAGE_KEY);
	if (saved === "name" || saved === "size" || saved === "time") {
		return saved;
	}
	return "default";
};

const loadSortOrder = (): TaskSortOrder => {
	if (typeof window === "undefined" || !window.localStorage) {
		return "asc";
	}
	const saved = window.localStorage.getItem(SORT_ORDER_STORAGE_KEY);
	if (saved === "desc") {
		return "desc";
	}
	return "asc";
};

const getTaskSortName = (task: DownloadTask | DisplayTask): string => {
	return getTaskName(task, { defaultName: "", maxLen: -1 }).toLowerCase();
};

const getTaskSortSize = (task: DownloadTask | DisplayTask): number => {
	return Number(task.totalLength) || 0;
};

const getTaskSortTime = (task: DownloadTask | DisplayTask): number => {
	return Number(task.createdAt) || 0;
};

export const useTaskStore = defineStore("task", {
	state: () => ({
		currentList: "active",
		taskDetailVisible: false,
		currentTaskGid: EMPTY_STRING,
		enabledFetchPeers: false,
		currentTaskItem: null as (DownloadTask & { peers?: PeerInfo[] }) | null,
		currentTaskFiles: [] as DownloadFile[],
		currentTaskPeers: [] as PeerInfo[],
		seedingList: [] as string[],
		taskList: [] as DownloadTask[],
		selectedGidList: [] as string[],
		speedHistoryRev: 0,
		taskOrderMap: {
			all: [] as string[],
			active: [] as string[],
			waiting: [] as string[],
			completed: [] as string[],
			stopped: [] as string[],
		},
		taskCountMap: {
			all: 0,
			active: 0,
			waiting: 0,
			completed: 0,
			stopped: 0,
		} as Record<string, number>,
		tasksPerPage: loadTasksPerPage(),
		filterText: "",
		sortBy: loadSortBy() as TaskSortBy,
		sortOrder: loadSortOrder() as TaskSortOrder,
		currentPageMap: {
			all: 1,
			active: 1,
			waiting: 1,
			completed: 1,
			stopped: 1,
		},
	}),
	getters: {
		currentPage(state) {
			return state.currentPageMap[state.currentList] || 1;
		},
		displayTaskList(state) {
			const result: DisplayTask[] = [];
			for (const task of state.taskList) {
				const isBT = checkTaskIsBT(task);
				const files = Array.isArray(task.files) ? task.files : [];
				const selectedFiles = files.filter(
					(f: DownloadFile) => f.selected !== "false",
				);
				if (isBT && selectedFiles.length > 1) {
					for (const file of selectedFiles) {
						result.push({
							...task,
							_displayKey: `${task.gid}#f${file.index}`,
							_isFileEntry: true,
							totalLength: file.length,
							completedLength: file.completedLength,
							files: [file],
							bittorrent: {
								...(task.bittorrent || {}),
								info: {},
							},
						});
					}
				} else {
					result.push({
						...task,
						_displayKey: task.gid,
					});
				}
			}
			return result;
		},
		filteredTaskList(state) {
			const filter = state.filterText.trim().toLowerCase();
			if (!filter) {
				return this.displayTaskList;
			}
			return this.displayTaskList.filter((task: DisplayTask) => {
				const name = getTaskSortName(task);
				return name.includes(filter);
			});
		},
		sortedTaskList(state) {
			const list = this.filteredTaskList;
			if (state.sortBy === "default") {
				return list;
			}
			const sorted = [...list];
			const order = state.sortOrder === "desc" ? -1 : 1;
			sorted.sort((a: DisplayTask, b: DisplayTask) => {
				let cmp = 0;
				switch (state.sortBy) {
					case "name":
						cmp = getTaskSortName(a).localeCompare(getTaskSortName(b));
						break;
					case "size":
						cmp = getTaskSortSize(a) - getTaskSortSize(b);
						break;
					case "time":
						cmp = getTaskSortTime(a) - getTaskSortTime(b);
						break;
				}
				return cmp * order;
			});
			return sorted;
		},
		totalPages(state) {
			return Math.max(
				1,
				Math.ceil(this.sortedTaskList.length / state.tasksPerPage),
			);
		},
		paginatedTaskList(state) {
			const currentPage = state.currentPageMap[state.currentList] || 1;
			const start = (currentPage - 1) * state.tasksPerPage;
			const end = start + state.tasksPerPage;
			return this.sortedTaskList.slice(start, end);
		},
	},
	actions: {
		applyTaskOrder(type: string, tasks: DownloadTask[] = []) {
			const order = this.taskOrderMap[type];
			if (!order || order.length === 0 || tasks.length < 2) {
				return tasks;
			}

			const orderIndex = new Map(order.map((gid, index) => [gid, index]));
			const fallbackIndex = new Map(
				tasks.map((task, index) => [task.gid, index]),
			);

			return [...tasks].sort((a, b) => {
				const aOrderIndex = orderIndex.get(a.gid);
				const bOrderIndex = orderIndex.get(b.gid);
				const aIndex =
					typeof aOrderIndex === "number"
						? aOrderIndex
						: Number.MAX_SAFE_INTEGER;
				const bIndex =
					typeof bOrderIndex === "number"
						? bOrderIndex
						: Number.MAX_SAFE_INTEGER;
				if (aIndex !== bIndex) {
					return aIndex - bIndex;
				}
				return (
					(fallbackIndex.get(a.gid) || 0) - (fallbackIndex.get(b.gid) || 0)
				);
			});
		},
		updateTaskOrder(type: string, gids: string[] = []) {
			this.taskOrderMap = {
				...this.taskOrderMap,
				[type]: [...gids],
			};
		},
		changeCurrentList(currentList: string) {
			this.currentList = currentList;
			this.selectedGidList = [];
			this.filterText = "";
			this.fetchList();
		},
		updateCurrentPage(listType: string, page: number) {
			const maxPage = Math.max(
				1,
				Math.ceil(this.sortedTaskList.length / this.tasksPerPage),
			);
			const normalizedPage = Math.min(
				Math.max(Math.floor(Number(page) || 1), 1),
				maxPage,
			);
			this.currentPageMap = {
				...this.currentPageMap,
				[listType]: normalizedPage,
			};
			this.selectedGidList = [];
		},
		ensurePageInRange(listType = this.currentList) {
			const currentPage = this.currentPageMap[listType] || 1;
			const maxPage = Math.max(
				1,
				Math.ceil(this.sortedTaskList.length / this.tasksPerPage),
			);
			if (currentPage > maxPage) {
				this.updateCurrentPage(listType, maxPage);
			}
			if (currentPage < 1) {
				this.updateCurrentPage(listType, 1);
			}
		},
		changeCurrentPage(page: number) {
			this.updateCurrentPage(this.currentList, page);
		},
		setTasksPerPage(value: number) {
			const next = clampTasksPerPage(value);
			this.tasksPerPage = next;
			this.ensurePageInRange(this.currentList);
			if (typeof window !== "undefined" && window.localStorage) {
				window.localStorage.setItem(TASKS_PER_PAGE_STORAGE_KEY, `${next}`);
			}
		},
		setFilterText(text: string) {
			this.filterText = text;
			this.ensurePageInRange(this.currentList);
		},
		setSortBy(sortBy: TaskSortBy) {
			this.sortBy = sortBy;
			this.ensurePageInRange(this.currentList);
			if (typeof window !== "undefined" && window.localStorage) {
				window.localStorage.setItem(SORT_BY_STORAGE_KEY, sortBy);
			}
		},
		setSortOrder(order: TaskSortOrder) {
			this.sortOrder = order;
			this.ensurePageInRange(this.currentList);
			if (typeof window !== "undefined" && window.localStorage) {
				window.localStorage.setItem(SORT_ORDER_STORAGE_KEY, order);
			}
		},
		toggleSortOrder() {
			this.setSortOrder(this.sortOrder === "asc" ? "desc" : "asc");
		},
		async fetchList() {
			try {
				const type = this.currentList;
				let fetchType = type;
				if (type === "completed") {
					fetchType = "stopped";
				}
				const rawData = (await api.fetchTaskList({
					type: fetchType,
				})) as DownloadTask[];

				let data: DownloadTask[];
				if (type === "completed") {
					data = rawData.filter(
						(task: DownloadTask) => task.status === TASK_STATUS.COMPLETE,
					);
				} else if (type === "stopped") {
					data = rawData.filter(
						(task: DownloadTask) => task.status !== TASK_STATUS.COMPLETE,
					);
				} else {
					data = rawData;
				}

				const orderedData = this.applyTaskOrder(type, data);
				this.taskList = orderedData;
				this.taskCountMap = { ...this.taskCountMap, [type]: data.length };
				this.ensurePageInRange(type);
				this.updateTaskOrder(
					type,
					orderedData.map((task) => task.gid),
				);

				const gids = orderedData.map((task) => task.gid);
				this.selectedGidList = intersection(this.selectedGidList, gids);
				return orderedData;
			} catch (err: unknown) {
				logger.warn("[Risuko] fetchList failed:", (err as Error).message);
				this.taskList = [];
				this.selectedGidList = [];
				return [];
			}
		},
		async updateTaskCountsFromStat(stat: Record<string, number>) {
			const numActive = stat.numActive || 0;
			const numWaiting = stat.numWaiting || 0;
			const numStoppedTotal = stat.numStoppedTotal || 0;

			let completedCount = this.taskCountMap.completed || 0;
			let stoppedCount = this.taskCountMap.stopped || 0;

			if (numStoppedTotal > 0) {
				try {
					const stoppedTasks = (await api.fetchTaskList({
						type: "stopped",
						keys: ["gid", "status"],
					})) as DownloadTask[];
					completedCount = stoppedTasks.filter(
						(t) => t.status === TASK_STATUS.COMPLETE,
					).length;
					stoppedCount = stoppedTasks.length - completedCount;
				} catch {
					// keep previous counts on failure
				}
			} else {
				completedCount = 0;
				stoppedCount = 0;
			}

			this.taskCountMap = {
				all: numActive + numWaiting + numStoppedTotal,
				active: numActive,
				waiting: numWaiting,
				completed: completedCount,
				stopped: stoppedCount,
			};
		},
		/**
		 * Sample speeds for all active/seeding tasks.
		 * Called every polling tick from EngineClient.
		 */
		async sampleActiveSpeeds() {
			try {
				// If we're on the active list, taskList already has speeds
				if (this.currentList === "active" && this.taskList.length > 0) {
					if (sampleSpeedsFromTasks(this.taskList)) {
						this.speedHistoryRev++;
					}
					return;
				}

				// Otherwise fetch for active tasks
				const tasks = (await api.fetchTaskList({
					type: "active",
					keys: [
						"gid",
						"status",
						"downloadSpeed",
						"uploadSpeed",
						"seeder",
						"bittorrent",
					],
				})) as DownloadTask[];
				if (sampleSpeedsFromTasks(tasks)) {
					this.speedHistoryRev++;
				}
			} catch {
				// Sampling is best-effort
			}
		},
		selectTasks(list: string[]) {
			this.selectedGidList = list;
		},
		selectAllTask() {
			this.selectedGidList = this.paginatedTaskList.map((task) => task.gid);
		},
		async fetchItem(gid: string) {
			try {
				const data = await api.fetchTaskItem({ gid });
				this.updateCurrentTaskItem(data);
				return data;
			} catch (err: unknown) {
				logger.warn("[Risuko] fetchItem failed:", (err as Error).message);
				this.updateCurrentTaskItem(null);
				return null;
			}
		},
		async fetchItemWithPeers(gid: string) {
			try {
				const data = await api.fetchTaskItemWithPeers({ gid });
				if (!data) {
					this.updateCurrentTaskItem(null);
					return null;
				}
				this.updateCurrentTaskItem(data);
				return data;
			} catch (err: unknown) {
				logger.warn(
					"[Risuko] fetchItemWithPeers failed:",
					(err as Error).message,
				);
				this.updateCurrentTaskItem(null);
				return null;
			}
		},
		async showTaskDetailByGid(gid: string) {
			try {
				const task = await api.fetchTaskItem({ gid });
				if (!task) {
					return null;
				}
				this.updateCurrentTaskItem(task);
				this.currentTaskGid = task.gid;
				this.taskDetailVisible = true;
				return task;
			} catch (err: unknown) {
				logger.warn(
					"[Risuko] showTaskDetailByGid failed:",
					(err as Error).message,
				);
				return null;
			}
		},
		showTaskDetail(task: DisplayTask) {
			if (task._isFileEntry) {
				return this.showTaskDetailByGid(task.gid);
			}
			this.updateCurrentTaskItem(task);
			this.currentTaskGid = task.gid;
			this.taskDetailVisible = true;
		},
		hideTaskDetail() {
			this.taskDetailVisible = false;
		},
		toggleEnabledFetchPeers(enabled: boolean) {
			this.enabledFetchPeers = enabled;
		},
		updateCurrentTaskItem(
			task: (DownloadTask & { peers?: PeerInfo[] }) | null,
		) {
			this.currentTaskItem = task;
			if (task) {
				this.currentTaskFiles = task.files;
				this.currentTaskPeers = task.peers;
			} else {
				this.currentTaskFiles = [];
				this.currentTaskPeers = [];
			}
		},
		updateCurrentTaskGid(gid: string) {
			this.currentTaskGid = gid;
		},
		addUri(data: {
			uris: string[];
			outs: string[];
			options: Record<string, string>;
		}) {
			const { uris, outs, options } = data;
			return api.addUri({ uris, outs, options }).then(() => {
				this.fetchList();
				const appStore = useAppStore();
				appStore.updateAddTaskOptions({});
			});
		},
		addTorrent(data: { torrentPath: string; options: Record<string, string> }) {
			const { torrentPath, options } = data;
			return api.addTorrent({ torrentPath, options }).then(() => {
				this.fetchList();
				const appStore = useAppStore();
				appStore.updateAddTaskOptions({});
			});
		},
		getTaskOption(gid: string) {
			return api.getOption({ gid }).catch((err: unknown) => {
				logger.warn("[Risuko] getTaskOption failed:", (err as Error).message);
				return {};
			});
		},
		changeTaskOption(payload: {
			gid: string;
			options: Record<string, string>;
		}) {
			const { gid, options } = payload;
			return api.changeOption({ gid, options });
		},
		removeTask(task: DownloadTask) {
			const { gid } = task;
			if (gid === this.currentTaskGid) {
				this.hideTaskDetail();
			}

			return api.removeTask({ gid }).finally(() => {
				speedHistoryCache.delete(gid);
				this.fetchList();
				this.saveSession();
			});
		},
		forcePauseTask(task: Pick<DownloadTask, "gid" | "status">) {
			const { gid, status } = task;
			if (status !== TASK_STATUS.ACTIVE) {
				return Promise.resolve(true);
			}

			return api.forcePauseTask({ gid }).finally(() => {
				this.fetchList();
				this.saveSession();
			});
		},
		pauseTask(task: DownloadTask) {
			const { gid } = task;
			const isBT = checkTaskIsBT(task);
			const promise = isBT
				? api.forcePauseTask({ gid })
				: api.pauseTask({ gid });
			promise.finally(() => {
				this.fetchList();
				this.saveSession();
			});
			return promise;
		},
		resumeTask(task: DownloadTask) {
			const { gid } = task;
			return api.resumeTask({ gid }).finally(() => {
				this.fetchList();
				this.saveSession();
			});
		},
		pauseAllTask() {
			return api
				.pauseAllTask()
				.catch(() => {
					return api.forcePauseAllTask();
				})
				.finally(() => {
					this.fetchList();
					this.saveSession();
				});
		},
		resumeAllTask() {
			return api.resumeAllTask().finally(() => {
				this.fetchList();
				this.saveSession();
			});
		},
		addToSeedingList(gid: string) {
			if (this.seedingList.includes(gid)) {
				return;
			}

			this.seedingList = [...this.seedingList, gid];
		},
		removeFromSeedingList(gid: string) {
			const idx = this.seedingList.indexOf(gid);
			if (idx === -1) {
				return;
			}

			this.seedingList = [
				...this.seedingList.slice(0, idx),
				...this.seedingList.slice(idx + 1),
			];
		},
		stopSeeding({ gid }: { gid: string }) {
			return this.pauseTask({ gid, status: "active" }).then(() => {
				const options = {
					seedTime: 0,
				};
				return this.changeTaskOption({ gid, options });
			});
		},
		removeTaskRecord(task: DownloadTask) {
			const { gid, status } = task;
			if (gid === this.currentTaskGid) {
				this.hideTaskDetail();
			}

			const { ERROR, COMPLETE, REMOVED } = TASK_STATUS;
			if ([ERROR, COMPLETE, REMOVED].indexOf(status) === -1) {
				return;
			}
			return api.removeTaskRecord({ gid }).finally(() => this.fetchList());
		},
		saveSession() {
			api.saveSession();
		},
		purgeTaskRecord() {
			return api.purgeTaskRecord().finally(() => this.fetchList());
		},
		toggleTask(task: DownloadTask) {
			const { status } = task;
			const { ACTIVE, WAITING, PAUSED } = TASK_STATUS;
			if (status === ACTIVE) {
				return this.pauseTask(task);
			} else if (status === WAITING || status === PAUSED) {
				return this.resumeTask(task);
			}
		},
		batchResumeSelectedTasks() {
			const gids: string[] = [...new Set<string>(this.selectedGidList)];
			if (gids.length === 0) {
				return;
			}

			return api.batchResumeTask({ gids }).finally(() => {
				this.fetchList();
				this.saveSession();
			});
		},
		batchPauseSelectedTasks() {
			const gids: string[] = [...new Set<string>(this.selectedGidList)];
			if (gids.length === 0) {
				return;
			}

			return api.batchPauseTask({ gids }).finally(() => {
				this.fetchList();
				this.saveSession();
			});
		},
		batchForcePauseTask(gids: string[]) {
			return api.batchForcePauseTask({ gids });
		},
		batchResumeTask(gids: string[]) {
			return api.batchResumeTask({ gids });
		},
		batchRemoveTask(gids: string[]) {
			return api.batchRemoveTask({ gids }).finally(() => {
				for (const gid of gids) {
					speedHistoryCache.delete(gid);
				}
				this.fetchList();
				this.saveSession();
			});
		},
		async syncSelectedTaskOrder(
			direction: "up" | "down",
			selectedGids: string[],
		) {
			const selectedGidSet = new Set(selectedGids);
			const selectedTasks = this.taskList.filter((task) =>
				selectedGidSet.has(task.gid),
			);
			const selectedTaskPayload = selectedTasks.map((task) => ({
				gid: task.gid,
				status: task.status,
			}));
			try {
				const result = (await api.syncSelectedTaskOrder({
					direction,
					selectedTasks: selectedTaskPayload,
				})) as SyncOrderResult;
				const movedValue = Number(result?.moved);
				const moved = Number.isFinite(movedValue) ? movedValue : 0;
				const partialError = !!result?.partialError;

				await this.fetchList();
				this.saveSession();

				if (partialError) {
					const err = Object.assign(new Error("priority-sync-failed"), {
						reconciled: true,
					});
					throw err;
				}

				return moved;
			} catch (err: unknown) {
				if (!(err as { reconciled?: boolean })?.reconciled) {
					await this.fetchList();
					this.saveSession();
				}

				throw err;
			}
		},
		async moveSelectedTasks(
			direction: "up" | "down",
			options: { onSyncError?: (error: unknown) => void } = {},
		) {
			const { onSyncError } = options;
			const selectedGids = [...this.selectedGidList];
			if (selectedGids.length === 0) {
				return 0;
			}

			const selectedSet = new Set(selectedGids);
			const nextList = [...this.taskList];
			let moved = 0;

			if (direction === "up") {
				for (let i = 1; i < nextList.length; i += 1) {
					const curr = nextList[i];
					const prev = nextList[i - 1];
					if (!selectedSet.has(curr.gid) || selectedSet.has(prev.gid)) {
						continue;
					}
					nextList[i - 1] = curr;
					nextList[i] = prev;
					moved += 1;
				}
			} else {
				for (let i = nextList.length - 2; i >= 0; i -= 1) {
					const curr = nextList[i];
					const next = nextList[i + 1];
					if (!selectedSet.has(curr.gid) || selectedSet.has(next.gid)) {
						continue;
					}
					nextList[i + 1] = curr;
					nextList[i] = next;
					moved += 1;
				}
			}

			if (moved === 0) {
				return 0;
			}

			this.taskList = nextList;
			this.updateTaskOrder(
				this.currentList,
				nextList.map((task) => task.gid),
			);
			this.saveSession();

			this.syncSelectedTaskOrder(direction, selectedGids).catch(
				(err: unknown) => {
					logger.warn(
						"[Risuko] syncSelectedTaskOrder failed:",
						(err as Error).message,
					);
					if (typeof onSyncError === "function") {
						onSyncError(err);
					}
				},
			);

			return moved;
		},
	},
});
