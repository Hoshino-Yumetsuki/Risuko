import {
	AUTO_SYNC_TRACKER_INTERVAL,
	MAX_NUM_OF_DIRECTORIES,
	MAX_NUM_OF_SAVED_CREDENTIALS,
} from "@shared/constants";
import type { AppConfig } from "@shared/types/config";
import type { SavedCredential } from "@shared/types/credential";
import {
	changeKeysToCamelCase,
	changeKeysToKebabCase,
	getLangDirection,
	pushItemToFixedLengthArray,
	removeArrayItem,
} from "@shared/utils";
import logger from "@shared/utils/logger";
import {
	fetchBtTrackerFromSource,
	reduceTrackerString,
} from "@shared/utils/tracker";
import { isEmpty } from "lodash";
import { defineStore } from "pinia";
import api from "@/api";
import { useTaskStore } from "@/store/task";

export const usePreferenceStore = defineStore("preference", {
	state: () => ({
		engineMode: "MAX",
		config: {
			locale: "en-US",
		} as AppConfig,
	}),
	getters: {
		theme: (state) => state.config.theme,
		locale: (state) => state.config.locale,
		direction: (state) => getLangDirection(state.config.locale),
	},
	actions: {
		async fetchPreference(): Promise<AppConfig> {
			try {
				const config = await api.fetchPreference();
				this.updatePreference(config);
				return config;
			} catch (err: unknown) {
				logger.warn("[Risuko] fetchPreference failed:", (err as Error).message);
				return {} as AppConfig;
			}
		},
		save(config: Partial<AppConfig>) {
			const taskStore = useTaskStore();
			taskStore.saveSession();

			if (isEmpty(config)) {
				return Promise.resolve();
			}

			// Round-trip through kebab→camelCase to normalize key names
			// (e.g. form uses m3u8OutputFormat, but lodash camelCase produces m3U8OutputFormat)
			const normalized = changeKeysToCamelCase(changeKeysToKebabCase(config));
			this.updatePreference(normalized);
			return Promise.resolve(api.savePreference(config));
		},
		recordHistoryDirectory(directory: string) {
			const { historyDirectories = [], favoriteDirectories = [] } = this.config;
			const all = new Set([...historyDirectories, ...favoriteDirectories]);
			if (all.has(directory)) {
				return;
			}

			this.addHistoryDirectory(directory);
		},
		addHistoryDirectory(directory: string) {
			const { historyDirectories = [] } = this.config;
			const history = pushItemToFixedLengthArray(
				historyDirectories,
				MAX_NUM_OF_DIRECTORIES,
				directory,
			);

			this.save({ historyDirectories: history });
		},
		favoriteDirectory(directory: string) {
			const { historyDirectories = [], favoriteDirectories = [] } = this.config;
			if (
				favoriteDirectories.includes(directory) ||
				favoriteDirectories.length >= MAX_NUM_OF_DIRECTORIES
			) {
				return;
			}

			const favorite = pushItemToFixedLengthArray(
				favoriteDirectories,
				MAX_NUM_OF_DIRECTORIES,
				directory,
			);
			const history = removeArrayItem(historyDirectories, directory);

			this.save({
				historyDirectories: history,
				favoriteDirectories: favorite,
			});
		},
		cancelFavoriteDirectory(directory: string) {
			const { historyDirectories = [], favoriteDirectories = [] } = this.config;
			if (historyDirectories.includes(directory)) {
				return;
			}

			const favorite = removeArrayItem(favoriteDirectories, directory);
			const history = pushItemToFixedLengthArray(
				historyDirectories,
				MAX_NUM_OF_DIRECTORIES,
				directory,
			);

			this.save({
				historyDirectories: history,
				favoriteDirectories: favorite,
			});
		},
		removeDirectory(directory: string) {
			const { historyDirectories = [], favoriteDirectories = [] } = this.config;

			const favorite = removeArrayItem(favoriteDirectories, directory);
			const history = removeArrayItem(historyDirectories, directory);

			this.save({
				historyDirectories: history,
				favoriteDirectories: favorite,
			});
		},
		getSavedCredentials(): SavedCredential[] {
			const { savedCredentials = [] } = this.config;
			return [...savedCredentials].sort(
				(a: SavedCredential, b: SavedCredential) =>
					(b.lastUsedAt || 0) - (a.lastUsedAt || 0),
			);
		},
		saveCredential(credential: SavedCredential) {
			const { savedCredentials = [] } = this.config;
			const idx = savedCredentials.findIndex(
				(c: SavedCredential) => c.id === credential.id,
			);
			let updated: SavedCredential[];
			if (idx >= 0) {
				updated = [...savedCredentials];
				updated[idx] = credential;
			} else {
				updated = [credential, ...savedCredentials];
				if (updated.length > MAX_NUM_OF_SAVED_CREDENTIALS) {
					updated = updated.slice(0, MAX_NUM_OF_SAVED_CREDENTIALS);
				}
			}
			this.save({ savedCredentials: updated });
		},
		removeCredential(id: string) {
			const { savedCredentials = [] } = this.config;
			const updated = savedCredentials.filter(
				(c: SavedCredential) => c.id !== id,
			);
			this.save({ savedCredentials: updated });
		},
		updateCredentialLastUsed(id: string) {
			const { savedCredentials = [] } = this.config;
			const idx = savedCredentials.findIndex(
				(c: SavedCredential) => c.id === id,
			);
			if (idx < 0) {
				return;
			}
			const updated = [...savedCredentials];
			updated[idx] = { ...updated[idx], lastUsedAt: Date.now() };
			this.save({ savedCredentials: updated });
		},
		findCredentialsByHost(host: string, protocol?: string): SavedCredential[] {
			const { savedCredentials = [] } = this.config;
			const lower = host.toLowerCase();
			return savedCredentials
				.filter((c: SavedCredential) => {
					if (c.host && c.host.toLowerCase() === lower) {
						if (protocol && c.protocol) {
							return c.protocol === protocol;
						}
						return true;
					}
					return false;
				})
				.sort(
					(a: SavedCredential, b: SavedCredential) =>
						(b.lastUsedAt || 0) - (a.lastUsedAt || 0),
				);
		},
		updateAppTheme(theme: string) {
			this.updatePreference({ theme });
		},
		updateAppLocale(locale: string) {
			this.updatePreference({ locale: locale || "en-US" });
		},
		updatePreference(config: Partial<AppConfig>) {
			this.config = { ...this.config, ...config };
		},
		fetchBtTracker(trackerSource: string[] = []) {
			const { proxy = { enable: false } } = this.config;
			logger.log("fetchBtTracker", trackerSource, proxy);
			return fetchBtTrackerFromSource(trackerSource, proxy);
		},
		async autoSyncTracker() {
			const config = this.config;
			if (!config.autoSyncTracker) {
				return;
			}

			const lastSync = config.lastSyncTrackerTime || 0;
			if (Date.now() - lastSync < AUTO_SYNC_TRACKER_INTERVAL) {
				return;
			}

			const trackerSource = config.trackerSource || [];
			if (!trackerSource.length) {
				return;
			}

			try {
				const data = await this.fetchBtTracker(trackerSource);
				const tracker = data.join(",").replace(/^\s*,|,\s*$/g, "");
				if (!tracker) {
					return;
				}
				await this.save({
					btTracker: reduceTrackerString(tracker),
					lastSyncTrackerTime: Date.now(),
				});
				logger.info("[Risuko] auto-sync tracker done");
			} catch (err) {
				logger.warn("[Risuko] auto-sync tracker failed:", err);
			}
		},
		toggleEngineMode() {
			const nextMode = this.engineMode === "MAX" ? "LIMIT" : "MAX";
			this.engineMode = nextMode;

			const config = this.config;
			const isMax = nextMode === "MAX";
			api.changeGlobalOption({
				"max-overall-download-limit": isMax
					? 0
					: (config.maxOverallDownloadLimit ?? 0),
				"max-overall-upload-limit": isMax
					? 0
					: (config.maxOverallUploadLimit ?? 0),
			});
		},
	},
});
