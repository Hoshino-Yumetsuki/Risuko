import type { DryRunMatch, RssFeed, RssItem, RssRule } from "@shared/types/rss";
import logger from "@shared/utils/logger";
import { defineStore } from "pinia";
import api from "@/api";
import { usePreferenceStore } from "@/store/preference";

const DEFAULT_ITEMS_PER_PAGE = 20;
const ITEMS_PER_PAGE_OPTIONS = [10, 20, 30, 50, 100];
const STORAGE_KEY = "risuko.rss-items-per-page";
const PREFS_STORAGE_KEY = "risuko.rss-reader-prefs";

export type DensityMode = "compact" | "comfortable";
export type ReaderMode = "split" | "list";
export type SortMode = "newest" | "oldest" | "size-desc" | "rule-match";

interface ReaderPrefs {
	densityMode: DensityMode;
	readerMode: ReaderMode;
	unreadOnly: boolean;
	matchedOnly: boolean;
	sortMode: SortMode;
	groupByEpisode: boolean;
	markReadOnScroll: boolean;
}

const DEFAULT_PREFS: ReaderPrefs = {
	densityMode: "comfortable",
	readerMode: "list",
	unreadOnly: false,
	matchedOnly: false,
	sortMode: "newest",
	groupByEpisode: false,
	markReadOnScroll: false,
};

function loadItemsPerPage(): number {
	try {
		const raw = localStorage.getItem(STORAGE_KEY);
		const parsed = Number(raw);
		if (ITEMS_PER_PAGE_OPTIONS.includes(parsed)) {
			return parsed;
		}
	} catch {
		// ignore
	}
	return DEFAULT_ITEMS_PER_PAGE;
}

function loadPrefs(): ReaderPrefs {
	try {
		const raw = localStorage.getItem(PREFS_STORAGE_KEY);
		if (raw) {
			return { ...DEFAULT_PREFS, ...JSON.parse(raw) };
		}
	} catch {
		// ignore
	}
	return { ...DEFAULT_PREFS };
}

function savePrefs(prefs: ReaderPrefs) {
	try {
		localStorage.setItem(PREFS_STORAGE_KEY, JSON.stringify(prefs));
	} catch {
		// ignore
	}
}

export const useRssStore = defineStore("rss", {
	state: () => {
		const prefs = loadPrefs();
		return {
			feeds: [] as RssFeed[],
			currentFeedId: null as string | null,
			items: {} as Record<string, RssItem[]>,
			rules: [] as RssRule[],
			loading: false,
			selectedItemIds: [] as string[],
			selectedItemId: null as string | null,
			filterText: "",
			itemsPerPage: loadItemsPerPage(),
			currentPage: 1,
			densityMode: prefs.densityMode,
			readerMode: prefs.readerMode,
			unreadOnly: prefs.unreadOnly,
			matchedOnly: prefs.matchedOnly,
			sortMode: prefs.sortMode,
			groupByEpisode: prefs.groupByEpisode,
			markReadOnScroll: prefs.markReadOnScroll,
			_eventUnlisteners: [] as (() => void)[],
		};
	},

	getters: {
		currentFeed(): RssFeed | undefined {
			if (!this.currentFeedId) {
				return undefined;
			}
			return this.feeds.find((f) => f.id === this.currentFeedId);
		},
		currentItems(): RssItem[] {
			let items: RssItem[];
			if (!this.currentFeedId || this.currentFeedId === "__downloaded__") {
				const downloadedOnly = this.currentFeedId === "__downloaded__";
				const allItems: RssItem[] = [];
				for (const key of Object.keys(this.items)) {
					const feedItems = this.items[key];
					if (feedItems) {
						for (const item of feedItems) {
							if (!downloadedOnly || item.is_downloaded) {
								allItems.push(item);
							}
						}
					}
				}
				items = allItems;
			} else {
				items = (this.items[this.currentFeedId] ?? []).slice();
			}
			if (this.unreadOnly) {
				items = items.filter((i) => !i.is_read);
			}
			if (this.matchedOnly) {
				items = items.filter((i) => i.matched_rule_id);
			}
			const q = this.filterText.trim().toLowerCase();
			if (q) {
				items = items.filter((i) => i.title.toLowerCase().includes(q));
			}
			switch (this.sortMode) {
				case "oldest":
					items.sort((a, b) => (a.pub_date ?? 0) - (b.pub_date ?? 0));
					break;
				case "size-desc":
					items.sort(
						(a, b) => (b.enclosure_length ?? 0) - (a.enclosure_length ?? 0),
					);
					break;
				case "rule-match":
					items.sort((a, b) => {
						const am = a.matched_rule_id ? 1 : 0;
						const bm = b.matched_rule_id ? 1 : 0;
						if (am !== bm) {
							return bm - am;
						}
						return (b.pub_date ?? 0) - (a.pub_date ?? 0);
					});
					break;
				default:
					items.sort((a, b) => (b.pub_date ?? 0) - (a.pub_date ?? 0));
			}
			return items;
		},
		totalPages(): number {
			return Math.max(
				1,
				Math.ceil(this.currentItems.length / this.itemsPerPage),
			);
		},
		paginatedItems(): RssItem[] {
			const start = (this.currentPage - 1) * this.itemsPerPage;
			return this.currentItems.slice(start, start + this.itemsPerPage);
		},
		itemsPerPageOptions(): number[] {
			return ITEMS_PER_PAGE_OPTIONS;
		},
	},

	actions: {
		_handleRssDownloadComplete(data: {
			feedId?: string;
			itemId?: string;
			downloadPath?: string;
		}) {
			const { feedId, itemId, downloadPath } = data;
			if (!feedId || !itemId) {
				return;
			}
			const feedItems = this.items[feedId];
			if (feedItems) {
				const item = feedItems.find((i) => i.id === itemId);
				if (item) {
					item.is_downloaded = true;
					item.download_path = downloadPath;
				}
			}
		},
		_handleRssDownloadError(data: {
			feedId?: string;
			itemId?: string;
			reason?: string;
		}) {
			const { feedId, itemId, reason } = data;
			logger.warn(
				`[Risuko] RSS download failed: feed=${feedId} item=${itemId} reason=${reason || "error"}`,
			);
		},
		async initEventListeners() {
			this.cleanupEventListeners();
			try {
				const { listen } = await import("@tauri-apps/api/event");
				const unlistenComplete = await listen<{
					feedId?: string;
					itemId?: string;
					downloadPath?: string;
				}>("rss:download-complete", (event) => {
					this._handleRssDownloadComplete(event.payload ?? {});
				});
				const unlistenError = await listen<{
					feedId?: string;
					itemId?: string;
					reason?: string;
				}>("rss:download-error", (event) => {
					this._handleRssDownloadError(event.payload ?? {});
				});
				this._eventUnlisteners.push(unlistenComplete, unlistenError);
			} catch (err) {
				logger.warn(
					"[Risuko] RSS event listener setup failed:",
					(err as Error)?.message || err,
				);
			}
		},

		cleanupEventListeners() {
			for (const unlisten of this._eventUnlisteners) {
				unlisten();
			}
			this._eventUnlisteners = [];
		},

		async fetchFeeds() {
			this.feeds = (await api.getRssFeeds()) as RssFeed[];
		},

		async addFeed(url: string) {
			this.loading = true;
			try {
				const feed = (await api.addRssFeed(url)) as RssFeed;
				this.feeds.push(feed);
				return feed;
			} finally {
				this.loading = false;
			}
		},

		async removeFeed(feedId: string) {
			await api.removeRssFeed(feedId);
			this.feeds = this.feeds.filter((f) => f.id !== feedId);
			delete this.items[feedId];
			if (this.currentFeedId === feedId) {
				this.currentFeedId = null;
			}
		},

		async refreshFeed(feedId: string) {
			this.loading = true;
			try {
				await api.refreshRssFeed(feedId);
				await this.fetchItems(feedId);
			} finally {
				this.loading = false;
			}
		},

		async refreshAll() {
			this.loading = true;
			try {
				await api.refreshAllRssFeeds();
				await this.fetchFeeds();
				for (const feed of this.feeds) {
					await this.fetchItems(feed.id);
				}
				this.selectedItemIds = [];
				this.currentPage = 1;
			} finally {
				this.loading = false;
			}
		},

		selectFeed(feedId: string | null) {
			this.currentFeedId = feedId;
			this.currentPage = 1;
			this.selectedItemIds = [];
			if (feedId && feedId !== "__downloaded__") {
				this.fetchItems(feedId);
			}
		},

		async fetchItems(feedId: string) {
			const items = (await api.getRssItems(feedId)) as RssItem[];
			this.items[feedId] = items;
		},

		async downloadItem(feedId: string, itemId: string) {
			const options: Record<string, string> = {};

			// Use RSS category dir if configured
			const prefStore = usePreferenceStore();
			const categoryDirs = prefStore.config.fileCategoryDirs;
			if (categoryDirs?.rss) {
				options.dir = categoryDirs.rss;
			}

			// Build filename from feed title
			const feed = this.feeds.find((f) => f.id === feedId);
			if (feed) {
				const feedItems = this.items[feedId];
				const item = feedItems?.find((i) => i.id === itemId);
				if (item) {
					const url = item.enclosure_url || item.link || "";
					let ext = "";
					try {
						const lastSegment = new URL(url).pathname.split("/").pop() ?? "";
						const dotIdx = lastSegment.lastIndexOf(".");
						if (dotIdx > 0) {
							ext = lastSegment.slice(dotIdx);
						}
					} catch {
						// invalid URL, skip extension
					}
					const baseName = feed.title
						.replace(/\s+/g, "_")
						.replace(/[^\w\-_.]/g, "");
					options.out = `${baseName}_${item.title
						.replace(/\s+/g, "_")
						.replace(/[^\w\-_.]/g, "")
						.slice(0, 80)}${ext}`;
				}
			}

			try {
				// Use tracked download: backend monitors completion in a background task
				// and emits rss:download-complete / rss:download-error events.
				// This returns immediately with the gid, keeping the UI non-blocking.
				await api.downloadRssItemTracked(
					feedId,
					itemId,
					Object.keys(options).length > 0 ? options : undefined,
				);
			} catch (err) {
				logger.warn(
					"[Risuko] RSS download failed:",
					(err as Error)?.message || err,
				);
			}
		},

		async fetchRules() {
			this.rules = (await api.getRssRules()) as RssRule[];
		},

		async addRule(rule: Omit<RssRule, "id">) {
			const created = (await api.addRssRule(rule)) as RssRule;
			this.rules.push(created);
			return created;
		},

		async updateRule(rule: RssRule) {
			const updated = (await api.updateRssRule(rule)) as RssRule;
			const idx = this.rules.findIndex((r) => r.id === updated.id);
			if (idx >= 0) {
				this.rules[idx] = updated;
			} else {
				this.rules.push(updated);
			}
			return updated;
		},

		async reorderRules(orderedIds: string[]) {
			await api.reorderRssRules(orderedIds);
			await this.fetchRules();
		},

		async dryRunRule(rule: RssRule, sampleSize?: number) {
			return (await api.dryRunRssRule(rule, sampleSize)) as DryRunMatch[];
		},

		async removeRule(ruleId: string) {
			await api.removeRssRule(ruleId);
			this.rules = this.rules.filter((r) => r.id !== ruleId);
		},

		selectItem(itemId: string | null) {
			this.selectedItemId = itemId;
		},

		_persistPrefs() {
			savePrefs({
				densityMode: this.densityMode,
				readerMode: this.readerMode,
				unreadOnly: this.unreadOnly,
				matchedOnly: this.matchedOnly,
				sortMode: this.sortMode,
				groupByEpisode: this.groupByEpisode,
				markReadOnScroll: this.markReadOnScroll,
			});
		},

		setDensityMode(mode: DensityMode) {
			this.densityMode = mode;
			this._persistPrefs();
		},
		setReaderMode(mode: ReaderMode) {
			this.readerMode = mode;
			this._persistPrefs();
		},
		setUnreadOnly(value: boolean) {
			this.unreadOnly = value;
			this.currentPage = 1;
			this._persistPrefs();
		},
		setMatchedOnly(value: boolean) {
			this.matchedOnly = value;
			this.currentPage = 1;
			this._persistPrefs();
		},
		setSortMode(mode: SortMode) {
			this.sortMode = mode;
			this.currentPage = 1;
			this._persistPrefs();
		},
		setGroupByEpisode(value: boolean) {
			this.groupByEpisode = value;
			this._persistPrefs();
		},
		setMarkReadOnScroll(value: boolean) {
			this.markReadOnScroll = value;
			this._persistPrefs();
		},

		async markItemRead(feedId: string, itemId: string) {
			const feedItems = this.items[feedId];
			if (!feedItems) {
				return;
			}
			const item = feedItems.find((i) => i.id === itemId);
			if (item && !item.is_read) {
				// Optimistic update
				item.is_read = true;
				if (this.unreadOnly) {
					this.ensurePageInRange();
				}
				try {
					await api.markRssItemRead(feedId, itemId);
				} catch (_e) {
					// Revert on failure
					item.is_read = false;
					if (this.unreadOnly) {
						this.ensurePageInRange();
					}
				}
			}
		},

		async markAllRead(feedId?: string) {
			const feedIds = feedId ? [feedId] : Object.keys(this.items);
			const entries: [string, string][] = [];
			for (const fid of feedIds) {
				const feedItems = this.items[fid];
				if (!feedItems) {
					continue;
				}
				for (const item of feedItems) {
					if (!item.is_read) {
						item.is_read = true;
						entries.push([fid, item.id]);
					}
				}
			}
			if (this.unreadOnly) {
				this.ensurePageInRange();
			}
			if (entries.length > 0) {
				try {
					await api.markRssItemsRead(entries);
				} catch (_e) {
					// Revert on failure
					for (const [fid, itemId] of entries) {
						const it = this.items[fid]?.find((i) => i.id === itemId);
						if (it) {
							it.is_read = false;
						}
					}
					if (this.unreadOnly) {
						this.ensurePageInRange();
					}
				}
			}
		},

		async updateFeedSettings(
			feedId: string,
			interval?: number,
			isActive?: boolean,
		) {
			await api.updateRssFeedSettings(feedId, interval, isActive);
			const feed = this.feeds.find((f) => f.id === feedId);
			if (feed) {
				if (interval !== undefined) {
					feed.update_interval_secs = interval;
				}
				if (isActive !== undefined) {
					feed.is_active = isActive;
				}
			}
		},

		toggleItemSelection(itemId: string) {
			const idx = this.selectedItemIds.indexOf(itemId);
			if (idx >= 0) {
				this.selectedItemIds = this.selectedItemIds.filter(
					(id) => id !== itemId,
				);
			} else {
				this.selectedItemIds = [...this.selectedItemIds, itemId];
			}
		},

		selectAllItems() {
			this.selectedItemIds = this.paginatedItems.map((i) => i.id);
		},

		setFilterText(text: string) {
			this.filterText = text;
			this.selectedItemIds = [];
			this.currentPage = 1;
		},

		clearSelection() {
			this.selectedItemIds = [];
		},

		setItemsPerPage(value: number | string) {
			const num = Number(value);
			if (!ITEMS_PER_PAGE_OPTIONS.includes(num)) {
				return;
			}
			this.itemsPerPage = num;
			this.currentPage = 1;
			this.selectedItemIds = [];
			try {
				localStorage.setItem(STORAGE_KEY, String(num));
			} catch {
				// ignore
			}
		},

		changePage(page: number) {
			const clamped = Math.max(1, Math.min(page, this.totalPages));
			if (clamped !== this.currentPage) {
				this.currentPage = clamped;
				this.selectedItemIds = [];
			}
		},

		ensurePageInRange() {
			if (this.currentPage > this.totalPages) {
				this.currentPage = this.totalPages;
			}
		},

		async deleteItem(feedId: string, itemId: string) {
			await api.deleteRssItems([[feedId, [itemId]]]);
			const feedItems = this.items[feedId];
			if (feedItems) {
				this.items[feedId] = feedItems.filter((i) => i.id !== itemId);
			}
			this.selectedItemIds = this.selectedItemIds.filter((id) => id !== itemId);
			this.ensurePageInRange();
		},

		async clearItemDownload(feedId: string, itemId: string) {
			await api.clearRssDownload(feedId, itemId);
			const feedItems = this.items[feedId];
			if (feedItems) {
				const item = feedItems.find((i) => i.id === itemId);
				if (item) {
					item.is_downloaded = false;
					item.download_path = undefined;
				}
			}
		},

		async batchDownload() {
			const selected = new Set(this.selectedItemIds);
			const items = this.currentItems.filter(
				(i) =>
					selected.has(i.id) && !i.is_downloaded && (i.enclosure_url || i.link),
			);
			for (const item of items) {
				await this.downloadItem(item.feed_id, item.id);
			}
			this.selectedItemIds = [];
		},

		async batchDelete() {
			const selected = new Set(this.selectedItemIds);
			const items = this.currentItems.filter((i) => selected.has(i.id));
			// Group by feed_id
			const grouped = new Map<string, string[]>();
			for (const item of items) {
				const list = grouped.get(item.feed_id);
				if (list) {
					list.push(item.id);
				} else {
					grouped.set(item.feed_id, [item.id]);
				}
			}
			const itemsByFeed: [string, string[]][] = Array.from(grouped.entries());
			await api.deleteRssItems(itemsByFeed);
			// Remove locally
			for (const [feedId, itemIds] of itemsByFeed) {
				const feedItems = this.items[feedId];
				if (feedItems) {
					this.items[feedId] = feedItems.filter((i) => !itemIds.includes(i.id));
				}
			}
			this.selectedItemIds = [];
			this.ensurePageInRange();
		},

		unreadCount(feedId: string): number {
			const items = this.items[feedId] ?? [];
			return items.filter((i) => !i.is_read).length;
		},
	},
});
