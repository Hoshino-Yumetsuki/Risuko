<template>
  <nav class="subnav-inner">
    <mo-enter tag="h3" preset="fadeInDown">{{ title }}</mo-enter>
    <LayoutGroup id="task-subnav">
      <ul>
        <mo-enter
          v-for="item in items"
          :key="item.key"
          tag="li"
          preset="fadeInLeft"
          :delay="item.delay"
          @click="() => nav(item.key)"
          :class="[current === item.key ? 'active' : '']"
        >
          <Motion
            v-if="current === item.key"
            layout-id="task-subnav-pill"
            class="subnav-active-bg"
            :initial="false"
            :transition="pillTransition"
          />
          <i class="subnav-icon">
            <component :is="item.icon" :size="20" />
          </i>
          <span>{{ $t(item.label) }}</span>
          <span v-if="taskCounts[item.key] > 0" class="subnav-badge">{{ taskCounts[item.key] }}</span>
        </mo-enter>
      </ul>
    </LayoutGroup>
  </nav>
</template>

<script lang="ts">
import logger from "@shared/utils/logger";
import { CircleCheck, LayoutList, Pause, Play, Square } from "lucide-vue-next";
import { LayoutGroup, Motion } from "motion-v";
import { useTaskStore } from "@/store/task";

export default {
	name: "mo-task-subnav",
	components: {
		CircleCheck,
		LayoutGroup,
		LayoutList,
		Motion,
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
		items() {
			return [
				{ key: "all", icon: LayoutList, label: "task.all", delay: 0.09 },
				{ key: "active", icon: Play, label: "task.active", delay: 0.13 },
				{ key: "waiting", icon: Pause, label: "task.waiting", delay: 0.17 },
				{
					key: "completed",
					icon: CircleCheck,
					label: "task.completed",
					delay: 0.21,
				},
				{ key: "stopped", icon: Square, label: "task.stopped", delay: 0.25 },
			];
		},
		pillTransition() {
			return { type: "spring", stiffness: 380, damping: 32 };
		},
	},
	methods: {
		nav(status: string) {
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
