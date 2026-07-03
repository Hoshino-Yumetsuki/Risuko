<template>
  <nav class="rss-feed-list">
    <ul class="rss-feed-items">
      <li
        class="rss-feed-item"
        :class="{ active: currentFeedId === null }"
        @click="selectFeed(null)"
      >
        <i class="rss-feed-icon">
          <Rss :size="15" />
        </i>
        <span class="rss-feed-name">{{ $t('rss.all-items') }}</span>
        <span v-if="totalUnread > 0" class="rss-feed-badge">{{ totalUnread }}</span>
      </li>
      <li
        class="rss-feed-item"
        :class="{ active: currentFeedId === '__downloaded__' }"
        @click="selectFeed('__downloaded__')"
      >
        <i class="rss-feed-icon">
          <Download :size="15" />
        </i>
        <span class="rss-feed-name">{{ $t('rss.downloaded') }}</span>
        <span v-if="downloadedCount > 0" class="rss-feed-badge">{{ downloadedCount }}</span>
      </li>
      <li
        v-for="feed in feeds"
        :key="feed.id"
        class="rss-feed-item"
        :class="{
          active: currentFeedId === feed.id,
          'rss-feed-item--error': feed.error_count >= 3,
          'rss-feed-item--inactive': !feed.is_active,
        }"
        @click="selectFeed(feed.id)"
        @contextmenu.prevent="openContextMenu($event, feed)"
      >
        <i class="rss-feed-icon">
          <CircleDot v-if="!feed.is_active" :size="15" />
          <AlertTriangle v-else-if="feed.error_count >= 3" :size="15" />
          <Rss v-else :size="15" />
        </i>
        <span class="rss-feed-name" :title="feed.title">{{ feed.title }}</span>
        <span v-if="unreadCount(feed.id) > 0" class="rss-feed-badge">
          {{ unreadCount(feed.id) }}
        </span>
      </li>
    </ul>
    <div class="rss-feed-footer">
      <Button size="sm" variant="ghost" class="rss-add-btn" @click="$emit('add-feed')">
        <Plus :size="14" />
        {{ $t('rss.add-feed') }}
      </Button>
    </div>

    <DropdownMenu :open="contextMenuOpen" @update:open="contextMenuOpen = $event">
      <DropdownMenuTrigger as-child>
        <span ref="contextAnchor" class="rss-context-anchor" :style="contextAnchorStyle" />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start">
        <DropdownMenuItem @click="handleRefresh">
          <RefreshCw :size="14" />
          {{ $t('rss.refresh') }}
        </DropdownMenuItem>
        <DropdownMenuItem @click="handleEditSettings">
          <Settings2 :size="14" />
          {{ $t('rss.feed-settings') }}
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem class="text-destructive" @click="handleRemove">
          <Trash2 :size="14" />
          {{ $t('rss.remove-feed') }}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  </nav>
</template>

<script lang="ts">
import {
	AlertTriangle,
	CircleDot,
	Download,
	Plus,
	RefreshCw,
	Rss,
	Settings2,
	Trash2,
} from "@lucide/vue";
import type { RssFeed } from "@shared/types/rss";
import { Button } from "@/components/ui/button";
import {
	DropdownMenu,
	DropdownMenuContent,
	DropdownMenuItem,
	DropdownMenuSeparator,
	DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useRssStore } from "@/store/rss";

export default {
	name: "rss-feed-list",
	components: {
		AlertTriangle,
		Button,
		CircleDot,
		Download,
		DropdownMenu,
		DropdownMenuContent,
		DropdownMenuItem,
		DropdownMenuSeparator,
		DropdownMenuTrigger,
		Plus,
		RefreshCw,
		Rss,
		Settings2,
		Trash2,
	},
	emits: ["add-feed"],
	data() {
		return {
			contextMenuOpen: false,
			contextFeed: null as RssFeed | null,
			contextAnchorStyle: {
				position: "fixed" as const,
				left: "0px",
				top: "0px",
			},
		};
	},
	computed: {
		feeds() {
			return useRssStore().feeds;
		},
		currentFeedId() {
			return useRssStore().currentFeedId;
		},
		totalUnread(): number {
			const store = useRssStore();
			return store.feeds.reduce((sum, f) => sum + store.unreadCount(f.id), 0);
		},
		downloadedCount(): number {
			const store = useRssStore();
			let count = 0;
			for (const items of Object.values(store.items)) {
				count += items.filter((i) => i.is_downloaded).length;
			}
			return count;
		},
	},
	methods: {
		selectFeed(feedId: string | null) {
			useRssStore().selectFeed(feedId);
		},
		unreadCount(feedId: string): number {
			return useRssStore().unreadCount(feedId);
		},
		openContextMenu(event: MouseEvent, feed: RssFeed) {
			this.contextFeed = feed;
			this.contextAnchorStyle = {
				position: "fixed",
				left: `${event.clientX}px`,
				top: `${event.clientY}px`,
			};
			this.contextMenuOpen = true;
		},
		async handleRefresh() {
			if (!this.contextFeed) {
				return;
			}
			await useRssStore().refreshFeed(this.contextFeed.id);
		},
		handleEditSettings() {
			if (!this.contextFeed) {
				return;
			}
			this.$parent!.$data.editingFeedId = this.contextFeed.id;
			this.$parent!.$data.showFeedSettingsDialog = true;
		},
		async handleRemove() {
			if (!this.contextFeed) {
				return;
			}
			await useRssStore().removeFeed(this.contextFeed.id);
		},
	},
};
</script>

<style scoped>
.rss-feed-list {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  user-select: none;
}

.rss-feed-items {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  list-style: none;
  margin: 0;
  padding: 14px 12px 8px;
  display: flex;
  flex-direction: column;
  gap: 1px;
}

.rss-feed-item {
  display: flex;
  align-items: center;
  gap: 8px;
  height: 30px;
  padding: 0 8px;
  border-radius: 6px;
  color: var(--text-2);
  font-size: 13px;
  cursor: pointer;
  transition:
    color var(--dur-1) var(--ease-out),
    background-color var(--dur-1) var(--ease-out);
}

.rss-feed-item:hover {
  color: var(--text-1);
  background: var(--surface-1);
}

.rss-feed-item.active {
  color: var(--text-1);
  background: var(--primary-soft);
}

.rss-feed-item.active .rss-feed-icon {
  color: var(--primary);
}

.rss-feed-item--inactive {
  opacity: 0.5;
}

.rss-feed-item--error .rss-feed-icon {
  color: var(--danger);
}

.rss-feed-icon {
  display: inline-flex;
  flex-shrink: 0;
  color: var(--text-3);
  line-height: 0;
  font-style: normal;
  transition: color var(--dur-1) var(--ease-out);
}

.rss-feed-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.rss-feed-badge {
  flex-shrink: 0;
  font-size: 11px;
  font-weight: 500;
  color: var(--text-3);
  font-variant-numeric: tabular-nums;
}

.rss-feed-footer {
  flex-shrink: 0;
}

.rss-add-btn {
  width: 100%;
  justify-content: center;
  gap: 4px;
  font-size: 12px;
}

.rss-context-anchor {
  width: 0;
  height: 0;
  pointer-events: none;
}
</style>
