<template>
  <div class="flyout-root">
    <header class="flyout-header" data-tauri-drag-region>
      <div class="flyout-title">
        <span class="flyout-title-text">{{ activeTabLabel }}</span>
        <span v-if="activeTabCount > 0" class="flyout-title-count">{{ activeTabCount }}</span>
      </div>
      <div class="flyout-summary" aria-label="Total transfer speed">
        <span class="flyout-summary-speed" :title="$t('task.task-download-speed')">
          <ArrowDown :size="13" aria-hidden="true" />
          {{ formatBytes(stat.downloadSpeed) }}/s
        </span>
        <span class="flyout-summary-speed" :title="$t('task.task-upload-speed')">
          <ArrowUp :size="13" aria-hidden="true" />
          {{ formatBytes(stat.uploadSpeed) }}/s
        </span>
      </div>
    </header>

    <nav
      class="flyout-tabs"
      role="tablist"
      :aria-label="$t('app.task-list')"
      @keydown="onTabKeydown"
    >
      <button
        v-for="tab in tabs"
        :id="`flyout-tab-${tab.key}`"
        :key="tab.key"
        ref="tabButtons"
        type="button"
        role="tab"
        class="flyout-tab"
        :class="{ active: currentList === tab.key }"
        :aria-selected="currentList === tab.key"
        :aria-label="`${tab.label}${countFor(tab.key) > 0 ? `, ${countFor(tab.key)}` : ''}`"
        :title="tab.label"
        :tabindex="currentList === tab.key ? 0 : -1"
        @click="selectTab(tab.key)"
      >
        <component :is="tab.icon" :size="16" aria-hidden="true" />
        <span v-if="countFor(tab.key) > 0" class="flyout-tab-badge">{{ countFor(tab.key) }}</span>
      </button>
    </nav>

    <main
      class="flyout-list"
      role="list"
      :aria-label="activeTabLabel"
      tabindex="0"
    >
      <template v-if="taskList.length > 0">
        <mo-flyout-task-item
          v-for="task in taskList"
          :key="task.gid"
          role="listitem"
          :task="task"
        />
      </template>
      <div v-else class="flyout-empty">
        <Inbox :size="34" aria-hidden="true" />
        <span>{{ $t('app.flyout-empty') }}</span>
      </div>
    </main>

    <footer class="flyout-footer" role="toolbar" :aria-label="$t('menu.task')">
      <button type="button" class="flyout-action" :aria-label="$t('task.pause-all-task')" :title="$t('task.pause-all-task')" @click="pauseAll">
        <Pause :size="17" aria-hidden="true" />
      </button>
      <button type="button" class="flyout-action" :aria-label="$t('task.resume-all-task')" :title="$t('task.resume-all-task')" @click="resumeAll">
        <Play :size="17" aria-hidden="true" />
      </button>
      <button type="button" class="flyout-action" :aria-label="$t('task.new-task')" :title="$t('task.new-task')" @click="newTask">
        <Plus :size="17" aria-hidden="true" />
      </button>
      <div class="flyout-footer-spacer"></div>
      <button type="button" class="flyout-action" :aria-label="$t('app.show')" :title="$t('app.show')" @click="openMain">
        <ExternalLink :size="17" aria-hidden="true" />
      </button>
      <button type="button" class="flyout-action" :aria-label="$t('app.preferences')" :title="$t('app.preferences')" @click="openPreferences">
        <Settings :size="17" aria-hidden="true" />
      </button>
      <button type="button" class="flyout-action flyout-action-danger" :aria-label="$t('app.quit')" :title="$t('app.quit')" @click="quit">
        <Power :size="17" aria-hidden="true" />
      </button>
    </footer>
  </div>
</template>

<script lang="ts">
import {
	ArrowDown,
	ArrowUp,
	CircleCheck,
	ExternalLink,
	Inbox,
	LayoutList,
	Pause,
	Play,
	Plus,
	Power,
	Settings,
	Square,
} from "@lucide/vue";
import { bytesToSize } from "@shared/utils";
import logger from "@shared/utils/logger";
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { markRaw } from "vue";
import { useAppStore } from "@/store/app";
import { useTaskStore } from "@/store/task";
import FlyoutTaskItem from "./FlyoutTaskItem.vue";

export default {
	name: "mo-flyout",
	components: {
		[FlyoutTaskItem.name as string]: FlyoutTaskItem,
		ArrowDown,
		ArrowUp,
		ExternalLink,
		Inbox,
		Pause,
		Play,
		Plus,
		Power,
		Settings,
	},
	computed: {
		stat() {
			return useAppStore().stat;
		},
		currentList(): string {
			return useTaskStore().currentList;
		},
		taskList() {
			return useTaskStore().taskList;
		},
		tabs() {
			return [
				{ key: "all", icon: markRaw(LayoutList), label: this.$t("task.all") },
				{ key: "active", icon: markRaw(Play), label: this.$t("task.active") },
				{
					key: "waiting",
					icon: markRaw(Pause),
					label: this.$t("task.waiting"),
				},
				{
					key: "completed",
					icon: markRaw(CircleCheck),
					label: this.$t("task.completed"),
				},
				{
					key: "stopped",
					icon: markRaw(Square),
					label: this.$t("task.stopped"),
				},
			];
		},
		activeTabLabel(): string {
			return (
				this.tabs.find((tab) => tab.key === this.currentList)?.label ||
				this.$t("task.all")
			);
		},
		activeTabCount(): number {
			return this.countFor(this.currentList);
		},
	},
	methods: {
		formatBytes(value: number): string {
			return bytesToSize(value);
		},
		countFor(key: string): number {
			return useTaskStore().taskCountMap[key] || 0;
		},
		selectTab(key: string): void {
			useTaskStore().changeCurrentList(key);
		},
		onTabKeydown(event: KeyboardEvent): void {
			const keys = this.tabs.map((tab) => tab.key);
			const current = keys.indexOf(this.currentList);
			let next = -1;
			if (event.key === "ArrowRight" || event.key === "ArrowDown") {
				next = (current + 1) % keys.length;
			} else if (event.key === "ArrowLeft" || event.key === "ArrowUp") {
				next = (current - 1 + keys.length) % keys.length;
			} else if (event.key === "Home") {
				next = 0;
			} else if (event.key === "End") {
				next = keys.length - 1;
			}
			if (next < 0) {
				return;
			}
			event.preventDefault();
			this.selectTab(keys[next]);
			this.$nextTick(() => {
				const buttons = this.$refs.tabButtons as
					| HTMLButtonElement[]
					| undefined;
				buttons?.[next]?.focus();
			});
		},
		pauseAll(): void {
			useTaskStore().pauseAllTask();
		},
		resumeAll(): void {
			useTaskStore().resumeAllTask();
		},
		async hideFlyout(): Promise<void> {
			try {
				await getCurrentWebviewWindow().hide();
			} catch (err) {
				logger.warn("[Risuko] flyout hide failed:", err);
			}
		},
		async showMain(): Promise<void> {
			try {
				await invoke("show_window");
			} catch (err) {
				logger.warn("[Risuko] show_window failed:", err);
			}
		},
		async newTask(): Promise<void> {
			await this.showMain();
			await emit("command", { command: "application:new-task" });
			await this.hideFlyout();
		},
		async openMain(): Promise<void> {
			await this.showMain();
			await this.hideFlyout();
		},
		async openPreferences(): Promise<void> {
			await this.showMain();
			await emit("command", { command: "application:preferences" });
			await this.hideFlyout();
		},
		async quit(): Promise<void> {
			await emit("confirm-quit");
			await this.hideFlyout();
		},
	},
};
</script>
