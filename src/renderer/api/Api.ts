import { startupOnlyKeys } from "@shared/configKeys";
import type { AppConfig } from "@shared/types/config";
import type { RssRule } from "@shared/types/rss";
import type {
	AutoRetryPlanResult,
	DownloadTask,
	GlobalStat,
	LowSpeedEvaluationResult,
	PeerInfo,
	SyncOrderResult,
} from "@shared/types/task";
import {
	changeKeysToCamelCase,
	changeKeysToKebabCase,
	formatOptionsForEngine,
	separateConfig,
} from "@shared/utils";
import logger from "@shared/utils/logger";
import { invoke } from "@tauri-apps/api/core";
import { isEmpty } from "lodash";

const ENGINE_RESTART_USER_KEYS: string[] = [];
const DEFAULT_TASK_LIST_FETCH_SIZE = 1000;
const MAX_TASK_LIST_FETCH_SIZE = 2000;

const parseConfiguredTaskListFetchSize = () => {
	const meta = import.meta as unknown as Record<
		string,
		Record<string, unknown> | undefined
	>;
	const raw = meta?.env?.VITE_TASK_LIST_FETCH_SIZE;
	const parsed = Number(raw);
	if (!Number.isFinite(parsed) || parsed <= 0) {
		return DEFAULT_TASK_LIST_FETCH_SIZE;
	}
	return Math.min(Math.trunc(parsed), MAX_TASK_LIST_FETCH_SIZE);
};

const TASK_LIST_FETCH_SIZE = parseConfiguredTaskListFetchSize();

const clampTaskListFetchSize = (value: unknown) => {
	const parsed = Number(value);
	if (!Number.isFinite(parsed) || parsed <= 0) {
		return TASK_LIST_FETCH_SIZE;
	}

	const normalized = Math.trunc(parsed);
	if (normalized > MAX_TASK_LIST_FETCH_SIZE) {
		logger.warn(
			`[Motrix] task list fetch size ${normalized} exceeds cap ${MAX_TASK_LIST_FETCH_SIZE}, clamping`,
		);
		return MAX_TASK_LIST_FETCH_SIZE;
	}
	return normalized;
};

export default class Api {
	config: AppConfig | null;
	options: Record<string, unknown>;
	ready: Promise<void>;

	constructor(options: Record<string, unknown> = {}) {
		this.options = options;
		this.config = null;
		this.ready = this.init();
	}

	async init() {
		this.config = await this.loadConfig();
	}

	async ensureReady() {
		await this.ready;
	}

	async loadConfigFromNativeStore() {
		const result = await invoke("get_app_config");
		return result;
	}

	async loadConfig(): Promise<AppConfig> {
		let result = await this.loadConfigFromNativeStore();
		result = changeKeysToCamelCase(result);
		return result as AppConfig;
	}

	async fetchPreference() {
		this.config = await this.loadConfig();
		return this.config;
	}

	async savePreference(params: Partial<AppConfig> = {}) {
		let kebabParams = changeKeysToKebabCase(params);
		kebabParams = await invoke("prepare_preference_patch", {
			params: kebabParams,
		});

		const { user, system } = separateConfig(kebabParams);
		const hasStartupOnlySystemChanges = Object.keys(system).some((key) =>
			startupOnlyKeys.includes(key),
		);
		const hasEngineRestartUserChanges = ENGINE_RESTART_USER_KEYS.some((key) =>
			Object.hasOwn(user, key),
		);

		await this.savePreferenceToNativeStore(kebabParams);

		if (hasStartupOnlySystemChanges || hasEngineRestartUserChanges) {
			await invoke("restart_engine");
		}

		this.config = await this.loadConfig();
	}

	async savePreferenceToNativeStore(params: Record<string, unknown> = {}) {
		const { user, system, others } = separateConfig(params);
		const config: Record<string, Record<string, unknown>> = {};

		if (!isEmpty(user)) {
			logger.info("[Motrix] save user config: ", user);
			config.user = user;
		}

		if (!isEmpty(system)) {
			logger.info("[Motrix] save system config: ", system);
			config.system = system;

			// Startup-only keys cannot be applied to active tasks via changeOption.
			const runtimeSystemEntries = Object.entries(system).filter(
				([key]) => !startupOnlyKeys.includes(key),
			);
			const runtimeSystem = Object.fromEntries(runtimeSystemEntries);
			if (!isEmpty(runtimeSystem)) {
				await this.changeGlobalOption(runtimeSystem).catch((err) => {
					logger.warn(
						"[Motrix] changeGlobalOption failed:",
						err?.message || err,
					);
				});
				this.updateActiveTaskOption(runtimeSystem);
			}
		}

		if (!isEmpty(others)) {
			logger.info("[Motrix] save config found illegal key: ", others);
		}

		return invoke("save_preference", { config });
	}

	getVersion() {
		return invoke<string>("get_version");
	}

	changeGlobalOption(options: Record<string, unknown>) {
		const args = formatOptionsForEngine(options);
		return invoke("change_global_option_engine", { options: args });
	}

	getGlobalOption() {
		return invoke<Record<string, string>>("get_global_option_engine").then(
			(data) => changeKeysToCamelCase(data),
		);
	}

	getOption(params: { gid: string } = { gid: "" }) {
		const { gid } = params;
		return invoke("get_option_engine", { gid }).then((data) =>
			changeKeysToCamelCase(data),
		);
	}

	updateActiveTaskOption(options: Record<string, unknown>) {
		this.fetchTaskList({ type: "active" }).then((data) => {
			if (isEmpty(data)) {
				return;
			}
			const gids = (data as DownloadTask[]).map((task) => task.gid);
			this.batchChangeOption({ gids, options });
		});
	}

	changeOption(
		params: { gid: string; options?: Record<string, unknown> } = { gid: "" },
	) {
		const { gid, options = {} } = params;
		const engineOptions = formatOptionsForEngine(options);
		return invoke("change_option", { gid, options: engineOptions });
	}

	getGlobalStat() {
		return invoke<GlobalStat>("get_global_stat");
	}

	calculateActiveTaskProgress(params: { tasks?: DownloadTask[] } = {}) {
		const { tasks = [] } = params;
		return invoke<DownloadTask[]>("calculate_active_task_progress", { tasks });
	}

	evaluateLowSpeedTasks(
		params: {
			tasks?: DownloadTask[];
			thresholdBytes?: number;
			strikeThreshold?: number;
			cooldownMs?: number;
			nowMs?: number;
			strikeMap?: Record<string, number>;
			recoverAtMap?: Record<string, number>;
		} = {},
	) {
		const {
			tasks = [],
			thresholdBytes = 0,
			strikeThreshold = 3,
			cooldownMs = 30000,
			nowMs = Date.now(),
			strikeMap = {},
			recoverAtMap = {},
		} = params;
		return invoke<LowSpeedEvaluationResult>("evaluate_low_speed_tasks", {
			tasks,
			thresholdBytes,
			strikeThreshold,
			cooldownMs,
			nowMs,
			strikeMap,
			recoverAtMap,
		});
	}

	planAutoRetry(
		params: {
			gid?: string;
			strategy?: string;
			intervalSeconds?: number;
			maxDelayMs?: number;
			attemptMap?: Record<string, number>;
		} = {},
	) {
		const {
			gid = "",
			strategy = "static",
			intervalSeconds = 5,
			maxDelayMs = 15 * 60 * 1000,
			attemptMap = {},
		} = params;
		return invoke<AutoRetryPlanResult>("plan_auto_retry", {
			gid,
			strategy,
			intervalSeconds,
			maxDelayMs,
			attemptMap,
		});
	}

	syncSelectedTaskOrder(
		params: { direction?: string; selectedTasks?: DownloadTask[] } = {},
	) {
		const { direction = "up", selectedTasks = [] } = params;
		return invoke<SyncOrderResult>("sync_selected_task_order", {
			direction,
			selectedTasks,
		});
	}

	addUri(params: {
		uris: string[];
		outs: string[];
		options?: Record<string, unknown>;
	}) {
		const { uris, outs, options } = params;
		const engineOptions = formatOptionsForEngine(options);
		return invoke("add_uri", {
			uris,
			outs,
			options: engineOptions,
		});
	}

	addTorrent(params: {
		torrentPath: string;
		options?: Record<string, unknown>;
	}) {
		const { torrentPath, options } = params;
		const engineOptions = formatOptionsForEngine(options);

		if (typeof torrentPath !== "string" || !torrentPath.trim()) {
			throw new Error("task.new-task-torrent-required");
		}

		return invoke("add_torrent_by_path", {
			path: torrentPath,
			options: engineOptions,
		});
	}

	async fetchDownloadingTaskList(
		params: { offset?: number; num?: number; keys?: string[] } = {},
	) {
		const { offset = 0, num = TASK_LIST_FETCH_SIZE, keys } = params;
		const safeNum = clampTaskListFetchSize(num);
		const [active, waiting] = await Promise.all([
			invoke<DownloadTask[]>("tell_active", { keys }),
			invoke<DownloadTask[]>("tell_waiting", { offset, num: safeNum, keys }),
		]);
		const activeArr = Array.isArray(active) ? active : [];
		const waitingArr = Array.isArray(waiting) ? waiting : [];
		return [...activeArr, ...waitingArr];
	}

	fetchWaitingTaskList(
		params: { offset?: number; num?: number; keys?: string[] } = {},
	) {
		const { offset = 0, num = TASK_LIST_FETCH_SIZE, keys } = params;
		const safeNum = clampTaskListFetchSize(num);
		return invoke<DownloadTask[]>("tell_waiting", {
			offset,
			num: safeNum,
			keys,
		});
	}

	fetchStoppedTaskList(
		params: { offset?: number; num?: number; keys?: string[] } = {},
	) {
		const { offset = 0, num = TASK_LIST_FETCH_SIZE, keys } = params;
		const safeNum = clampTaskListFetchSize(num);
		return invoke<DownloadTask[]>("tell_stopped", {
			offset,
			num: safeNum,
			keys,
		});
	}

	fetchActiveTaskList(params: { keys?: string[] } = {}) {
		const { keys } = params;
		return invoke<DownloadTask[]>("tell_active", { keys });
	}

	fetchTaskList(
		params: {
			type?: string;
			offset?: number;
			num?: number;
			keys?: string[];
		} = {},
	) {
		const { type } = params;
		switch (type) {
			case "active":
				return this.fetchDownloadingTaskList(params);
			case "waiting":
				return this.fetchWaitingTaskList(params);
			case "stopped":
				return this.fetchStoppedTaskList(params);
			default:
				return this.fetchDownloadingTaskList(params);
		}
	}

	fetchTaskItem(params: { gid: string }) {
		const { gid } = params;
		return invoke<DownloadTask>("tell_status", { gid });
	}

	async fetchTaskItemWithPeers(params: {
		gid: string;
	}): Promise<(DownloadTask & { peers: PeerInfo[] }) | null> {
		const { gid } = params;
		const [status, peers] = await Promise.all([
			invoke<DownloadTask>("tell_status", { gid }),
			invoke<PeerInfo[]>("get_peers", { gid }),
		]);
		if (!status) {
			return null;
		}
		return {
			...status,
			peers: Array.isArray(peers) ? peers : [],
		};
	}

	pauseTask(params: { gid: string }) {
		const { gid } = params;
		return invoke("pause_task", { gid });
	}

	pauseAllTask() {
		return invoke("pause_all_tasks");
	}

	forcePauseTask(params: { gid: string }) {
		return this.pauseTask(params);
	}

	forcePauseAllTask() {
		return this.pauseAllTask();
	}

	resumeTask(params: { gid: string }) {
		const { gid } = params;
		return invoke("unpause_task", { gid });
	}

	resumeAllTask() {
		return invoke("unpause_all_tasks");
	}

	removeTask(params: { gid: string }) {
		const { gid } = params;
		return invoke("remove_task", { gid });
	}

	saveSession() {
		return invoke("save_session");
	}

	purgeTaskRecord() {
		return invoke("purge_download_result");
	}

	removeTaskRecord(params: { gid: string }) {
		const { gid } = params;
		return invoke("remove_download_result", { gid });
	}

	multicall(
		method: string,
		params: { gids?: string[]; options?: Record<string, unknown> } = {},
	) {
		const { gids, options = {} } = params;
		const engineOptions = formatOptionsForEngine(options);
		return invoke("multicall_engine", { method, gids, options: engineOptions });
	}

	batchChangeOption(
		params: { gids?: string[]; options?: Record<string, unknown> } = {},
	) {
		return this.multicall("motrix.changeOption", params);
	}

	batchRemoveTask(params: { gids?: string[] } = {}) {
		return this.multicall("motrix.remove", params);
	}

	batchResumeTask(params: { gids?: string[] } = {}) {
		return this.multicall("motrix.unpause", params);
	}

	batchPauseTask(params: { gids?: string[] } = {}) {
		return this.multicall("motrix.pause", params);
	}

	batchForcePauseTask(params: { gids?: string[] } = {}) {
		return this.multicall("motrix.forcePause", params);
	}

	// ── RSS ──────────────────────────────────────────────────────

	addRssFeed(url: string) {
		return invoke("add_rss_feed", { url });
	}

	removeRssFeed(feedId: string) {
		return invoke("remove_rss_feed", { feedId });
	}

	refreshRssFeed(feedId: string) {
		return invoke("refresh_rss_feed", { feedId });
	}

	refreshAllRssFeeds() {
		return invoke("refresh_all_rss_feeds");
	}

	getRssFeeds() {
		return invoke("get_rss_feeds");
	}

	getRssItems(feedId: string) {
		return invoke("get_rss_items", { feedId });
	}

	updateRssFeedSettings(feedId: string, interval?: number, isActive?: boolean) {
		return invoke("update_rss_feed_settings", { feedId, interval, isActive });
	}

	addRssRule(rule: Omit<RssRule, "id">) {
		return invoke("add_rss_rule", { rule });
	}

	removeRssRule(ruleId: string) {
		return invoke("remove_rss_rule", { ruleId });
	}

	getRssRules() {
		return invoke("get_rss_rules");
	}

	downloadRssItem(
		feedId: string,
		itemId: string,
		options?: Record<string, unknown>,
	) {
		return invoke("download_rss_item", { feedId, itemId, options });
	}

	deleteRssItems(itemsByFeed: [string, string[]][]) {
		return invoke("delete_rss_items", { itemsByFeed });
	}

	clearRssDownload(feedId: string, itemId: string) {
		return invoke("clear_rss_download", { feedId, itemId });
	}

	markRssDownloaded(feedId: string, itemId: string, downloadPath?: string) {
		return invoke("mark_rss_downloaded", { feedId, itemId, downloadPath });
	}

	readRssDownload(feedId: string, itemId: string) {
		return invoke("read_rss_download", { feedId, itemId }) as Promise<string>;
	}

	downloadRssItemTracked(
		feedId: string,
		itemId: string,
		options?: Record<string, unknown>,
	) {
		return invoke("download_rss_item_tracked", { feedId, itemId, options });
	}

	inferOutFromUri(uri: string) {
		return invoke<string>("infer_out_from_uri", { uri });
	}

	resolveFileCategory(filename: string) {
		return invoke<string>("resolve_file_category", { filename });
	}
}
