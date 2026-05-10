<template>
  <div class="main panel panel-layout panel-layout--h">
    <aside class="subnav hidden-xs-only subnav-pane">
      <mo-task-subnav :current="status" />
    </aside>
    <div class="content panel panel-layout panel-layout--v relative">
      <mo-enter tag="header" preset="fadeInDown" class="panel-header">
        <h4 class="task-title hidden-xs-only">{{ title }}</h4>
        <mo-subnav-switcher :title="title" :subnavs="subnavs" class="hidden-sm-and-up" />
        <mo-task-actions />
      </mo-enter>
      <div class="task-toolbar">
        <div class="task-toolbar-left">
          <div class="task-filter-input">
            <Search :size="12" class="task-filter-icon" />
            <input
              type="text"
              class="task-filter-field"
              :placeholder="$t('task.filter-placeholder')"
              :value="filterText"
              @input="onFilterInput"
            />
            <button
              v-if="filterText"
              class="task-filter-clear"
              @click="onFilterClear"
            >
              <X :size="12" />
            </button>
          </div>
        </div>
        <div class="task-toolbar-right">
          <Select
            v-if="availableTags.length > 0 || filterTag"
            :model-value="filterTag || '__all__'"
            @update:model-value="onFilterTagChange"
          >
            <SelectTrigger size="sm" class="task-toolbar-select" style="min-width: 100px">
              <SelectValue placeholder="Tag" />
            </SelectTrigger>
            <SelectContent align="end">
              <SelectItem value="__all__">{{ $t('task.filter-tag-all') }}</SelectItem>
              <SelectItem v-for="tag in availableTags" :key="tag" :value="tag">
                {{ tag }}
              </SelectItem>
            </SelectContent>
          </Select>
          <Select :model-value="sortBy" @update:model-value="onSortByChange">
            <SelectTrigger size="sm" class="task-toolbar-select">
              <SelectValue />
            </SelectTrigger>
            <SelectContent align="end">
              <SelectItem v-for="item in sortOptions" :key="item.value" :value="item.value">
                {{ item.label }}
              </SelectItem>
            </SelectContent>
          </Select>
          <i
            v-if="sortBy !== 'default'"
            class="task-action task-sort-order"
            @click="onToggleSortOrder"
            :title="sortOrder === 'asc' ? $t('task.sort-ascending') : $t('task.sort-descending')"
          >
            <ArrowUpNarrowWide v-if="sortOrder === 'asc'" :size="14" />
            <ArrowDownNarrowWide v-else :size="14" />
          </i>
        </div>
      </div>
      <main class="panel-content">
        <mo-task-list />
      </main>
      <mo-loading-overlay :show="taskActionLoading" :text="taskActionLoadingText" />
    </div>
  </div>
</template>

<script lang="ts">
import { ADD_TASK_TYPE } from "@shared/constants";
import { getTaskUri, parseHeader } from "@shared/utils";
import logger from "@shared/utils/logger";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import {
	ArrowDownNarrowWide,
	ArrowUpNarrowWide,
	Search,
	X,
} from "lucide-vue-next";
import api from "@/api";
import { commands } from "@/components/CommandManager/instance";
import SubnavSwitcher from "@/components/Subnav/SubnavSwitcher.vue";
import TaskSubnav from "@/components/Subnav/TaskSubnav.vue";
import TaskActions from "@/components/Task/TaskActions.vue";
import TaskList from "@/components/Task/TaskList.vue";
import { confirm } from "@/components/ui/confirm-dialog";
import LoadingOverlay from "@/components/ui/LoadingOverlay.vue";
import {
	Select,
	SelectContent,
	SelectItem,
	SelectTrigger,
	SelectValue,
} from "@/components/ui/select";
import { useAppStore } from "@/store/app";
import { usePreferenceStore } from "@/store/preference";
import { useTaskStore } from "@/store/task";
import { moveTaskFilesToTrash, showItemInFolder } from "@/utils/native";

export default {
	name: "mo-content-task",
	components: {
		[TaskSubnav.name]: TaskSubnav,
		[TaskActions.name]: TaskActions,
		[TaskList.name]: TaskList,
		[SubnavSwitcher.name]: SubnavSwitcher,
		[LoadingOverlay.name]: LoadingOverlay,
		Select,
		SelectContent,
		SelectItem,
		SelectTrigger,
		SelectValue,
		Search,
		X,
		ArrowUpNarrowWide,
		ArrowDownNarrowWide,
	},
	props: {
		status: {
			type: String,
			default: "all",
		},
	},
	computed: {
		taskList() {
			return useTaskStore().taskList;
		},
		selectedGidList() {
			return useTaskStore().selectedGidList;
		},
		selectedGidListCount() {
			return useTaskStore().selectedGidList.length;
		},
		noConfirmBeforeDelete() {
			return usePreferenceStore().config.noConfirmBeforeDeleteTask;
		},
		subnavs() {
			return [
				{
					key: "all",
					title: this.$t("task.all"),
					route: "/task/all",
				},
				{
					key: "active",
					title: this.$t("task.active"),
					route: "/task/active",
				},
				{
					key: "waiting",
					title: this.$t("task.waiting"),
					route: "/task/waiting",
				},
				{
					key: "completed",
					title: this.$t("task.completed"),
					route: "/task/completed",
				},
				{
					key: "stopped",
					title: this.$t("task.stopped"),
					route: "/task/stopped",
				},
			];
		},
		title() {
			const subnav = this.subnavs.find((item) => item.key === this.status);
			return subnav.title;
		},
		filterText() {
			return useTaskStore().filterText;
		},
		filterTag() {
			return useTaskStore().filterTag;
		},
		availableTags() {
			const tags = new Set<string>();
			for (const task of useTaskStore().taskList) {
				if (task.tag) {
					tags.add(task.tag);
				}
			}
			return Array.from(tags).sort();
		},
		sortBy() {
			return useTaskStore().sortBy;
		},
		sortOrder() {
			return useTaskStore().sortOrder;
		},
		sortOptions() {
			return [
				{ label: this.$t("task.sort-default"), value: "default" },
				{ label: this.$t("task.sort-by-name"), value: "name" },
				{ label: this.$t("task.sort-by-size"), value: "size" },
				{ label: this.$t("task.sort-by-time"), value: "time" },
			];
		},
	},
	watch: {
		status: "onStatusChange",
	},
	data() {
		return {
			taskActionLoading: false,
			taskActionLoadingText: "",
		};
	},
	methods: {
		onFilterInput(event) {
			useTaskStore().setFilterText(event.target.value);
		},
		onFilterClear() {
			useTaskStore().setFilterText("");
		},
		onFilterTagChange(value) {
			useTaskStore().setFilterTag(value === "__all__" ? "" : value);
		},
		onSortByChange(value) {
			useTaskStore().setSortBy(value);
		},
		onToggleSortOrder() {
			useTaskStore().toggleSortOrder();
		},
		async withTaskActionLoading(loadingText, action: () => Promise<unknown>) {
			if (this.taskActionLoading) {
				return false;
			}

			this.taskActionLoading = true;
			this.taskActionLoadingText = `${loadingText || ""}`;
			const startTime = Date.now();
			const MIN_DISPLAY_MS = 400;
			try {
				await action();
				return true;
			} finally {
				const elapsed = Date.now() - startTime;
				if (elapsed < MIN_DISPLAY_MS) {
					await new Promise((r) => setTimeout(r, MIN_DISPLAY_MS - elapsed));
				}
				this.taskActionLoading = false;
				this.taskActionLoadingText = "";
			}
		},
		onStatusChange() {
			this.changeCurrentList();
		},
		changeCurrentList() {
			useTaskStore().changeCurrentList(this.status);
		},
		directAddTask(uri, options = {}) {
			const uris = [uri];
			const payload = {
				uris,
				outs: [] as string[],
				options: {
					...options,
				},
			};
			useTaskStore()
				.addUri(payload)
				.catch((err) => {
					this.$msg.error(err.message);
				});
		},
		showAddTaskDialog(
			uri: string,
			options: { header?: string; [key: string]: unknown } = {},
		) {
			const { header, ...rest } = options;
			logger.log("[Risuko] show add task dialog options: ", options);

			const headers = parseHeader(header as string);
			const newOptions = {
				...rest,
				...headers,
			};

			const appStore = useAppStore();
			appStore.updateAddTaskUrl(uri);
			appStore.updateAddTaskOptions(newOptions);
			appStore.showAddTaskDialog(ADD_TASK_TYPE.URI);
		},
		async deleteTaskFiles(task) {
			let targetTask = task;
			// For per-file rows of a multi-file BT torrent, keep the per-file
			// shape (files: [theFile], bittorrent.info cleared) so only that
			// single file gets trashed instead of the whole torrent folder
			if (targetTask?.gid && !targetTask._isFileEntry) {
				try {
					const fullTask = await api.fetchTaskItem({
						gid: targetTask.gid,
					});
					if (fullTask) {
						targetTask = {
							...targetTask,
							...fullTask,
						};
					}
				} catch (err) {
					logger.warn(
						"[Risuko] fetch full task before delete files failed:",
						err,
					);
				}
			}

			const result = await moveTaskFilesToTrash(targetTask);
			if (!result) {
				throw new Error("task.remove-task-file-fail");
			}
			return true;
		},
		async removeTask(task, taskName, isRemoveWithFiles = false) {
			const loadingText = this.$t("task.loading-delete-task");
			// File deletion must finish inside the same loading window as the
			// task removal. Otherwise the user can re-add the same magnet
			// before the files are trashed; the new torrent then scans the
			// still-present files, marks every piece local and reports
			// "seeding" without ever actually downloading
			return this.withTaskActionLoading(loadingText, async () => {
				await this.removeTaskItem(task, taskName);
				if (isRemoveWithFiles) {
					try {
						await this.deleteTaskFiles(task);
					} catch (err) {
						logger.warn("[Risuko] file delete failed:", err);
						this.$msg.error(
							this.$t(err?.message || "task.remove-task-file-fail"),
						);
					}
				}
			});
		},
		async removeTaskRecord(task, taskName, isRemoveWithFiles = false) {
			const loadingText = this.$t("task.loading-remove-record");
			return this.withTaskActionLoading(loadingText, async () => {
				await this.removeTaskRecordItem(task, taskName);
				if (isRemoveWithFiles) {
					try {
						await this.deleteTaskFiles(task);
					} catch (err) {
						logger.warn("[Risuko] file delete failed:", err);
						this.$msg.error(
							this.$t(err?.message || "task.remove-task-file-fail"),
						);
					}
				}
			});
		},
		async removeTaskItem(task, taskName) {
			try {
				await useTaskStore().removeTask(task);
				this.$msg.success(
					this.$t("task.delete-task-success", {
						taskName,
					}),
				);
			} catch ({ code }) {
				if (code === 1) {
					this.$msg.error(
						this.$t("task.delete-task-fail", {
							taskName,
						}),
					);
				}
			}
		},
		async removeTaskRecordItem(task, taskName) {
			try {
				await useTaskStore().removeTaskRecord(task);
				this.$msg.success(
					this.$t("task.remove-record-success", {
						taskName,
					}),
				);
			} catch ({ code }) {
				if (code === 1) {
					this.$msg.error(
						this.$t("task.remove-record-fail", {
							taskName,
						}),
					);
				}
			}
		},
		async removeTasks(taskList, isRemoveWithFiles = false) {
			const loadingText = this.$t("task.loading-batch-delete-task");
			// Await file deletion so re-adding any of the same magnet links
			// can't race the trash operation (see removeTask comment above).
			return this.withTaskActionLoading(loadingText, async () => {
				const gids = taskList.map((task) => task.gid);
				await this.removeTaskItems(gids);
				if (isRemoveWithFiles) {
					try {
						await this.batchDeleteTaskFiles(taskList);
					} catch (err) {
						logger.warn("[Risuko] batch file delete failed:", err);
						this.$msg.error(
							this.$t(err?.message || "task.remove-task-file-fail"),
						);
					}
				}
			});
		},
		async batchDeleteTaskFiles(taskList) {
			const results = await Promise.allSettled(
				taskList.map((task) => this.deleteTaskFiles(task)),
			);
			let failed = false;
			results.forEach((r, i) => {
				if (r.status === "rejected") {
					failed = true;
					const task = taskList[i];
					logger.warn(
						`[Risuko] delete task files failed (gid=${task?.gid}, name=${
							task?.bt_name || task?.bittorrent?.info?.name || ""
						}):`,
						r.reason,
					);
				}
			});
			if (failed) {
				throw new Error("task.remove-task-file-fail");
			}
			return true;
		},
		async removeTaskItems(gids) {
			try {
				await useTaskStore().batchRemoveTask(gids);
				this.$msg.success(this.$t("task.batch-delete-task-success"));
			} catch ({ code }) {
				if (code === 1) {
					this.$msg.error(this.$t("task.batch-delete-task-fail"));
				}
			}
		},
		handlePauseTask(payload) {
			const { task, taskName } = payload;
			this.$msg.info(this.$t("task.download-pause-message", { taskName }));
			useTaskStore()
				.pauseTask(task)
				.catch(({ code }) => {
					if (code === 1) {
						this.$msg.error(this.$t("task.pause-task-fail", { taskName }));
					}
				});
		},
		handleResumeTask(payload) {
			const { task, taskName } = payload;
			useTaskStore()
				.resumeTask(task)
				.catch(({ code }) => {
					if (code === 1) {
						this.$msg.error(
							this.$t("task.resume-task-fail", {
								taskName,
							}),
						);
					}
				});
		},
		handleStopTaskSeeding(payload) {
			const { task } = payload;
			useTaskStore().stopSeeding(task);
			this.$msg.info({
				message: this.$t("task.bt-stopping-seeding-tip"),
				duration: 8000,
			});
		},
		handleRestartTask(payload) {
			const { task, taskName, showDialog } = payload;
			const { gid } = task;
			const uri = getTaskUri(task);

			useTaskStore()
				.getTaskOption(gid)
				.then((data: Record<string, string>) => {
					logger.log("[Risuko] get task option:", data);
					const { dir, header, split } = data;
					const options = {
						dir,
						header,
						split,
						out: taskName,
					};

					if (showDialog) {
						this.showAddTaskDialog(uri, options);
					} else {
						this.directAddTask(uri, options);
						useTaskStore().removeTaskRecord(task);
					}
				});
		},
		handleRevealInFolder(payload) {
			const { path, fallbackPath } = payload;
			showItemInFolder(path, {
				errorMsg: this.$t("task.file-not-exist"),
				fallbackPath,
			});
		},
		async handleDeleteTask(payload) {
			const { task, taskName, deleteWithFiles } = payload;
			const { noConfirmBeforeDelete } = this;

			if (noConfirmBeforeDelete) {
				this.removeTask(task, taskName, deleteWithFiles);
				return;
			}

			const { confirmed, checkboxChecked } = await confirm({
				message: this.$t("task.delete-task-confirm", { taskName }),
				title: this.$t("task.delete-task"),
				kind: "warning",
				confirmText: this.$t("app.yes"),
				cancelText: this.$t("app.no"),
				checkboxLabel: this.$t("task.delete-task-label"),
				checkboxChecked: deleteWithFiles,
			});
			if (confirmed) {
				this.removeTask(task, taskName, checkboxChecked);
			}
		},
		async handleDeleteTaskRecord(payload) {
			const { task, taskName, deleteWithFiles } = payload;
			const { noConfirmBeforeDelete } = this;

			if (noConfirmBeforeDelete) {
				this.removeTaskRecord(task, taskName, deleteWithFiles);
				return;
			}

			const { confirmed, checkboxChecked } = await confirm({
				message: this.$t("task.remove-record-confirm", { taskName }),
				title: this.$t("task.remove-record"),
				kind: "warning",
				confirmText: this.$t("app.yes"),
				cancelText: this.$t("app.no"),
				checkboxLabel: this.$t("task.remove-record-label"),
				checkboxChecked: !!deleteWithFiles,
			});
			if (confirmed) {
				this.removeTaskRecord(task, taskName, checkboxChecked);
			}
		},
		async handleBatchDeleteTask(payload) {
			const { deleteWithFiles } = payload;
			const {
				noConfirmBeforeDelete,
				selectedGidList,
				selectedGidListCount,
				taskList,
			} = this;
			if (selectedGidListCount === 0) {
				return;
			}

			const selectedTaskList = taskList.filter((task) => {
				return selectedGidList.includes(task.gid);
			});

			if (noConfirmBeforeDelete) {
				this.removeTasks(selectedTaskList, deleteWithFiles);
				return;
			}

			const count = `${selectedGidListCount}`;
			const { confirmed, checkboxChecked } = await confirm({
				message: this.$t("task.batch-delete-task-confirm", { count }),
				title: this.$t("task.delete-selected-tasks"),
				kind: "warning",
				confirmText: this.$t("app.yes"),
				cancelText: this.$t("app.no"),
				checkboxLabel: this.$t("task.delete-task-label"),
				checkboxChecked: deleteWithFiles,
			});
			if (confirmed) {
				this.removeTasks(selectedTaskList, checkboxChecked);
			}
		},
		handleCopyTaskLink(payload) {
			const { task } = payload;
			const uri = getTaskUri(task);
			writeText(uri).then(() => {
				this.$msg.success(this.$t("task.copy-link-success"));
			});
		},
		handleShowTaskInfo(payload) {
			const { task } = payload;
			useTaskStore().showTaskDetail(task);
		},
	},
	created() {
		this.changeCurrentList();
	},
	mounted() {
		commands.on("pause-task", this.handlePauseTask);
		commands.on("resume-task", this.handleResumeTask);
		commands.on("stop-task-seeding", this.handleStopTaskSeeding);
		commands.on("restart-task", this.handleRestartTask);
		commands.on("reveal-in-folder", this.handleRevealInFolder);
		commands.on("delete-task", this.handleDeleteTask);
		commands.on("delete-task-record", this.handleDeleteTaskRecord);
		commands.on("batch-delete-task", this.handleBatchDeleteTask);
		commands.on("copy-task-link", this.handleCopyTaskLink);
		commands.on("show-task-info", this.handleShowTaskInfo);
	},
	beforeUnmount() {
		commands.off("pause-task", this.handlePauseTask);
		commands.off("resume-task", this.handleResumeTask);
		commands.off("stop-task-seeding", this.handleStopTaskSeeding);
		commands.off("restart-task", this.handleRestartTask);
		commands.off("reveal-in-folder", this.handleRevealInFolder);
		commands.off("delete-task", this.handleDeleteTask);
		commands.off("delete-task-record", this.handleDeleteTaskRecord);
		commands.off("batch-delete-task", this.handleBatchDeleteTask);
		commands.off("copy-task-link", this.handleCopyTaskLink);
		commands.off("show-task-info", this.handleShowTaskInfo);
	},
};
</script>
