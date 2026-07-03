<template>
  <header class="task-detail-header">
    <div class="task-detail-header-info">
      <h4 class="task-detail-header-name" :title="taskName">
        {{ taskName || $t('task.task-detail-title') }}
      </h4>
      <task-status v-if="task" :status="taskStatus" />
    </div>
    <button
      class="task-detail-close"
      type="button"
      :aria-label="$t('window.close')"
      @click="$emit('close')"
    >
      <X :size="16" />
    </button>
  </header>
</template>

<script lang="ts">
import { X } from "@lucide/vue";
import { TASK_STATUS } from "@shared/constants";
import { checkTaskIsSeeder, getTaskName } from "@shared/utils";
import TaskStatus from "@/components/Task/TaskStatus.vue";

export default {
	name: "task-detail-header",
	components: {
		[TaskStatus.name]: TaskStatus,
		X,
	},
	props: {
		task: {
			type: Object,
		},
	},
	emits: ["close"],
	computed: {
		taskName() {
			if (!this.task) {
				return "";
			}
			return getTaskName(this.task, {
				defaultName: this.$t("task.get-task-name"),
				maxLen: -1,
			});
		},
		taskStatus() {
			if (!this.task) {
				return TASK_STATUS.WAITING;
			}
			return checkTaskIsSeeder(this.task)
				? TASK_STATUS.SEEDING
				: this.task.status;
		},
	},
};
</script>
