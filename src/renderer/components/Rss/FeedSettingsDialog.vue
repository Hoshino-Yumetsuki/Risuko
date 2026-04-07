<template>
  <Dialog :open="visible" @update:open="handleOpenChange">
    <DialogContent class="rss-feed-settings-dialog" :show-close-button="true">
      <DialogHeader>
        <DialogTitle>{{ $t('rss.feed-settings') }}</DialogTitle>
      </DialogHeader>
      <div v-if="feed" class="rss-feed-settings-body">
        <div class="rss-setting-row">
          <label class="rss-setting-label">{{ $t('rss.feed-url') }}</label>
          <span class="rss-setting-value rss-setting-url" :title="feed.url">{{ feed.url }}</span>
        </div>
        <div class="rss-setting-row">
          <label class="rss-setting-label">{{ $t('rss.last-fetched') }}</label>
          <span class="rss-setting-value">
            {{ feed.last_fetched_at ? formatDate(feed.last_fetched_at) : $t('rss.never') }}
          </span>
        </div>
        <div class="rss-setting-row">
          <label class="rss-setting-label">{{ $t('rss.update-interval') }}</label>
          <select v-model="interval" class="rss-setting-select">
            <option :value="300">5 min</option>
            <option :value="600">10 min</option>
            <option :value="1800">30 min</option>
            <option :value="3600">1 hour</option>
            <option :value="7200">2 hours</option>
            <option :value="21600">6 hours</option>
            <option :value="86400">24 hours</option>
          </select>
        </div>
        <div class="rss-setting-row">
          <label class="rss-setting-label">{{ $t('rss.active') }}</label>
          <Switch :model-value="isActive" @update:model-value="isActive = $event" />
        </div>
      </div>
      <DialogFooter>
        <Button variant="outline" @click="handleClose">
          {{ $t('rss.cancel') }}
        </Button>
        <Button @click="handleSave">
          {{ $t('rss.save') }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>

<script lang="ts">
import type { RssFeed } from "@shared/types/rss";
import { Button } from "@/components/ui/button";
import {
	Dialog,
	DialogContent,
	DialogFooter,
	DialogHeader,
	DialogTitle,
} from "@/components/ui/dialog";
import { Switch } from "@/components/ui/switch";
import { useRssStore } from "@/store/rss";

export default {
	name: "mo-rss-feed-settings-dialog",
	components: {
		Button,
		Dialog,
		DialogContent,
		DialogFooter,
		DialogHeader,
		DialogTitle,
		Switch,
	},
	props: {
		visible: { type: Boolean, default: false },
		feedId: { type: String, default: "" },
	},
	emits: ["close"],
	data() {
		return {
			interval: 1800,
			isActive: true,
		};
	},
	computed: {
		feed(): RssFeed | undefined {
			return useRssStore().feeds.find((f) => f.id === this.feedId);
		},
	},
	watch: {
		visible(val) {
			if (val && this.feed) {
				this.interval = this.feed.update_interval_secs;
				this.isActive = this.feed.is_active;
			}
		},
	},
	methods: {
		async handleSave() {
			if (!this.feedId) {
				return;
			}
			await useRssStore().updateFeedSettings(
				this.feedId,
				this.interval,
				this.isActive,
			);
			this.$emit("close");
		},
		handleOpenChange(open: boolean) {
			if (!open) {
				this.handleClose();
			}
		},
		handleClose() {
			this.$emit("close");
		},
		formatDate(timestamp: number): string {
			return new Date(timestamp * 1000).toLocaleString();
		},
	},
};
</script>

<style scoped>
.rss-feed-settings-body {
  display: flex;
  flex-direction: column;
  gap: 12px;
  padding: 8px 0;
}

.rss-setting-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}

.rss-setting-label {
  font-size: 13px;
  font-weight: 500;
  flex-shrink: 0;
}

.rss-setting-value {
  font-size: 13px;
  color: hsl(var(--muted-foreground));
  text-align: right;
}

.rss-setting-url {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 280px;
}

.rss-setting-select {
  height: 32px;
  padding: 0 8px;
  font-size: 13px;
  border: 1px solid hsl(var(--border));
  border-radius: var(--radius);
  background: transparent;
  color: inherit;
}
</style>
