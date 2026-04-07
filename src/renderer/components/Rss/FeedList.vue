<template>
  <nav class="subnav-inner">
    <h3>{{ $t('subnav.rss') }}</h3>
    <ul>
      <li
        :class="{ active: currentFeedId === null }"
        style="--stagger-index: 0"
        @click="selectFeed(null)"
      >
        <i class="subnav-icon">
          <Rss :size="20" />
        </i>
        <span>{{ $t('rss.all-items') }}</span>
        <span v-if="totalUnread > 0" class="rss-feed-badge">{{ totalUnread }}</span>
      </li>
      <li
        :class="{ active: currentFeedId === '__downloaded__' }"
        style="--stagger-index: 1"
        @click="selectFeed('__downloaded__')"
      >
        <i class="subnav-icon">
          <Download :size="20" />
        </i>
        <span>{{ $t('rss.downloaded') }}</span>
        <span v-if="downloadedCount > 0" class="rss-feed-badge">{{ downloadedCount }}</span>
      </li>
      <li
        v-for="(feed, index) in feeds"
        :key="feed.id"
        :class="{
          active: currentFeedId === feed.id,
          'rss-feed-entry--error': feed.error_count >= 3,
          'rss-feed-entry--inactive': !feed.is_active,
        }"
        :style="{ '--stagger-index': index + 2 }"
        @click="selectFeed(feed.id)"
        @contextmenu.prevent="openContextMenu($event, feed)"
      >
        <i class="subnav-icon">
          <CircleDot v-if="!feed.is_active" :size="20" />
          <AlertTriangle v-else-if="feed.error_count >= 3" :size="20" />
          <Rss v-else :size="20" />
        </i>
        <span class="rss-feed-name" :title="feed.title">{{ feed.title }}</span>
        <span v-if="unreadCount(feed.id) > 0" class="rss-feed-badge">
          {{ unreadCount(feed.id) }}
        </span>
      </li>
    </ul>
    <div class="rss-subnav-footer">
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
import type { RssFeed } from "@shared/types/rss";
import {
	AlertTriangle,
	CircleDot,
	Download,
	Plus,
	RefreshCw,
	Rss,
	Settings2,
	Trash2,
} from "lucide-vue-next";
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
	name: "mo-rss-feed-list",
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
.rss-feed-entry--inactive {
  opacity: 0.5;
}

.rss-feed-entry--error .subnav-icon svg {
  color: hsl(var(--destructive));
}

.rss-feed-name {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.rss-feed-badge {
  font-size: 10px;
  font-weight: 600;
  background: var(--color-primary);
  color: #fff;
  border-radius: 9999px;
  padding: 0 5px;
  min-width: 16px;
  height: 16px;
  display: flex;
  align-items: center;
  justify-content: center;
  line-height: 1;
  margin-left: auto;
}

.rss-subnav-footer {
  padding: 8px 12px;
  border-top: 1px solid var(--mo-subnav-border-color);
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
