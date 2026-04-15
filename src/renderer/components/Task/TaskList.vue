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
        <div :attr="item.gid" :class="getItemClass(item)" @click="handleItemClick(item, $event)">
          <mo-task-item :task="item" />
        </div>
      </template>
    </recycle-scroller>
    <mo-drag-select v-else class="task-list" attribute="attr" @change="handleDragSelectChange">
      <div
        v-for="item in paginatedTaskList"
        :key="item._displayKey"
        :attr="item.gid"
        :class="getItemClass(item)"
        :style="{ '--stagger-index': paginatedTaskList.indexOf(item) }"
        @click="handleItemClick(item, $event)"
      >
        <mo-task-item :task="item" />
      </div>
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
  <div class="no-task" v-else>
    <div class="no-task-inner">
      {{ $t('task.no-task') }}
    </div>
  </div>
</template>

<script lang="ts">
import { cloneDeep } from "lodash";
import DragSelect from "@/components/DragSelect/Index.vue";
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
		const selectedList = cloneDeep(useTaskStore().selectedGidList) || [];
		return {
			selectedList,
			lastClickedGid: null as string | null,
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
				// Ignore if user is typing in an input/textarea
				const tag = (event.target as HTMLElement)?.tagName;
				if (tag === "INPUT" || tag === "TEXTAREA") {
					return;
				}
				event.preventDefault();
				const allGids = [...new Set(this.paginatedTaskList.map((t) => t.gid))];
				this.selectedList = allGids;
				useTaskStore().selectTasks(cloneDeep(allGids));
			}
		},
		handleItemClick(item, event) {
			const gid = item.gid;
			const isMulti = event.metaKey || event.ctrlKey;
			const isShift = event.shiftKey;
			let newList: string[];

			if (isShift && this.lastClickedGid) {
				const gids = this.paginatedTaskList.map((t) => t.gid);
				const anchorIdx = gids.indexOf(this.lastClickedGid);
				const currentIdx = gids.indexOf(gid);
				if (anchorIdx !== -1 && currentIdx !== -1) {
					const start = Math.min(anchorIdx, currentIdx);
					const end = Math.max(anchorIdx, currentIdx);
					const rangeGids = gids.slice(start, end + 1);
					if (isMulti) {
						// Shift+Cmd: add range to existing selection
						const set = new Set<string>(this.selectedList);
						for (const g of rangeGids) {
							set.add(g);
						}
						newList = [...set];
					} else {
						// Shift only: replace selection with range
						newList = rangeGids;
					}
				} else {
					newList = [gid];
				}
			} else if (isMulti) {
				const idx = this.selectedList.indexOf(gid);
				newList =
					idx === -1
						? [...this.selectedList, gid]
						: this.selectedList.filter((id) => id !== gid);
			} else {
				newList =
					this.selectedList.length === 1 && this.selectedList[0] === gid
						? []
						: [gid];
			}

			if (!isShift) {
				this.lastClickedGid = gid;
			}

			this.selectedList = newList;
			useTaskStore().selectTasks(cloneDeep(newList));
		},
		handleDragSelectChange(selectedList) {
			this.selectedList = selectedList;
			useTaskStore().selectTasks(cloneDeep(selectedList));
		},
		getItemClass(item) {
			const isSelected = this.selectedList.includes(item.gid);
			return {
				selected: isSelected,
			};
		},
	},
	watch: {
		selectedGidList(newVal) {
			this.selectedList = newVal;
		},
	},
};
</script>
