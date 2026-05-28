<template>
  <div id="container" class="layout-root">
    <mo-aside />
    <router-view v-slot="{ Component, route }">
      <Transition name="page" mode="out-in">
        <component
          :is="Component"
          :key="route.path.startsWith('/preference') ? 'preference' : route.path.startsWith('/rss') ? 'rss' : 'task'"
          class="page-view"
        />
      </Transition>
    </router-view>
    <mo-speedometer />
    <nav v-if="isAndroid" class="android-bottom-nav" :aria-label="$t('app.primary-navigation')">
      <button class="android-nav-item" :class="{ active: routePath.startsWith('/task') || routePath === '/' }" @click="nav('/task')">
        <ListTodo :size="22" />
        <span>{{ $t('app.nav-tasks') }}</span>
      </button>
      <button class="android-nav-item" :class="{ active: routePath.startsWith('/rss') }" @click="nav('/rss')">
        <Rss :size="22" />
        <span>{{ $t('app.nav-rss') }}</span>
      </button>
      <button class="android-fab" :aria-label="$t('app.add-task')" @click="showAddTask">
        <Plus :size="28" />
      </button>
      <button class="android-nav-item" :class="{ active: routePath.startsWith('/health') }" @click="nav('/health')">
        <Activity :size="22" />
        <span>{{ $t('app.nav-health') }}</span>
      </button>
      <button class="android-nav-item" :class="{ active: routePath.startsWith('/preference') }" @click="nav('/preference/basic')">
        <Settings2 :size="22" />
        <span>{{ $t('app.nav-settings') }}</span>
      </button>
    </nav>
    <mo-add-task :visible="addTaskVisible" :type="addTaskType" />
    <mo-about-panel :visible="aboutPanelVisible" />
    <mo-task-detail
      :visible="taskDetailVisible"
      :gid="currentTaskGid"
      :task="currentTaskItem"
      :files="currentTaskFiles"
      :peers="currentTaskPeers"
    />
    <mo-dragger />
  </div>
</template>

<script lang="ts">
import { Activity, ListTodo, Plus, Rss, Settings2 } from "@lucide/vue";
import { ADD_TASK_TYPE } from "@shared/constants";
import { invoke } from "@tauri-apps/api/core";
import { toast } from "vue-sonner";
import AboutPanel from "@/components/About/AboutPanel.vue";
import Aside from "@/components/Aside/Index.vue";
import Dragger from "@/components/Dragger/Index.vue";
import Speedometer from "@/components/Speedometer/Speedometer.vue";
import AddTask from "@/components/Task/AddTask.vue";
import TaskDetail from "@/components/TaskDetail/Index.vue";
import is from "@/shims/platform";
import { useAppStore } from "@/store/app";
import { useTaskStore } from "@/store/task";

export default {
	name: "mo-main",
	components: {
		[AboutPanel.name]: AboutPanel,
		[Aside.name]: Aside,
		[Speedometer.name]: Speedometer,
		[AddTask.name]: AddTask,
		[TaskDetail.name]: TaskDetail,
		[Dragger.name]: Dragger,
		Activity,
		ListTodo,
		Plus,
		Rss,
		Settings2,
	},
	data() {
		return {
			androidBackGuardTimer: null as number | null,
			lastAndroidRootBackAt: 0,
		};
	},
	computed: {
		isAndroid() {
			return is.android();
		},
		routePath() {
			return this.$route.path;
		},
		aboutPanelVisible() {
			return useAppStore().aboutPanelVisible;
		},
		addTaskVisible() {
			return useAppStore().addTaskVisible;
		},
		addTaskType() {
			return useAppStore().addTaskType;
		},
		taskDetailVisible() {
			return useTaskStore().taskDetailVisible;
		},
		currentTaskGid() {
			return useTaskStore().currentTaskGid;
		},
		currentTaskItem() {
			return useTaskStore().currentTaskItem;
		},
		currentTaskFiles() {
			return useTaskStore().currentTaskFiles;
		},
		currentTaskPeers() {
			return useTaskStore().currentTaskPeers;
		},
	},
	mounted() {
		this.installAndroidBackGuard();
	},
	beforeUnmount() {
		this.removeAndroidBackGuard();
	},
	watch: {
		routePath() {
			this.scheduleAndroidBackGuard();
		},
	},
	methods: {
		nav(page: string) {
			this.$router
				.push({ path: page })
				.catch(() => undefined)
				.finally(() => this.scheduleAndroidBackGuard());
		},
		showAddTask() {
			useAppStore().showAddTaskDialog(ADD_TASK_TYPE.URI);
		},
		installAndroidBackGuard() {
			if (!this.isAndroid) {
				return;
			}
			this.pushAndroidBackGuard();
			window.addEventListener("popstate", this.onAndroidBackGesture);
			window.addEventListener("risuko-android-back", this.onAndroidBackGesture);
		},
		removeAndroidBackGuard() {
			if (this.androidBackGuardTimer != null) {
				window.clearTimeout(this.androidBackGuardTimer);
				this.androidBackGuardTimer = null;
			}
			window.removeEventListener("popstate", this.onAndroidBackGesture);
			window.removeEventListener(
				"risuko-android-back",
				this.onAndroidBackGesture,
			);
		},
		scheduleAndroidBackGuard() {
			if (!this.isAndroid) {
				return;
			}
			if (this.androidBackGuardTimer != null) {
				window.clearTimeout(this.androidBackGuardTimer);
			}
			this.androidBackGuardTimer = window.setTimeout(() => {
				this.androidBackGuardTimer = null;
				this.pushAndroidBackGuard();
			}, 0);
		},
		pushAndroidBackGuard() {
			if (!this.isAndroid || window.history.state?.risukoAndroidBackGuard) {
				return;
			}
			window.history.pushState(
				{
					...(window.history.state || {}),
					risukoAndroidBackGuard: true,
				},
				"",
				window.location.href,
			);
		},
		onAndroidBackGesture() {
			if (!this.isAndroid) {
				return;
			}
			this.handleAndroidBackGesture();
			this.scheduleAndroidBackGuard();
		},
		handleAndroidBackGesture() {
			const appStore = useAppStore();
			const taskStore = useTaskStore();
			if (appStore.cloudflareDialog.visible) {
				appStore.hideCloudflareDialog();
				return;
			}
			if (appStore.addTaskVisible) {
				appStore.hideAddTaskDialog();
				return;
			}
			if (appStore.aboutPanelVisible) {
				appStore.hideAboutPanel();
				return;
			}
			if (taskStore.taskDetailVisible) {
				taskStore.hideTaskDetail();
				return;
			}
			if (this.routePath !== "/task" && this.routePath !== "/") {
				this.$router.push({ path: "/task" }).catch(() => undefined);
				this.lastAndroidRootBackAt = 0;
				return;
			}
			const now = Date.now();
			if (now - this.lastAndroidRootBackAt <= 1800) {
				this.lastAndroidRootBackAt = 0;
				invoke("quit_app").catch(() => undefined);
				return;
			}
			this.lastAndroidRootBackAt = now;
			toast.info(this.$t("app.android-back-again-to-quit"));
		},
	},
};
</script>
