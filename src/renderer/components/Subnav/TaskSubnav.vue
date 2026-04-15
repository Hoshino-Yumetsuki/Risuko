<template>
  <nav class="subnav-inner">
    <h3>{{ title }}</h3>
    <ul>
      <li
        @click="() => nav('all')"
        :class="[current === 'all' ? 'active' : '']"
        style="--stagger-index: 0"
      >
        <i class="subnav-icon">
          <LayoutList :size="20" />
        </i>
        <span>{{ $t('task.all') }}</span>
        <span v-if="taskCounts.all > 0" class="subnav-badge">{{ taskCounts.all }}</span>
      </li>
      <li
        @click="() => nav('active')"
        :class="[current === 'active' ? 'active' : '']"
        style="--stagger-index: 1"
      >
        <i class="subnav-icon">
          <Play :size="20" />
        </i>
        <span>{{ $t('task.active') }}</span>
        <span v-if="taskCounts.active > 0" class="subnav-badge">{{ taskCounts.active }}</span>
      </li>
      <li
        @click="() => nav('waiting')"
        :class="[current === 'waiting' ? 'active' : '']"
        style="--stagger-index: 2"
      >
        <i class="subnav-icon">
          <Pause :size="20" />
        </i>
        <span>{{ $t('task.waiting') }}</span>
        <span v-if="taskCounts.waiting > 0" class="subnav-badge">{{ taskCounts.waiting }}</span>
      </li>
      <li
        @click="() => nav('completed')"
        :class="[current === 'completed' ? 'active' : '']"
        style="--stagger-index: 3"
      >
        <i class="subnav-icon">
          <CircleCheck :size="20" />
        </i>
        <span>{{ $t('task.completed') }}</span>
        <span v-if="taskCounts.completed > 0" class="subnav-badge">{{ taskCounts.completed }}</span>
      </li>
      <li
        @click="() => nav('stopped')"
        :class="[current === 'stopped' ? 'active' : '']"
        style="--stagger-index: 4"
      >
        <i class="subnav-icon">
          <Square :size="20" />
        </i>
        <span>{{ $t('task.stopped') }}</span>
        <span v-if="taskCounts.stopped > 0" class="subnav-badge">{{ taskCounts.stopped }}</span>
      </li>
    </ul>
  </nav>
</template>

<script lang="ts">
import logger from "@shared/utils/logger";
import { CircleCheck, LayoutList, Pause, Play, Square } from "lucide-vue-next";
import { useTaskStore } from "@/store/task";

export default {
	name: "mo-task-subnav",
	components: {
		CircleCheck,
		LayoutList,
		Pause,
		Play,
		Square,
	},
	props: {
		current: {
			type: String,
			default: "all",
		},
	},
	computed: {
		title() {
			return this.$t("subnav.task-list");
		},
		taskCounts() {
			return useTaskStore().taskCountMap;
		},
	},
	methods: {
		nav(status = "active") {
			this.$router
				.push({
					path: `/task/${status}`,
				})
				.catch((err) => {
					logger.log(err);
				});
		},
	},
};
</script>
