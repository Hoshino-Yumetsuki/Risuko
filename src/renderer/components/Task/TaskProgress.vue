<template>
  <ui-progress v-if="isActive" :percentage="percent" status="success" :color="color" />
  <ui-progress v-else :percentage="percent" :color="color" />
</template>

<script lang="ts">
import colors from "@shared/colors";
import { TASK_STATUS } from "@shared/constants";
import { calcProgress } from "@shared/utils";

export default {
	name: "mo-task-progress",
	props: {
		total: {
			type: Number,
		},
		completed: {
			type: Number,
		},
		status: {
			type: String,
			default: TASK_STATUS.ACTIVE,
		},
	},
	computed: {
		isActive() {
			return this.status === TASK_STATUS.ACTIVE;
		},
		percent() {
			return calcProgress(this.total, this.completed);
		},
		color() {
			return colors[this.status];
		},
	},
};
</script>
