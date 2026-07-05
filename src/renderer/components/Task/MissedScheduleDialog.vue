<template>
  <Dialog :open="visible" @update:open="onOpenChange">
    <DialogContent class="w-[26rem] gap-0 p-0">
      <DialogHeader class="flex flex-row items-center gap-2 border-b border-border/60 px-4 py-3">
        <div class="flex size-7 items-center justify-center rounded-md bg-amber-500/15 text-amber-600 dark:text-amber-400">
          <CalendarClock :size="14" />
        </div>
        <DialogTitle class="flex-1 text-sm font-medium">
          {{ $t('task.missed-schedule-title') }}
        </DialogTitle>
      </DialogHeader>

      <div class="flex max-h-[50vh] flex-col gap-2 overflow-y-auto px-4 py-3">
        <p class="text-xs leading-relaxed text-muted-foreground">
          {{ $t('task.missed-schedule-hint') }}
        </p>
        <div
          v-for="task in tasks"
          :key="task.gid"
          class="flex items-center gap-2 rounded-md border border-border/60 bg-muted/30 px-2.5 py-2"
        >
          <div class="min-w-0 flex-1">
            <p class="truncate text-xs font-medium">{{ taskName(task) }}</p>
            <p class="text-[11px] text-muted-foreground">{{ formatTime(task.startAt) }}</p>
          </div>
          <Button size="sm" variant="outline" @click="startNow(task)">
            {{ $t('task.start-now') }}
          </Button>
        </div>
      </div>

      <DialogFooter class="flex justify-end gap-2 border-t border-border/60 px-4 py-3">
        <Button variant="ghost" size="sm" @click="close">
          {{ $t('task.missed-schedule-dismiss') }}
        </Button>
        <Button size="sm" :disabled="tasks.length === 0" @click="startAll">
          {{ $t('task.missed-schedule-start-all') }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>

<script lang="ts">
import { CalendarClock } from "@lucide/vue";
import type { DownloadTask } from "@shared/types/task";
import { getTaskName, localeDateTimeFormat } from "@shared/utils";
import { Button } from "@/components/ui/button";
import {
	Dialog,
	DialogContent,
	DialogFooter,
	DialogHeader,
	DialogTitle,
} from "@/components/ui/dialog";
import { useAppStore } from "@/store/app";
import { usePreferenceStore } from "@/store/preference";
import { useTaskStore } from "@/store/task";

export default {
	name: "missed-schedule-dialog",
	components: {
		CalendarClock,
		Button,
		Dialog,
		DialogContent,
		DialogFooter,
		DialogHeader,
		DialogTitle,
	},
	computed: {
		visible(): boolean {
			return useAppStore().missedScheduledVisible;
		},
		tasks(): DownloadTask[] {
			return useAppStore().missedScheduledTasks;
		},
	},
	methods: {
		taskName(task: DownloadTask): string {
			return getTaskName(task);
		},
		formatTime(startAt?: string): string {
			const secs = Number(startAt);
			if (!Number.isFinite(secs) || secs <= 0) {
				return "";
			}
			return localeDateTimeFormat(
				secs,
				usePreferenceStore().config?.locale || "en-US",
			);
		},
		onOpenChange(open: boolean) {
			if (!open) {
				this.close();
			}
		},
		close() {
			useAppStore().hideMissedScheduled();
		},
		async startNow(task: DownloadTask) {
			try {
				await useTaskStore().startNow(task.gid);
			} catch (err) {
				this.$msg?.error?.(
					(err as Error)?.message ||
						this.$t("task.start-now-fail", { taskName: this.taskName(task) }),
				);
				return;
			}
			const remaining = useAppStore().missedScheduledTasks.filter(
				(t) => t.gid !== task.gid,
			);
			if (remaining.length === 0) {
				this.close();
			} else {
				useAppStore().showMissedScheduled(remaining);
			}
		},
		async startAll() {
			const pending = [...useAppStore().missedScheduledTasks];
			const results = await Promise.allSettled(
				pending.map((t) => useTaskStore().startNow(t.gid)),
			);
			const failed = pending.filter((_, i) => results[i].status === "rejected");
			if (failed.length > 0) {
				useAppStore().showMissedScheduled(failed);
				const firstErr = results.find((r) => r.status === "rejected") as
					| PromiseRejectedResult
					| undefined;
				this.$msg?.error?.(
					(firstErr?.reason as Error)?.message ||
						this.$t("task.missed-schedule-start-all-fail", {
							count: failed.length,
						}),
				);
				return;
			}
			this.close();
		},
	},
};
</script>
