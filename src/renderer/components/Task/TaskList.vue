<template>
  <div class="task-list-wrapper" v-if="taskList.length > 0">
    <recycle-scroller
      v-if="useVirtualList"
      class="task-list task-list-virtual"
      :items="paginatedTaskList"
      :item-size="112"
      key-field="_displayKey"
    >
      <template #default="{ item }">
        <div :attr="item._displayKey || item.gid" @click="handleItemClick(item, $event)">
          <mo-task-item :task="item" :selected="isItemSelected(item)" />
        </div>
      </template>
    </recycle-scroller>
    <mo-drag-select v-else class="task-list" attribute="attr" @change="handleDragSelectChange">
      <mo-enter
        v-for="(item, index) in paginatedTaskList"
        :key="item._displayKey"
        preset="fadeInUp"
        :duration="0.4"
        :delay="getStaggerDelay(index)"
        :attr="item._displayKey || item.gid"
        @click="handleItemClick(item, $event)"
      >
        <mo-task-item :task="item" :selected="isItemSelected(item)" />
      </mo-enter>
    </mo-drag-select>
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
  <mo-enter v-else preset="fadeInUp" class="no-task">
    <div class="no-task-inner">
      {{ $t('task.no-task') }}
    </div>
  </mo-enter>
</template>

<script lang="ts">
import { cloneDeep } from "lodash";
import DragSelect from "@/components/DragSelect/Index.vue";
import is from "@/shims/platform";
import { useTaskStore } from "@/store/task";
import TaskItem from "./TaskItem.vue";

const VIRTUAL_LIST_THRESHOLD = 120;

export default {
	name: "mo-task-list",
	components: {
		[DragSelect.name]: DragSelect,
		[TaskItem.name]: TaskItem,
	},
	data() {
		// Mirror the store's row keys so selection state survives page changes and remounts
		const selectedList = cloneDeep(useTaskStore().selectedGidList) || [];
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
		useVirtualList() {
			return this.taskList.length >= VIRTUAL_LIST_THRESHOLD;
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
				// Ignore shortcuts while the user is typing
				const tag = (event.target as HTMLElement)?.tagName;
				if (tag === "INPUT" || tag === "TEXTAREA") {
					return;
				}
				event.preventDefault();
				const allKeys = this.paginatedTaskList.map(
					(t) => t._displayKey || t.gid,
				);
				this.selectedList = allKeys;
				useTaskStore().selectTasks(cloneDeep(allKeys));
			}
		},
		handleItemClick(item, event) {
			const key: string = item._displayKey || item.gid;
			// Android has no modifier keys, so a tap toggles this row like Cmd/Ctrl-click
			// Keep the rest selected unless the user uses desktop-style single select
			const isMulti = event.metaKey || event.ctrlKey || is.android();
			const isShift = event.shiftKey;
			let newList: string[];

			if (isShift && this.lastClickedKey) {
				const keys = this.paginatedTaskList.map((t) => t._displayKey || t.gid);
				const anchorIdx = keys.indexOf(this.lastClickedKey);
				const currentIdx = keys.indexOf(key);
				if (anchorIdx !== -1 && currentIdx !== -1) {
					const start = Math.min(anchorIdx, currentIdx);
					const end = Math.max(anchorIdx, currentIdx);
					const rangeKeys = keys.slice(start, end + 1);
					if (isMulti) {
						// Shift+Cmd adds the range to the current selection
						const set = new Set<string>(this.selectedList);
						for (const k of rangeKeys) {
							set.add(k);
						}
						newList = [...set];
					} else {
						// Shift alone replaces the selection with the range
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
			useTaskStore().selectTasks(cloneDeep(newList));
		},
		handleDragSelectChange(selectedList) {
			// DragSelect gives us the row key from `attr`
			this.selectedList = selectedList;
			useTaskStore().selectTasks(cloneDeep(selectedList));
		},
		isItemSelected(item): boolean {
			const key = item._displayKey || item.gid;
			return this.selectedList.includes(key);
		},
	},
	watch: {
		selectedGidList(newVal: string[]) {
			// The store already tracks row keys, so the component just mirrors them
			this.selectedList = newVal;
		},
	},
};
</script>
