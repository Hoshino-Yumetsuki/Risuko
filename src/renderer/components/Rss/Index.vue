<template>
  <div class="main panel panel-layout panel-layout--h">
    <aside class="subnav hidden-xs-only subnav-pane">
      <mo-rss-feed-list @add-feed="showAddFeedDialog = true" />
    </aside>
    <div class="content panel panel-layout panel-layout--v relative">
      <header class="panel-header">
        <h4 class="task-title hidden-xs-only">
          {{ currentFeed ? currentFeed.title : isDownloadedView ? $t('rss.downloaded') : $t('rss.all-items') }}
        </h4>
        <div class="task-actions">
          <div class="task-page-size">
            <Select :model-value="`${itemsPerPage}`" @update:model-value="onItemsPerPageChange">
              <SelectTrigger size="sm" class="task-page-size-trigger">
                <SelectValue />
              </SelectTrigger>
              <SelectContent align="end">
                <SelectItem v-for="size in itemsPerPageOptions" :key="size" :value="`${size}`">
                  {{ $t('rss.items-per-page', { count: size }) }}
                </SelectItem>
              </SelectContent>
            </Select>
          </div>
          <i
            class="task-action"
            :class="{ disabled: selectedCount === 0 }"
            :title="$t('rss.delete-selected')"
            @click="handleBatchDelete"
          >
            <Trash2 :size="14" />
          </i>
          <i
            class="task-action"
            :class="{ disabled: downloadableSelectedCount === 0 }"
            :title="$t('rss.download-selected')"
            @click="handleBatchDownload"
          >
            <Download :size="14" />
          </i>
          <i
            class="task-action"
            :title="$t('rss.refresh')"
            @click="handleRefreshAll"
          >
            <RefreshCw :size="14" :class="{ 'animate-spin': loading }" />
          </i>
          <i
            class="task-action"
            :title="$t('rss.add-feed')"
            @click="showAddFeedDialog = true"
          >
            <Plus :size="14" />
          </i>
        </div>
      </header>
      <div v-if="feeds.length > 0" class="task-toolbar">
        <div class="task-toolbar-left">
          <div class="task-filter-input">
            <Search :size="12" class="task-filter-icon" />
            <input
              type="text"
              class="task-filter-field"
              :placeholder="$t('rss.filter-placeholder')"
              :value="filterText"
              @input="onFilterInput"
            />
            <button
              v-if="filterText"
              class="task-filter-clear"
              @click="onFilterClear"
            >
              <X :size="12" />
            </button>
          </div>
        </div>
        <div class="task-toolbar-right">
          <div class="rss-select-all" @click="toggleSelectAll">
            <Checkbox
              :model-value="selectAllState"
              @update:model-value="toggleSelectAll"
              @click.stop
            />
            <span class="rss-select-label">
              {{ selectedCount > 0 ? $t('rss.selected-count', { count: selectedCount }) : $t('rss.select-all') }}
            </span>
          </div>
        </div>
      </div>
      <main class="panel-content">
        <mo-rss-item-list v-if="feeds.length > 0" />
        <div v-else class="no-task">
          <div class="rss-empty-inner">
            <Rss :size="48" class="rss-empty-icon" />
            <p>{{ $t('rss.no-feeds') }}</p>
            <Button size="sm" @click="showAddFeedDialog = true">
              <Plus :size="14" />
              {{ $t('rss.add-feed') }}
            </Button>
          </div>
        </div>
      </main>
    </div>
    <mo-rss-add-feed-dialog
      :visible="showAddFeedDialog"
      @close="showAddFeedDialog = false"
    />
    <mo-rss-feed-settings-dialog
      :visible="showFeedSettingsDialog"
      :feed-id="editingFeedId"
      @close="showFeedSettingsDialog = false"
    />
  </div>
</template>

<script lang="ts">
import {
	Download,
	Plus,
	RefreshCw,
	Rss,
	Search,
	Trash2,
	X,
} from "lucide-vue-next";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
	Select,
	SelectContent,
	SelectItem,
	SelectTrigger,
	SelectValue,
} from "@/components/ui/select";
import { useRssStore } from "@/store/rss";
import RssAddFeedDialog from "./AddFeedDialog.vue";
import RssFeedList from "./FeedList.vue";
import RssFeedSettingsDialog from "./FeedSettingsDialog.vue";
import RssItemList from "./ItemList.vue";

export default {
	name: "mo-rss-index",
	components: {
		Button,
		Checkbox,
		Download,
		Plus,
		RefreshCw,
		Rss,
		Search,
		Select,
		SelectContent,
		SelectItem,
		SelectTrigger,
		SelectValue,
		Trash2,
		X,
		"mo-rss-feed-list": RssFeedList,
		"mo-rss-item-list": RssItemList,
		"mo-rss-add-feed-dialog": RssAddFeedDialog,
		"mo-rss-feed-settings-dialog": RssFeedSettingsDialog,
	},
	data() {
		return {
			showAddFeedDialog: false,
			showFeedSettingsDialog: false,
			editingFeedId: "",
		};
	},
	computed: {
		feeds() {
			return useRssStore().feeds;
		},
		currentFeed() {
			return useRssStore().currentFeed;
		},
		isDownloadedView() {
			return useRssStore().currentFeedId === "__downloaded__";
		},
		loading() {
			return useRssStore().loading;
		},
		filterText() {
			return useRssStore().filterText;
		},
		itemsPerPage() {
			return useRssStore().itemsPerPage;
		},
		itemsPerPageOptions() {
			return useRssStore().itemsPerPageOptions;
		},
		selectedCount() {
			return useRssStore().selectedItemIds.length;
		},
		pageItemCount() {
			return useRssStore().paginatedItems.length;
		},
		allSelected() {
			return (
				this.pageItemCount > 0 && this.selectedCount === this.pageItemCount
			);
		},
		selectAllState(): boolean | "indeterminate" {
			if (this.allSelected) {
				return true;
			}
			if (this.selectedCount > 0) {
				return "indeterminate";
			}
			return false;
		},
		downloadableSelectedCount() {
			const store = useRssStore();
			const selected = new Set(store.selectedItemIds);
			return store.paginatedItems.filter(
				(i) =>
					selected.has(i.id) && !i.is_downloaded && (i.enclosure_url || i.link),
			).length;
		},
	},
	async created() {
		const store = useRssStore();
		store.initEventListeners();
		await store.fetchFeeds();
		for (const feed of store.feeds) {
			await store.fetchItems(feed.id);
		}
	},
	beforeUnmount() {
		useRssStore().cleanupEventListeners();
	},
	methods: {
		onFilterInput(event: Event) {
			useRssStore().setFilterText((event.target as HTMLInputElement).value);
		},
		onFilterClear() {
			useRssStore().setFilterText("");
		},
		onItemsPerPageChange(value: string) {
			useRssStore().setItemsPerPage(value);
		},
		toggleSelectAll() {
			const store = useRssStore();
			if (this.allSelected) {
				store.clearSelection();
			} else {
				store.selectAllItems();
			}
		},
		async handleRefreshAll() {
			await useRssStore().refreshAll();
		},
		async handleBatchDownload() {
			if (this.downloadableSelectedCount === 0) {
				return;
			}
			await useRssStore().batchDownload();
		},
		async handleBatchDelete() {
			if (this.selectedCount === 0) {
				return;
			}
			await useRssStore().batchDelete();
		},
	},
};
</script>

<style scoped>
.rss-empty-inner {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 12px;
  color: var(--mo-no-task-color);
}

.rss-empty-icon {
  opacity: 0.3;
}

.rss-select-all {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  cursor: pointer;
  user-select: none;
}

.rss-select-label {
  font-size: 0.6875rem;
  color: var(--mo-task-action-color);
}
</style>
