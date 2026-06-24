import { getCategoriesForKey, syncCategories } from "@shared/syncCategories";
import { changeKeysToKebabCase } from "@shared/utils";
import logger from "@shared/utils/logger";
import axios from "axios";
import { defineStore } from "pinia";
import api from "@/api";
import { useAuthStore } from "./auth";
import { usePreferenceStore } from "./preference";

function getAxiosErrorMessage(err: unknown): string {
	const response = (
		err as { response?: { data?: { error?: string }; status?: number } }
	).response;
	if (response?.data?.error) {
		return response.data.error;
	}
	if (response?.status === 429) {
		return "Too many requests. Please try again later.";
	}
	return (err as Error)?.message || "Sync failed";
}

interface SyncState {
	syncing: boolean;
	lastSyncAt: number | null;
}

let pushTimer: ReturnType<typeof setTimeout> | null = null;
const pendingCategories = new Set<string>();

export const useSyncStore = defineStore("sync", {
	state: (): SyncState => ({
		syncing: false,
		lastSyncAt: null,
	}),
	actions: {
		getSelectedCategories(): string[] {
			const preferenceStore = usePreferenceStore();
			return (preferenceStore.config.cloudSyncCategories as string[]) || [];
		},

		isAutoSyncEnabled(): boolean {
			const preferenceStore = usePreferenceStore();
			return (
				!!preferenceStore.config.cloudSyncAuto && !!useAuthStore().isLoggedIn
			);
		},

		extractCategoryData(categoryId: string): Record<string, unknown> {
			const cat = syncCategories.find((c) => c.id === categoryId);
			if (!cat) {
				return {};
			}

			const preferenceStore = usePreferenceStore();
			const config = changeKeysToKebabCase(
				preferenceStore.config || {},
			) as Record<string, unknown>;
			const data: Record<string, unknown> = {};
			for (const key of cat.keys) {
				if (key in config) {
					data[key] = config[key];
				}
			}
			return data;
		},

		getCategoryTimestamp(categoryId: string): number | undefined {
			const preferenceStore = usePreferenceStore();
			const timestamps = preferenceStore.config.cloudSyncCategoryTimestamps as
				| Record<string, number>
				| undefined;
			return timestamps?.[categoryId];
		},

		async setCategoryTimestamp(
			categoryId: string,
			timestamp: number,
		): Promise<void> {
			const preferenceStore = usePreferenceStore();
			const timestamps = {
				...preferenceStore.config.cloudSyncCategoryTimestamps,
			};
			timestamps[categoryId] = timestamp;
			await preferenceStore.save(
				{ cloudSyncCategoryTimestamps: timestamps },
				{ skipSync: true },
			);
		},

		async pushCategory(categoryId: string): Promise<number | undefined> {
			const authStore = useAuthStore();
			if (!authStore.isLoggedIn || !authStore.token) {
				return;
			}

			const data = this.extractCategoryData(categoryId);
			if (Object.keys(data).length === 0) {
				return;
			}

			const { data: resp } = await axios.put(
				`${authStore.serverUrl}/settings`,
				{ category: categoryId, data },
				{ headers: authStore.getAuthHeaders() },
			);
			return resp.updatedAt as number;
		},

		async pushAll(): Promise<void> {
			const authStore = useAuthStore();
			if (!authStore.isLoggedIn || !authStore.token) {
				throw new Error("Not logged in");
			}

			this.syncing = true;
			try {
				const categories = this.getSelectedCategories();
				for (const categoryId of categories) {
					const updatedAt = await this.pushCategory(categoryId);
					if (updatedAt) {
						await this.setCategoryTimestamp(categoryId, updatedAt);
					}
				}
				this.lastSyncAt = Date.now();
				await api.savePreference({ cloudSyncLastAt: this.lastSyncAt });
				logger.info("[Risuko] sync pushAll done");
			} catch (err) {
				const message = getAxiosErrorMessage(err);
				logger.warn("[Risuko] sync pushAll failed:", message);
				throw new Error(message);
			} finally {
				this.syncing = false;
			}
		},

		async pullCategory(categoryId: string): Promise<void> {
			const authStore = useAuthStore();
			if (!authStore.isLoggedIn || !authStore.token) {
				return;
			}

			const { data } = await axios.get(
				`${authStore.serverUrl}/settings/${categoryId}`,
				{ headers: authStore.getAuthHeaders() },
			);

			if (!data.data) {
				return;
			}

			const preferenceStore = usePreferenceStore();
			await preferenceStore.save(data.data, { skipSync: true });
			if (data.updatedAt) {
				await this.setCategoryTimestamp(categoryId, data.updatedAt as number);
			}
		},

		async pullAll(): Promise<void> {
			const authStore = useAuthStore();
			if (!authStore.isLoggedIn || !authStore.token) {
				throw new Error("Not logged in");
			}

			this.syncing = true;
			try {
				const { data } = await axios.get(`${authStore.serverUrl}/settings`, {
					headers: authStore.getAuthHeaders(),
				});

				const preferenceStore = usePreferenceStore();
				const selectedCategories = this.getSelectedCategories();
				const settings = data.settings || {};
				const timestamps = data.timestamps || {};
				const merged: Record<string, unknown> = {};

				for (const categoryId of selectedCategories) {
					if (settings[categoryId]) {
						Object.assign(merged, settings[categoryId]);
						if (timestamps[categoryId]) {
							await this.setCategoryTimestamp(
								categoryId,
								timestamps[categoryId] as number,
							);
						}
					}
				}

				if (Object.keys(merged).length > 0) {
					await preferenceStore.save(merged, { skipSync: true });
				}

				this.lastSyncAt = Date.now();
				await preferenceStore.save({ cloudSyncLastAt: this.lastSyncAt });
				logger.info("[Risuko] sync pullAll done");
			} catch (err) {
				const message = getAxiosErrorMessage(err);
				logger.warn("[Risuko] sync pullAll failed:", message);
				throw new Error(message);
			} finally {
				this.syncing = false;
			}
		},

		async syncBidirectional(): Promise<void> {
			const authStore = useAuthStore();
			if (!authStore.isLoggedIn || !authStore.token) {
				throw new Error("Not logged in");
			}

			this.syncing = true;
			try {
				const { data } = await axios.get(`${authStore.serverUrl}/settings`, {
					headers: authStore.getAuthHeaders(),
				});

				const preferenceStore = usePreferenceStore();
				const selectedCategories = this.getSelectedCategories();
				const remoteSettings = data.settings || {};
				const remoteTimestamps = data.timestamps || {};

				const toPull: Record<string, unknown> = {};
				const toPullTimestamps: Record<string, number> = {};

				for (const categoryId of selectedCategories) {
					const localTimestamp = this.getCategoryTimestamp(categoryId);
					const remoteTimestamp = remoteTimestamps[categoryId] as
						| number
						| undefined;
					const localData = this.extractCategoryData(categoryId);
					const remoteData = remoteSettings[categoryId] as
						| Record<string, unknown>
						| undefined;
					const hasLocal = Object.keys(localData).length > 0;
					const hasRemote = remoteTimestamp !== undefined;

					if (!hasLocal && !hasRemote) {
						continue;
					}

					if (!hasLocal && hasRemote && remoteData) {
						Object.assign(toPull, remoteData);
						toPullTimestamps[categoryId] = remoteTimestamp;
						continue;
					}

					if (hasLocal && !hasRemote) {
						const updatedAt = await this.pushCategory(categoryId);
						if (updatedAt) {
							await this.setCategoryTimestamp(categoryId, updatedAt);
						}
						continue;
					}

					if (localTimestamp === undefined && hasRemote && remoteData) {
						Object.assign(toPull, remoteData);
						toPullTimestamps[categoryId] = remoteTimestamp;
						continue;
					}

					if (localTimestamp && remoteTimestamp) {
						if (localTimestamp > remoteTimestamp) {
							const updatedAt = await this.pushCategory(categoryId);
							if (updatedAt) {
								await this.setCategoryTimestamp(categoryId, updatedAt);
							}
						} else if (remoteTimestamp > localTimestamp && remoteData) {
							Object.assign(toPull, remoteData);
							toPullTimestamps[categoryId] = remoteTimestamp;
						}
						// Equal: skip
					}
				}

				if (Object.keys(toPull).length > 0) {
					await preferenceStore.save(toPull, { skipSync: true });
					for (const [cat, ts] of Object.entries(toPullTimestamps)) {
						await this.setCategoryTimestamp(cat, ts);
					}
				}

				this.lastSyncAt = Date.now();
				await preferenceStore.save({ cloudSyncLastAt: this.lastSyncAt });
				logger.info("[Risuko] sync bidirectional done");
			} catch (err) {
				const message = getAxiosErrorMessage(err);
				logger.warn("[Risuko] sync bidirectional failed:", message);
				throw new Error(message);
			} finally {
				this.syncing = false;
			}
		},

		async syncNow(): Promise<void> {
			await this.syncBidirectional();
		},

		async restoreFromCloud(): Promise<void> {
			await this.pullAll();
		},

		async fullSync(): Promise<void> {
			await this.syncBidirectional();
		},

		async syncOnStartup(): Promise<void> {
			if (!this.isAutoSyncEnabled()) {
				return;
			}
			try {
				await this.syncBidirectional();
			} catch (err) {
				logger.warn("[Risuko] syncOnStartup failed:", (err as Error).message);
			}
		},

		syncOnChange(changedConfig: Record<string, unknown>): void {
			if (!this.isAutoSyncEnabled()) {
				return;
			}

			const kebabConfig = changeKeysToKebabCase(changedConfig);

			for (const key of Object.keys(kebabConfig)) {
				const cats = getCategoriesForKey(key);
				for (const cat of cats) {
					if (this.getSelectedCategories().includes(cat)) {
						pendingCategories.add(cat);
					}
				}
			}

			if (pendingCategories.size === 0) {
				return;
			}

			if (pushTimer) {
				clearTimeout(pushTimer);
			}

			pushTimer = setTimeout(async () => {
				pushTimer = null;
				const categories = [...pendingCategories];
				pendingCategories.clear();
				try {
					for (const categoryId of categories) {
						const updatedAt = await this.pushCategory(categoryId);
						if (updatedAt) {
							await this.setCategoryTimestamp(categoryId, updatedAt);
						}
					}
					this.lastSyncAt = Date.now();
					await api.savePreference({ cloudSyncLastAt: this.lastSyncAt });
				} catch (err) {
					logger.warn("[Risuko] syncOnChange failed:", (err as Error).message);
				}
			}, 500);
		},

		async deleteCategoryData(categoryId: string): Promise<void> {
			const authStore = useAuthStore();
			if (!authStore.isLoggedIn || !authStore.token) {
				return;
			}

			await axios.delete(`${authStore.serverUrl}/settings/${categoryId}`, {
				headers: authStore.getAuthHeaders(),
			});
		},
	},
});
