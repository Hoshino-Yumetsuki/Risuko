import {
	AUTO_SYNC_TRACKER_INTERVAL,
	MAX_NUM_OF_DIRECTORIES,
	MAX_NUM_OF_SAVED_CREDENTIALS,
} from "@shared/constants";
import { getLanguage } from "@shared/locales";
import { getCategoriesForKey } from "@shared/syncCategories";
import type { AppConfig } from "@shared/types/config";
import {
	CREDENTIAL_SECRET_FIELDS,
	type SavedCredential,
} from "@shared/types/credential";
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
import { useSyncStore } from "@/store/sync";
import { useTaskStore } from "@/store/task";

export const usePreferenceStore = defineStore("preference", {
	state: () => ({
		engineMode: "MAX",
		vaultEnabled: false,
		config: {
			locale: "auto",
		} as AppConfig,
	}),
	getters: {
		theme: (state) => state.config.theme,
		locale: (state) => state.config.locale,
		direction: (state) => getLangDirection(getLanguage(state.config.locale)),
	},
	actions: {
		async fetchPreference(): Promise<AppConfig> {
			try {
				const config = await api.fetchPreference();
				this.updatePreference(config);
				// Probe the vault before returning so callers awaiting fetchPreference
				// observe the correct vaultEnabled state on the very next save
				await this.fetchVaultStatus();
				return config;
			} catch (err: unknown) {
				logger.warn("[Risuko] fetchPreference failed:", (err as Error).message);
				return {} as AppConfig;
			}
		},
		save(config: Partial<AppConfig>, options?: { skipSync?: boolean }) {
			const taskStore = useTaskStore();
			taskStore.saveSession();

			if (isEmpty(config)) {
				return Promise.resolve();
			}

			// Update local sync timestamps for affected categories
			if (!options?.skipSync) {
				const timestamps = {
					...this.config.cloudSyncCategoryTimestamps,
				};
				const kebabConfig = changeKeysToKebabCase(config);
				let touched = false;
				for (const key of Object.keys(kebabConfig)) {
					for (const cat of getCategoriesForKey(key)) {
						timestamps[cat] = Math.floor(Date.now() / 1000);
						touched = true;
					}
				}
				if (touched) {
					config = { ...config, cloudSyncCategoryTimestamps: timestamps };
				}
			}

			// Round-trip through kebab→camelCase to normalize key names
			// (e.g. form uses m3u8OutputFormat, but lodash camelCase produces m3U8OutputFormat)
			const normalized = changeKeysToCamelCase(changeKeysToKebabCase(config));
			this.updatePreference(normalized);

			// Trigger cloud sync push if auto-sync is enabled
			if (!options?.skipSync) {
				try {
					useSyncStore().syncOnChange(config);
				} catch (err) {
					logger.warn("[Risuko] syncOnChange failed:", (err as Error).message);
				}
			}

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
		async fetchVaultStatus() {
			try {
				const { enabled } = await api.vaultStatus();
				this.vaultEnabled = enabled;
			} catch (err) {
				logger.warn("[Risuko] vaultStatus failed:", (err as Error).message);
				this.vaultEnabled = false;
			}
		},
		/**
		 * Split a credential into (metadata, secrets). Used when persisting:
		 * metadata is stored inline in `user.json`, secrets go to the OS
		 * keychain when the vault is available
		 */
		_splitCredentialSecrets(credential: SavedCredential): {
			meta: SavedCredential;
			secrets: Record<string, string>;
		} {
			const meta: SavedCredential = { ...credential };
			const secrets: Record<string, string> = {};
			for (const key of CREDENTIAL_SECRET_FIELDS) {
				const val = meta[key];
				if (typeof val === "string" && val.length > 0) {
					secrets[key] = val;
				}
				meta[key] = undefined;
			}
			return { meta, secrets };
		},
		async saveCredential(credential: SavedCredential) {
			const { savedCredentials = [] } = this.config;
			let toStore: SavedCredential = credential;

			if (this.vaultEnabled) {
				// Derive `wasVaulted` from the *persisted* record, not from
				// the incoming credential. Callers may construct a fresh
				// SavedCredential without the `vaulted` flag (e.g. sink-form
				// sync paths that don't carry vault metadata); trusting the
				// incoming flag would silently drop the OS-keychain entry on
				// every metadata-only save
				const persisted = savedCredentials.find(
					(c: SavedCredential) => c.id === credential.id,
				);
				const wasVaulted = !!persisted?.vaulted;
				const { meta, secrets } = this._splitCredentialSecrets(credential);
				// Strip the transient flag before persistence
				const explicitClear = !!meta.clearVault;
				delete meta.clearVault;
				try {
					if (Object.keys(secrets).length > 0) {
						await api.vaultPutCredential(credential.id, secrets);
						meta.vaulted = true;
					} else if (explicitClear) {
						// User asked to wipe the keychain entry
						await api.vaultRemoveCredential(credential.id);
						meta.vaulted = false;
					} else if (wasVaulted) {
						// Metadata-only edit on a vaulted credential — leave
						// the existing keychain entry alone
						meta.vaulted = true;
					} else {
						// No prior vault entry and no secret material — nothing
						// to do, just record the inline (empty) state
						meta.vaulted = false;
					}
					toStore = meta;
				} catch (err) {
					logger.warn(
						"[Risuko] vaultPutCredential failed, falling back to inline:",
						(err as Error).message,
					);
					// Best-effort cleanup of any stale vault entry so that the
					// inline fallback isn't silently shadowed by older keychain
					// secrets the next time the credential is loaded
					try {
						await api.vaultRemoveCredential(credential.id);
					} catch {
						// already logged above; nothing else we can do here
					}
					toStore = { ...credential, vaulted: false };
					delete (toStore as SavedCredential).clearVault;
				}
			} else if (credential.clearVault) {
				// Strip the transient flag even when the vault isn't in use
				toStore = { ...credential };
				delete (toStore as SavedCredential).clearVault;
			}

			const idx = savedCredentials.findIndex(
				(c: SavedCredential) => c.id === toStore.id,
			);
			let updated: SavedCredential[];
			if (idx >= 0) {
				updated = [...savedCredentials];
				updated[idx] = toStore;
			} else {
				updated = [toStore, ...savedCredentials];
				if (updated.length > MAX_NUM_OF_SAVED_CREDENTIALS) {
					updated = updated.slice(0, MAX_NUM_OF_SAVED_CREDENTIALS);
				}
			}
			await this.save({ savedCredentials: updated });
		},
		async removeCredential(id: string) {
			const { savedCredentials = [] } = this.config;
			const target = savedCredentials.find((c: SavedCredential) => c.id === id);
			if (target?.vaulted) {
				try {
					await api.vaultRemoveCredential(id);
				} catch (err) {
					logger.warn(
						"[Risuko] vaultRemoveCredential failed:",
						(err as Error).message,
					);
				}
			}
			const updated = savedCredentials.filter(
				(c: SavedCredential) => c.id !== id,
			);
			await this.save({ savedCredentials: updated });
		},
		/**
		 * Lazily hydrate the secret fields of a vaulted credential from the
		 * OS keychain. Returns a shallow copy with secrets populated; the
		 * returned object is NOT cached in the store
		 */
		async loadCredentialSecrets(
			credential: SavedCredential,
		): Promise<SavedCredential> {
			if (!credential.vaulted) {
				return credential;
			}
			if (!this.vaultEnabled) {
				// Stored credential expects the OS keychain but the backend is
				// unreachable in this session. Surface the issue so the user
				// understands why the form fields stay blank instead of failing
				// silently mid-download
				logger.warn(
					`[Risuko] credential ${credential.id} is vaulted but the OS keychain is unavailable; secrets will not be applied.`,
				);
				return credential;
			}
			try {
				const secrets = await api.vaultGetCredential(credential.id);
				if (!secrets) {
					logger.warn(
						`[Risuko] credential ${credential.id} marked vaulted but no entry in OS keychain.`,
					);
					return credential;
				}
				return { ...credential, ...secrets };
			} catch (err) {
				logger.warn(
					"[Risuko] vaultGetCredential failed:",
					(err as Error).message,
				);
				return credential;
			}
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
			this.updatePreference({ locale: locale || "auto" });
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
