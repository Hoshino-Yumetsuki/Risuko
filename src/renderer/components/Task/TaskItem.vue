<template>
  <div
    :key="task._displayKey || task.gid"
    class="task-item"
    :class="{ 'is-active': shouldPulse, 'is-complete': isComplete, selected: selected }"
    v-on:dblclick="onDbClick"
  >
    <div class="task-status-indicator" :class="`status-${taskStatus}`"></div>
    <div class="task-content">
      <div class="task-header">
        <div class="task-name" :title="taskFullName">
          <span>{{ taskFullName }}</span>
          <span
            v-if="task.tag"
            class="task-tag"
            :style="tagStyle"
          >
            {{ task.tag }}
          </span>
        </div>
        <mo-task-item-actions mode="LIST" :task="task" />
      </div>
      <div class="task-progress">
        <mo-task-progress
          :completed="Number(task.completedLength)"
          :total="Number(task.totalLength)"
          :status="taskStatus"
        />
        <mo-task-progress-info :task="task" />
      </div>
      <div v-if="isMultiFileBT" class="task-files">
        <button
          type="button"
          class="task-files-toggle"
          @click.stop="toggleFiles"
        >
          <ChevronDown v-if="filesExpanded" :size="12" />
          <ChevronRight v-else :size="12" />
          <span>{{ $t('task.bt-files-count', { count: displayFiles.length }) }}</span>
        </button>
        <div v-show="filesExpanded" class="task-files-list">
          <div
            v-for="file in displayFiles"
            :key="file.index"
            class="task-file-row"
            :class="{ 'is-complete': file.isComplete }"
          >
            <div class="task-file-head">
              <span class="task-file-name" :title="file.path">{{ file.name }}</span>
              <span class="task-file-pct">{{ file.percent }}%</span>
            </div>
            <mo-task-progress
              :completed="file.completed"
              :total="file.total"
              :status="file.status"
            />
            <div class="task-file-meta">
              <span>{{ formatBytes(file.completed) }} / {{ formatBytes(file.total) }}</span>
            </div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<script lang="ts">
import { ChevronDown, ChevronRight } from "@lucide/vue";
import { TASK_STATUS } from "@shared/constants";
import {
	bytesToSize,
	calcProgress,
	checkTaskIsBT,
	checkTaskIsSeeder,
	getTaskName,
} from "@shared/utils";
import logger from "@shared/utils/logger";
import { useTaskStore } from "@/store/task";
import { getTaskFullPath, openItem } from "@/utils/native";
import TaskItemActions from "./TaskItemActions.vue";
import TaskProgress from "./TaskProgress.vue";
import TaskProgressInfo from "./TaskProgressInfo.vue";

export default {
	name: "mo-task-item",
	components: {
		[TaskItemActions.name]: TaskItemActions,
		[TaskProgress.name]: TaskProgress,
		[TaskProgressInfo.name]: TaskProgressInfo,
		ChevronDown,
		ChevronRight,
	},
	props: {
		task: {
			type: Object,
		},
		selected: {
			type: Boolean,
			default: false,
		},
	},
	data() {
		return {
			filesExpanded: true,
		};
	},
	computed: {
		taskFullName() {
			return getTaskName(this.task, {
				defaultName: this.$t("task.get-task-name"),
				maxLen: -1,
			});
		},
		taskName() {
			return getTaskName(this.task, {
				defaultName: this.$t("task.get-task-name"),
			});
		},
		isSeeder() {
			return checkTaskIsSeeder(this.task);
		},
		isMultiFileBT() {
			const files = Array.isArray(this.task.files) ? this.task.files : [];
			return checkTaskIsBT(this.task) && files.length > 1;
		},
		// Selected files of a multi-file BT torrent, each with its own
		// progress so the grouped card can show per-file status
		displayFiles() {
			const files = Array.isArray(this.task.files) ? this.task.files : [];
			return files
				.filter((f) => f.selected !== "false")
				.map((f) => {
					const total = Number(f.length || 0);
					const completed = Number(f.completedLength || 0);
					const isComplete = total > 0 && completed >= total;
					const segs = `${f.path || ""}`.split(/[/\\]/);
					const name = segs[segs.length - 1] || f.path || "";
					const percent = `${calcProgress(total, completed, 1)}`.replace(
						/\.0$/,
						"",
					);
					return {
						index: f.index,
						path: f.path,
						name,
						total,
						completed,
						percent,
						isComplete,
						status: isComplete ? TASK_STATUS.COMPLETE : this.taskStatus,
					};
				});
		},
		taskStatus() {
			const { task, isSeeder } = this;
			if (isSeeder) {
				return TASK_STATUS.SEEDING;
			} else {
				return task.status;
			}
		},
		isActive() {
			return this.taskStatus === TASK_STATUS.ACTIVE;
		},
		shouldPulse() {
			if (!this.isActive) {
				return false;
			}

			const downloadSpeed = Number(this.task.downloadSpeed || 0);
			const uploadSpeed = Number(this.task.uploadSpeed || 0);
			return downloadSpeed > 0 || uploadSpeed > 0;
		},
		isComplete() {
			return this.taskStatus === TASK_STATUS.COMPLETE;
		},
		tagStyle() {
			const tag = this.task.tag || "";
			let hash = 0;
			for (let i = 0; i < tag.length; i++) {
				hash = tag.charCodeAt(i) + ((hash << 5) - hash);
			}
			const hue = Math.abs(hash % 360);
			return {
				backgroundColor: `hsl(${hue}, 65%, 90%)`,
				color: `hsl(${hue}, 70%, 25%)`,
				border: `1px solid hsl(${hue}, 65%, 80%)`,
			};
		},
	},
	methods: {
		onDbClick() {
			const { status } = this.task;
			const { COMPLETE, WAITING, PAUSED } = TASK_STATUS;
			if (status === COMPLETE) {
				this.openTask();
			} else if ([WAITING, PAUSED].includes(status)) {
				this.toggleTask();
			}
		},
		async openTask() {
			// `openItem` delegates to Rust so each platform can open the file its own way
			// Android uses a SAF document URI plus the file MIME, then shows the normal chooser
			const { taskName } = this;
			this.$msg.info(this.$t("task.opening-task-message", { taskName }));
			const fullPath = getTaskFullPath(this.task);
			try {
				const result = await openItem(fullPath);
				if (result) {
					this.$msg.error(this.$t("task.file-not-exist"));
				}
			} catch (err) {
				logger.warn("[Risuko] open task failed:", err);
				this.$msg.error(`${err}`);
			}
		},
		toggleTask() {
			useTaskStore().toggleTask(this.task);
		},
		toggleFiles() {
			this.filesExpanded = !this.filesExpanded;
		},
		formatBytes(value) {
			return bytesToSize(value);
		},
	},
};
</script>
