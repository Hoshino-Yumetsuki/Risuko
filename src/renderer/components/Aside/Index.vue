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
        <mo-enter tag="li" preset="fadeInLeft" :delay="0.15" @click="nav('/task')" class="non-draggable">
          <ListTodo :size="20" />
        </mo-enter>
        <mo-enter tag="li" preset="fadeInLeft" :delay="0.2" @click="showAddTask()" class="non-draggable">
          <Plus :size="20" />
        </mo-enter>
        <mo-enter tag="li" preset="fadeInLeft" :delay="0.25" @click="nav('/rss')" class="non-draggable">
          <Rss :size="20" />
        </mo-enter>
      </ul>
      <ul class="menu bottom-menu">
        <mo-enter tag="li" preset="fadeInLeft" :delay="0.32" @click="nav('/health')" class="non-draggable">
          <Activity :size="20" />
        </mo-enter>
        <mo-enter tag="li" preset="fadeInLeft" :delay="0.35" @click="nav('/preference')" class="non-draggable">
          <Settings2 :size="20" />
        </mo-enter>
        <mo-enter tag="li" preset="fadeInLeft" :delay="0.4" @click="showAboutPanel" class="non-draggable">
          <Info :size="20" />
        </mo-enter>
      </ul>
    </div>
  </aside>
</template>

<script lang="ts">
import { Activity, Info, ListTodo, Plus, Rss, Settings2 } from "@lucide/vue";
import { ADD_TASK_TYPE } from "@shared/constants";
import logger from "@shared/utils/logger";
import LogoMini from "@/components/Logo/LogoMini.vue";
import is from "@/shims/platform";
import { useAppStore } from "@/store/app";
import { getRisukoVersion } from "@/utils/version";

export default {
	name: "mo-aside",
	components: {
		[LogoMini.name]: LogoMini,
		Activity,
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
