<template>
  <div class="task-list-wrapper" v-if="taskList.length > 0">
    <recycle-scroller
      v-if="useVirtualList"
      class="task-list task-list-virtual"
      :items="paginatedTaskList"
      :item-size="virtualItemSize"
      key-field="_displayKey"
    >
      <template #default="{ item }">
        <div :attr="item._displayKey" @click="handleItemClick(item, $event)">
          <task-item :task="item" :selected="isItemSelected(item)" />
        </div>
      </template>
    </recycle-scroller>
    <drag-select v-else class="task-list" attribute="attr" @change="handleDragSelectChange">
      <motion-enter
        v-for="(item, index) in paginatedTaskList"
        :key="item._displayKey"
        preset="fadeInUp"
        :duration="0.4"
        :delay="getStaggerDelay(index)"
        :attr="item._displayKey"
        @click="handleItemClick(item, $event)"
      >
        <task-item :task="item" :selected="isItemSelected(item)" />
      </motion-enter>
    </drag-select>
    <footer class="task-pagination">
      <button
        class="task-pagination-btn"
        type="button"
        :disabled="currentPage <= 1"
        @click="onPrevPageClick"
      >
        {{ $t('task.pagination-prev') }}
      </button>
      <span class="task-pagination-text">{{ currentPage }} / {{ totalPages }}</span>
      <button
        class="task-pagination-btn"
        type="button"
        :disabled="currentPage >= totalPages"
        @click="onNextPageClick"
      >
        {{ $t('task.pagination-next') }}
      </button>
    </footer>
  </div>
  <motion-enter v-else preset="fade" class="no-task">
    <div class="no-task-inner">
      <span class="no-task-icon">
        <Inbox :size="22" />
      </span>
      <p>{{ $t('task.no-task') }}</p>
      <Button size="sm" variant="outline" @click="showAddTask">
        <Plus :size="14" />
        {{ $t('app.add-task') }}
      </Button>
    </div>
  </motion-enter>
</template>

<script lang="ts">
import { Inbox, Plus } from "@lucide/vue";
import { ADD_TASK_TYPE } from "@shared/constants";
import { checkTaskIsBT } from "@shared/utils";
import DragSelect from "@/components/DragSelect/Index.vue";
import { Button } from "@/components/ui/button";
import is from "@/shims/platform";
import { useAppStore } from "@/store/app";
import { usePreferenceStore } from "@/store/preference";
import { useTaskStore } from "@/store/task";
import TaskItem from "./TaskItem.vue";

const VIRTUAL_LIST_THRESHOLD = 120;

export default {
	name: "task-list",
	components: {
		[DragSelect.name]: DragSelect,
		[TaskItem.name]: TaskItem,
		Button,
		Inbox,
		Plus,
	},
	data() {
		const selectedList = [...(useTaskStore().selectedGidList || [])];
		return {
			selectedList,
			lastClickedKey: null as string | null,
		};
	},
	computed: {
		taskList() {
			return useTaskStore().taskList;
		},
		paginatedTaskList() {
			return useTaskStore().paginatedTaskList;
		},
		selectedGidList() {
			return useTaskStore().selectedGidList;
		},
		currentPage() {
			return useTaskStore().currentPage;
		},
		totalPages() {
			return useTaskStore().totalPages;
		},
		virtualItemSize() {
			if (usePreferenceStore().taskListStyle === "card") {
				return is.android() ? 136 : 90;
			}
			return is.android() ? 88 : 64;
		},
		useVirtualList() {
			if (this.taskList.length < VIRTUAL_LIST_THRESHOLD) {
				return false;
			}
			return !this.paginatedTaskList.some((task) => {
				const files = Array.isArray(task.files) ? task.files : [];
				return checkTaskIsBT(task) && files.length > 1;
			});
		},
	},
	mounted() {
		this._onKeyDown = this.onKeyDown.bind(this);
		window.addEventListener("keydown", this._onKeyDown);
	},
	beforeUnmount() {
		window.removeEventListener("keydown", this._onKeyDown);
	},
	methods: {
		showAddTask() {
			useAppStore().showAddTaskDialog(ADD_TASK_TYPE.URI);
		},
		getStaggerDelay(index: number): number {
			const MAX_DELAY = 0.6;
			return Math.min((index + 1) * 0.03, MAX_DELAY);
		},
		onPrevPageClick() {
			if (this.currentPage <= 1) {
				return;
			}
			useTaskStore().changeCurrentPage(this.currentPage - 1);
		},
		onNextPageClick() {
			if (this.currentPage >= this.totalPages) {
				return;
			}
			useTaskStore().changeCurrentPage(this.currentPage + 1);
		},
		onKeyDown(event: KeyboardEvent) {
			if ((event.metaKey || event.ctrlKey) && event.key === "a") {
				const tag = (event.target as HTMLElement)?.tagName;
				if (tag === "INPUT" || tag === "TEXTAREA") {
					return;
				}
				event.preventDefault();
				const allKeys = this.paginatedTaskList.map((t) => t._displayKey);
				this.selectedList = allKeys;
				useTaskStore().selectTasks([...allKeys]);
			}
		},
		handleItemClick(item, event) {
			const key: string = item._displayKey;
			const isMulti = event.metaKey || event.ctrlKey || is.android();
			const isShift = event.shiftKey;
			let newList: string[];

			if (isShift && this.lastClickedKey) {
				const keys = this.paginatedTaskList.map((t) => t._displayKey);
				const anchorIdx = keys.indexOf(this.lastClickedKey);
				const currentIdx = keys.indexOf(key);
				if (anchorIdx !== -1 && currentIdx !== -1) {
					const start = Math.min(anchorIdx, currentIdx);
					const end = Math.max(anchorIdx, currentIdx);
					const rangeKeys = keys.slice(start, end + 1);
					if (isMulti) {
						const set = new Set<string>(this.selectedList);
						for (const k of rangeKeys) {
							set.add(k);
						}
						newList = [...set];
					} else {
						newList = rangeKeys;
					}
				} else {
					newList = [key];
				}
			} else if (isMulti) {
				const idx = this.selectedList.indexOf(key);
				newList =
					idx === -1
						? [...this.selectedList, key]
						: this.selectedList.filter((id) => id !== key);
			} else {
				newList =
					this.selectedList.length === 1 && this.selectedList[0] === key
						? []
						: [key];
			}

			if (!isShift) {
				this.lastClickedKey = key;
			}

			this.selectedList = newList;
			useTaskStore().selectTasks([...newList]);
		},
		handleDragSelectChange(selectedList) {
			this.selectedList = selectedList;
			useTaskStore().selectTasks([...selectedList]);
		},
		isItemSelected(item): boolean {
			const key = item._displayKey;
			return this.selectedList.includes(key);
		},
	},
	watch: {
		selectedGidList(newVal: string[]) {
			this.selectedList = newVal;
		},
	},
};
</script>
