<template>
  <aside class="aside hidden-sm-and-down" :class="{ draggable: asideDraggable }" :style="vibrancy">
    <div class="aside-inner">
      <div class="aside-brand">
        <mo-logo-mini />
        <div class="aside-version" v-if="appVersion">
          {{ appVersion }}
        </div>
      </div>
      <ul class="menu top-menu">
        <li @click="nav('/task')" class="non-draggable" style="animation-delay: 0s">
          <ListTodo :size="20" />
        </li>
        <li @click="showAddTask()" class="non-draggable" style="animation-delay: 0.06s">
          <Plus :size="20" />
        </li>
        <li @click="nav('/rss')" class="non-draggable" style="animation-delay: 0.12s">
          <Rss :size="20" />
        </li>
      </ul>
      <ul class="menu bottom-menu">
        <li @click="nav('/preference')" class="non-draggable" style="animation-delay: 0s">
          <Settings2 :size="20" />
        </li>
        <li @click="showAboutPanel" class="non-draggable" style="animation-delay: 0.06s">
          <Info :size="20" />
        </li>
      </ul>
    </div>
  </aside>
</template>

<script lang="ts">
import { ADD_TASK_TYPE } from "@shared/constants";
import logger from "@shared/utils/logger";
import { Info, ListTodo, Plus, Rss, Settings2 } from "lucide-vue-next";
import LogoMini from "@/components/Logo/LogoMini.vue";
import is from "@/shims/platform";
import { useAppStore } from "@/store/app";
import { getRisukoVersion } from "@/utils/version";

export default {
	name: "mo-aside",
	components: {
		[LogoMini.name]: LogoMini,
		Info,
		ListTodo,
		Plus,
		Rss,
		Settings2,
	},
	data() {
		return {
			appVersion: "",
		};
	},
	async created() {
		this.appVersion = await getRisukoVersion();
	},
	computed: {
		asideDraggable() {
			return !is.macOS();
		},
		vibrancy() {
			return is.macOS()
				? {
						backdropFilter: "saturate(120%) blur(10px)",
					}
				: {};
		},
	},
	methods: {
		showAddTask(taskType = ADD_TASK_TYPE.URI) {
			useAppStore().showAddTaskDialog(taskType);
		},
		showAboutPanel() {
			useAppStore().showAboutPanel();
		},
		nav(page) {
			this.$router
				.push({
					path: page,
				})
				.catch((err) => {
					logger.log(err);
				});
		},
	},
};
</script>
