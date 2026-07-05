<template>
  <Dialog :open="visible" @update:open="onOpenChange">
    <DialogContent class="w-96 gap-0 p-0">
      <DialogHeader class="flex flex-row items-center gap-2 border-b border-border/60 px-4 py-3">
        <div class="flex size-7 items-center justify-center rounded-md bg-amber-500/15 text-amber-600 dark:text-amber-400">
          <Clock :size="14" />
        </div>
        <DialogTitle class="flex-1 text-sm font-medium">
          {{ $t('task.schedule-dialog-title') }}
        </DialogTitle>
      </DialogHeader>

      <div class="flex flex-col gap-3 px-4 py-4">
        <p class="text-xs leading-relaxed text-muted-foreground">
          {{ $t('task.schedule-dialog-hint') }}
        </p>
        <p v-if="taskName" class="truncate text-xs font-medium">{{ taskName }}</p>
        <DateTimePicker v-model="startAt" :placeholder="$t('task.schedule-pick-time')" />
      </div>

      <DialogFooter class="flex justify-end gap-2 border-t border-border/60 px-4 py-3">
        <Button variant="ghost" size="sm" @click="close">
          {{ $t('app.cancel') }}
        </Button>
        <Button size="sm" :disabled="!startAt" @click="confirm">
          {{ $t('task.schedule-confirm') }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>

<script lang="ts">
import { Clock } from "@lucide/vue";
import type { DownloadTask } from "@shared/types/task";
import { getTaskName } from "@shared/utils";
import type { PropType } from "vue";
import { Button } from "@/components/ui/button";
import { DateTimePicker } from "@/components/ui/date-time-picker";
import {
	Dialog,
	DialogContent,
	DialogFooter,
	DialogHeader,
	DialogTitle,
} from "@/components/ui/dialog";
import { useTaskStore } from "@/store/task";

export default {
	name: "schedule-dialog",
	components: {
		Clock,
		Button,
		DateTimePicker,
		Dialog,
		DialogContent,
		DialogFooter,
		DialogHeader,
		DialogTitle,
	},
	props: {
		visible: { type: Boolean, default: false },
		task: { type: Object as PropType<DownloadTask | null>, default: null },
	},
	emits: ["update:visible"],
	data() {
		return { startAt: null as number | null };
	},
	computed: {
		taskName(): string {
			return this.task ? getTaskName(this.task) : "";
		},
	},
	watch: {
		visible(v: boolean) {
			if (v) {
				const existing = Number(this.task?.startAt);
				this.startAt =
					Number.isFinite(existing) && existing > 0
						? existing
						: this.nextTwoAm();
			}
		},
	},
	methods: {
		nextTwoAm(): number {
			const d = new Date();
			d.setHours(2, 0, 0, 0);
			if (d.getTime() <= Date.now()) {
				d.setDate(d.getDate() + 1);
			}
			return Math.floor(d.getTime() / 1000);
		},
		onOpenChange(open: boolean) {
			if (!open) {
				this.close();
			}
		},
		close() {
			this.$emit("update:visible", false);
		},
		async confirm() {
			if (!this.task || !this.startAt) {
				return;
			}
			try {
				await useTaskStore().setSchedule(this.task.gid, this.startAt);
				this.close();
			} catch (err) {
				this.$msg?.error?.(
					(err as Error)?.message || this.$t("task.schedule-fail"),
				);
			}
		},
	},
};
</script>
