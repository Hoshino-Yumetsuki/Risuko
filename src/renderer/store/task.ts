import { TASK_STATUS } from "@shared/constants";
import type {
	DownloadFile,
	DownloadTask,
	PeerInfo,
	SyncOrderResult,
} from "@shared/types/task";
import { calcProgress, checkTaskIsBT, getTaskName } from "@shared/utils";
import logger from "@shared/utils/logger";
import { defineStore } from "pinia";
import api from "@/api";
import { useAppStore } from "@/store/app";

const DEFAULT_TASKS_PER_PAGE = 20;
const TASKS_PER_PAGE_OPTIONS = [10, 20, 30, 40, 50];
const TASKS_PER_PAGE_STORAGE_KEY = "risuko.tasks-per-page";
const SORT_BY_STORAGE_KEY = "risuko.task-sort-by";
const SORT_ORDER_STORAGE_KEY = "risuko.task-sort-order";

/** Max speed samples kept per task */
export const SPEED_HISTORY_LIMIT = 60;

/** Max tasks tracked in the speed-history LRU */
const SPEED_HISTORY_GID_LIMIT = 200;

type SpeedSample = { download: number; upload: number };

/** Module cache: gid -> speed samples */
// please work please work please work
const speedHistoryCache = new Map<string, SpeedSample[]>();

export function getSpeedHistory(gid: string): SpeedSample[] {
	return speedHistoryCache.get(gid) || [];
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
		// Re-insert so the sampled gid becomes most-recently-used
		speedHistoryCache.delete(task.gid);
		speedHistoryCache.set(task.gid, next);
		// Bound tracked gids by evicting least-recently-used entries
		while (speedHistoryCache.size > SPEED_HISTORY_GID_LIMIT) {
			const oldest = speedHistoryCache.keys().next().value;
			if (oldest === undefined) {
				break;
			}
			speedHistoryCache.delete(oldest);
		}
		changed = true;
	}
	return changed;
}

type TaskSortBy = "default" | "name" | "size" | "time";
type TaskSortOrder = "asc" | "desc";
type DisplayTask = DownloadTask & {
	_displayKey: string;
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
	const saved = localStorage.getItem(TASKS_PER_PAGE_STORAGE_KEY);
	if (saved === null) {
		return DEFAULT_TASKS_PER_PAGE;
	}
	return clampTasksPerPage(Number(saved));
};

const loadSortBy = (): TaskSortBy => {
	const saved = localStorage.getItem(SORT_BY_STORAGE_KEY);
	if (saved === "name" || saved === "size" || saved === "time") {
		return saved;
	}
	return "default";
};

const loadSortOrder = (): TaskSortOrder => {
	const saved = localStorage.getItem(SORT_ORDER_STORAGE_KEY);
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
		currentTaskGid: "",
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
		filterTag: "",
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
			// One card per task
			return state.taskList.map((task) => ({
				...task,
				_displayKey: task.gid,
			}));
		},
		totalLength(state) {
			return state.taskList.reduce(
				(sum, task) => sum + Number(task.totalLength || 0),
				0,
			);
		},
		totalCompletedLength(state) {
			return state.taskList.reduce(
				(sum, task) => sum + Number(task.completedLength || 0),
				0,
			);
		},
		totalProgressPercent() {
			const result = calcProgress(
				this.totalLength,
				this.totalCompletedLength,
				1,
			);
			return `${result}`.replace(/\.0$/, "");
		},
		filteredTaskList(state) {
			const filter = state.filterText.trim().toLowerCase();
			const tagFilter = state.filterTag.trim().toLowerCase();
			let list = this.displayTaskList;
			if (filter) {
				list = list.filter((task: DisplayTask) => {
					const name = getTaskSortName(task);
					return name.includes(filter);
				});
			}
			if (tagFilter) {
				list = list.filter((task: DisplayTask) => {
					const taskTag = (task.tag || "").toLowerCase();
					return taskTag === tagFilter;
				});
			}
			return list;
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
		selectedGids(state): string[] {
			return state.selectedGidList;
		},
		// Selected rows as `DisplayTask` objects
		selectedTaskRows(state): DisplayTask[] {
			if (state.selectedGidList.length === 0) {
				return [];
			}
			const want = new Set(state.selectedGidList);
			return (this.displayTaskList as DisplayTask[]).filter((task) =>
				want.has(task._displayKey || task.gid),
			);
		},
	},
	actions: {
		applyTaskOrder(type: string, tasks: DownloadTask[] = []) {
			const order = this.taskOrderMap[type];
			if (!order || order.length === 0 || tasks.length < 2) {
				return tasks;
			}

			const orderIndex = new Map<string, number>(
				order.map((gid, index) => [gid, index]),
			);
			const fallbackIndex = new Map<string, number>(
				tasks.map((task, index) => [task.gid, index]),
			);

			return [...tasks].sort(
				(a, b) =>
					(orderIndex.get(a.gid) ?? Number.MAX_SAFE_INTEGER) -
						(orderIndex.get(b.gid) ?? Number.MAX_SAFE_INTEGER) ||
					(fallbackIndex.get(a.gid) || 0) - (fallbackIndex.get(b.gid) || 0),
			);
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
			this.filterTag = "";
			this.fetchList();
		},
		updateCurrentPage(listType: string, page: number) {
			const maxPage = this.totalPages;
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
			const maxPage = this.totalPages;
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
			localStorage.setItem(TASKS_PER_PAGE_STORAGE_KEY, `${next}`);
		},
		setFilterText(text: string) {
			this.filterText = text;
			this.ensurePageInRange(this.currentList);
		},
		setFilterTag(tag: string) {
			this.filterTag = tag;
			this.ensurePageInRange(this.currentList);
		},
		setSortBy(sortBy: TaskSortBy) {
			this.sortBy = sortBy;
			this.ensurePageInRange(this.currentList);
			localStorage.setItem(SORT_BY_STORAGE_KEY, sortBy);
		},
		setSortOrder(order: TaskSortOrder) {
			this.sortOrder = order;
			this.ensurePageInRange(this.currentList);
			localStorage.setItem(SORT_ORDER_STORAGE_KEY, order);
		},
		toggleSortOrder() {
			this.setSortOrder(this.sortOrder === "asc" ? "desc" : "asc");
		},
		async fetchList() {
			const type = this.currentList;
			try {
				let fetchType = type;
				if (type === "completed") {
					fetchType = "stopped";
				}
				const rawData = (await api.fetchTaskList({
					type: fetchType,
				})) as DownloadTask[];

				// Drop stale results if the user switched tabs mid-flight
				if (type !== this.currentList) {
					return [];
				}

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
				// Count visible rows, not raw backend tasks, so the sidebar matches the list
				this.taskCountMap = {
					...this.taskCountMap,
					[type]: orderedData.length,
				};
				this.ensurePageInRange(type);
				this.updateTaskOrder(
					type,
					orderedData.map((task) => task.gid),
				);

				const visibleKeys = new Set(
					this.displayTaskList.map((row) => row._displayKey),
				);
				this.selectedGidList = this.selectedGidList.filter((key) =>
					visibleKeys.has(key),
				);
				return orderedData;
			} catch (err: unknown) {
				logger.warn("[Risuko] fetchList failed:", (err as Error).message);
				if (type !== this.currentList) {
					return [];
				}
				this.taskList = [];
				this.selectedGidList = [];
				return [];
			}
		},
		async updateTaskCountsFromStat(stat: Record<string, number>) {
			const numActive = stat.numActive || 0;
			const numWaiting = stat.numWaiting || 0;
			const numStoppedTotal = stat.numStoppedTotal || 0;

			let activeCount = numActive;
			let waitingCount = numWaiting;
			let completedCount = this.taskCountMap.completed || 0;
			let stoppedCount = this.taskCountMap.stopped || 0;
			let allCount = numActive + numWaiting + numStoppedTotal;

			// Fetch small projections so sidebar counts match visible rows
			// Skip the fetch when aria2 says there is nothing to count
			const needFetch = numActive + numWaiting + numStoppedTotal > 0;
			if (needFetch) {
				try {
					const [activeData, waitingData, stoppedData] = await Promise.all([
						numActive > 0
							? (api.fetchTaskList({
									type: "active",
									keys: ["gid", "status", "files", "bittorrent"],
								}) as Promise<DownloadTask[]>)
							: Promise.resolve([] as DownloadTask[]),
						numWaiting > 0
							? (api.fetchTaskList({
									type: "waiting",
									keys: ["gid", "status", "files", "bittorrent"],
								}) as Promise<DownloadTask[]>)
							: Promise.resolve([] as DownloadTask[]),
						numStoppedTotal > 0
							? (api.fetchTaskList({
									type: "stopped",
									keys: ["gid", "status", "files", "bittorrent"],
								}) as Promise<DownloadTask[]>)
							: Promise.resolve([] as DownloadTask[]),
					]);
					const activeArr = Array.isArray(activeData) ? activeData : [];
					const waitingArr = Array.isArray(waitingData) ? waitingData : [];
					const stoppedArr = Array.isArray(stoppedData) ? stoppedData : [];
					const completedArr = stoppedArr.filter(
						(t) => t.status === TASK_STATUS.COMPLETE,
					);
					const stoppedOnlyArr = stoppedArr.filter(
						(t) => t.status !== TASK_STATUS.COMPLETE,
					);
					activeCount = activeArr.length;
					waitingCount = waitingArr.length;
					completedCount = completedArr.length;
					stoppedCount = stoppedOnlyArr.length;
					allCount = activeCount + waitingCount + completedCount + stoppedCount;
				} catch {
					// Keep previous counts on failure
					activeCount = this.taskCountMap.active || 0;
					waitingCount = this.taskCountMap.waiting || 0;
					completedCount = this.taskCountMap.completed || 0;
					stoppedCount = this.taskCountMap.stopped || 0;
					allCount = this.taskCountMap.all || 0;
				}
			} else {
				completedCount = 0;
				stoppedCount = 0;
			}

			this.taskCountMap = {
				all: allCount,
				active: activeCount,
				waiting: waitingCount,
				completed: completedCount,
				stopped: stoppedCount,
			};
		},
		/**
		 * Sample speeds for all active/seeding tasks
		 * Called every polling tick from EngineClient
		 */
		async sampleActiveSpeeds() {
			try {
				// The active list already has speeds
				if (this.currentList === "active" && this.taskList.length > 0) {
					if (sampleSpeedsFromTasks(this.taskList)) {
						this.speedHistoryRev++;
					}
					return;
				}

				// Other tabs fetch a small active-task projection
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
				// Sampling is best effort
			}
		},
		selectTasks(list: string[]) {
			this.selectedGidList = list;
		},
		selectAllTask() {
			const selectableKeys = this.paginatedTaskList
				.map((task) => task._displayKey || task.gid || "")
				.filter(Boolean);
			const selectedKeys = new Set(this.selectedGidList);
			const allSelected =
				selectableKeys.length > 0 &&
				selectableKeys.every((key) => selectedKeys.has(key));
			this.selectedGidList = allSelected ? [] : selectableKeys;
		},
		async fetchItem(gid: string) {
			try {
				const data = await api.fetchTaskItem({ gid });
				this.updateCurrentTaskItem(data);
				return data;
			} catch (err: unknown) {
				logger.warn("[Risuko] fetchItem failed:", (err as Error).message);
				return null;
			}
		},
		async fetchItemWithPeers(gid: string) {
			try {
				const data = await api.fetchTaskItemWithPeers({ gid });
				if (!data) {
					return null;
				}
				this.updateCurrentTaskItem(data);
				return data;
			} catch (err: unknown) {
				logger.warn(
					"[Risuko] fetchItemWithPeers failed:",
					(err as Error).message,
				);
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
			this.updateCurrentTaskItem(task);
			this.currentTaskGid = task.gid;
			this.taskDetailVisible = true;
		},
		hideTaskDetail() {
			this.taskDetailVisible = false;
		},
		updateCurrentTaskItem(
			task: (DownloadTask & { peers?: PeerInfo[] }) | null,
		) {
			this.currentTaskItem = task;
			if (task) {
				this.currentTaskFiles = task.files;
				this.currentTaskPeers = task.peers ?? this.currentTaskPeers;
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
		addMedia(data: { url: string; options?: Record<string, unknown> }) {
			const { url, options } = data;
			return api.addMedia({ url, options }).then(() => {
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
		async addTorrents(data: {
			paths: string[];
			options: Record<string, unknown>;
		}) {
			const { paths, options } = data;
			const results = await api.addTorrentsByPaths({ paths, options });
			this.fetchList();
			const appStore = useAppStore();
			appStore.updateAddTaskOptions({});
			return results;
		},
		async addMetalinks(data: {
			paths: string[];
			options: Record<string, unknown>;
		}) {
			const { paths, options } = data;
			const results = await api.addMetalinksByPaths({ paths, options });
			this.fetchList();
			const appStore = useAppStore();
			appStore.updateAddTaskOptions({});
			return results;
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

			return api
				.removeTask({ gid })
				.then(() =>
					// Engine `remove()` only marks the task as Removed
					// Drop the record too so it disappears from every view, including all
					api.removeTaskRecord({ gid }).catch(() => undefined),
				)
				.finally(() => {
					speedHistoryCache.delete(gid);
					// Drop cloudflare retry flags for tasks that no longer exist
					useAppStore().clearCloudflareRetryFlag(gid);
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
			this.seedingList = this.seedingList.filter((g) => g !== gid);
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
			const gids: string[] = this.selectedGids;
			if (gids.length === 0) {
				return;
			}

			return api.batchResumeTask({ gids }).finally(() => {
				this.fetchList();
				this.saveSession();
			});
		},
		batchPauseSelectedTasks() {
			const gids: string[] = this.selectedGids;
			if (gids.length === 0) {
				return;
			}

			return api.batchPauseTask({ gids }).finally(() => {
				this.fetchList();
				this.saveSession();
			});
		},
		batchRemoveTask(gids: string[]) {
			return api
				.batchRemoveTask({ gids })
				.then(() =>
					// Drop records so removed rows disappear from every view
					Promise.all(
						gids.map((gid) =>
							api.removeTaskRecord({ gid }).catch(() => undefined),
						),
					),
				)
				.finally(() => {
					const appStore = useAppStore();
					for (const gid of gids) {
						speedHistoryCache.delete(gid);
						// Prune the cloudflare retry flag for removed tasks
						appStore.clearCloudflareRetryFlag(gid);
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
			const selectedGids = this.selectedGids;
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
