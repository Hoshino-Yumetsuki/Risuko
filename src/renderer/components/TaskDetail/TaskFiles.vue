<template>
  <div class="task-files" :class="{ 'task-files--detail': mode === 'DETAIL' }" v-if="files">
    <template v-if="mode === 'DETAIL'">
      <div class="task-files-grid-row task-files-grid-head">
        <span>
          <Checkbox :model-value="allSelected" @update:model-value="toggleAll" />
        </span>
        <span>{{ $t('task.file-name') }}</span>
        <span>{{ $t('task.file-extension') }}</span>
        <span class="task-files-num">%</span>
        <span class="task-files-num">{{ $t('task.file-completed-size') }}</span>
        <span class="task-files-num">{{ $t('task.file-size') }}</span>
      </div>
      <recycle-scroller
        class="task-files-scroller"
        :items="files"
        :item-size="36"
        key-field="idx"
      >
        <template #default="{ item }">
          <div
            class="task-files-grid-row"
            :class="{ selected: isSelected(item) }"
            @dblclick="toggleRow(item, !isSelected(item))"
          >
            <span>
              <Checkbox
                :model-value="isSelected(item)"
                @update:model-value="(val) => toggleRow(item, val)"
              />
            </span>
            <button
              type="button"
              class="task-files-name"
              :title="item.path || item.name"
              :aria-pressed="isSelected(item)"
              @click="toggleRow(item, !isSelected(item))"
            >{{ item.name }}</button>
            <span>{{ formatExtension(item.extension) }}</span>
            <span class="task-files-num">{{ calcProgress(item.length, item.completedLength, 1) }}</span>
            <span class="task-files-num">{{ formatBytes(item.completedLength) }}</span>
            <span class="task-files-num">{{ formatBytes(item.length) }}</span>
            <div class="task-files-bar">
              <div
                class="task-files-bar-fill"
                :class="{ 'task-files-bar-fill--failed': item.failed }"
                :style="{ width: `${calcProgress(item.length, item.completedLength)}%` }"
              ></div>
            </div>
          </div>
        </template>
      </recycle-scroller>
    </template>
    <div v-else class="table-wrapper">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead class="w-10.5">
              <Checkbox :model-value="allSelected" @update:model-value="toggleAll" />
            </TableHead>
            <TableHead class="min-w-50">{{ $t('task.file-name') }}</TableHead>
            <TableHead class="w-20">{{ $t('task.file-extension') }}</TableHead>
            <TableHead class="w-21.25 text-right">{{ $t('task.file-size') }}</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow
            v-for="row in files"
            :key="row.idx"
            @dblclick="toggleRow(row, !isSelected(row))"
            :class="{ 'bg-muted/50': isSelected(row) }"
          >
            <TableCell>
              <Checkbox
                :model-value="isSelected(row)"
                @update:model-value="(val) => toggleRow(row, val)"
              />
            </TableCell>
            <TableCell class="max-w-50">
              <button
                type="button"
                class="task-files-name"
                :title="row.path || row.name"
                :aria-pressed="isSelected(row)"
                @click="toggleRow(row, !isSelected(row))"
              >
                {{ row.name }}
              </button>
            </TableCell>
            <TableCell>{{ formatExtension(row.extension) }}</TableCell>
            <TableCell class="text-right">{{ formatBytes(row.length) }}</TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </div>
    <div class="files-toolbar">
      <div class="files-toolbar-filters">
        <ui-button
          size="sm"
          variant="outline"
          :title="$t('task.filter-video')"
          :aria-label="$t('task.filter-video')"
          @click="toggleVideoSelection()"
          ><Video :size="12"
        /></ui-button>
        <ui-button
          size="sm"
          variant="outline"
          :title="$t('task.filter-audio')"
          :aria-label="$t('task.filter-audio')"
          @click="toggleAudioSelection()"
          ><Headphones :size="12"
        /></ui-button>
        <ui-button
          size="sm"
          variant="outline"
          :title="$t('task.filter-image')"
          :aria-label="$t('task.filter-image')"
          @click="toggleImageSelection()"
          ><Image :size="12"
        /></ui-button>
        <ui-button
          size="sm"
          variant="outline"
          :title="$t('task.filter-document')"
          :aria-label="$t('task.filter-document')"
          @click="toggleDocumentSelection()"
          ><FileText :size="12"
        /></ui-button>
      </div>
      <div class="files-toolbar-summary">
        {{
          $t('task.selected-files-sum', {
            selectedFilesCount,
            selectedFilesTotalSize,
          })
        }}
      </div>
    </div>
  </div>
</template>
<script lang="ts">
import { FileText, Headphones, Image, Video } from "@lucide/vue";
import {
	AUDIO_SUFFIXES,
	DOCUMENT_SUFFIXES,
	IMAGE_SUFFIXES,
	NONE_SELECTED_FILES,
	SELECTED_ALL_FILES,
	SUB_SUFFIXES,
	VIDEO_SUFFIXES,
} from "@shared/constants";
import {
	bytesToSize,
	calcProgress,
	filterFilesBySuffix,
	removeExtensionDot,
} from "@shared/utils";
import { isEmpty } from "lodash";
import { Checkbox } from "@/components/ui/checkbox";
import UiButton from "@/components/ui/compat/UiButton.vue";
import {
	Table,
	TableBody,
	TableCell,
	TableHead,
	TableHeader,
	TableRow,
} from "@/components/ui/table";

interface TaskFileRow {
	idx: number;
	path: string;
	name: string;
	extension: string;
	length: number;
	completedLength: string;
	selected: boolean;
	failed?: boolean;
}

export default {
	name: "task-files",
	components: {
		[UiButton.name]: UiButton,
		Checkbox,
		Table,
		TableBody,
		TableCell,
		TableHead,
		TableHeader,
		TableRow,
		Video,
		Headphones,
		Image,
		FileText,
	},
	props: {
		mode: {
			type: String,
			default: "ADD",
			validator: (value: string) => ["ADD", "DETAIL"].includes(value),
		},
		files: { type: Array, default: () => [] },
	},
	data() {
		return { selectedIndices: new Set<number>() };
	},
	computed: {
		allSelected() {
			return (
				this.files?.length > 0 &&
				this.selectedIndices.size === this.files.length
			);
		},
		selectedFiles() {
			return (this.files as TaskFileRow[]).filter((f) =>
				this.selectedIndices.has(f.idx),
			);
		},
		selectedFilesCount() {
			return this.selectedIndices.size;
		},
		selectedFilesTotalSize() {
			const total = this.selectedFiles.reduce(
				(acc: number, cur: TaskFileRow) =>
					acc + parseInt(String(cur.length), 10),
				0,
			);
			return bytesToSize(total);
		},
		selectedFileIndex() {
			const { files, selectedIndices } = this;
			if ((files as TaskFileRow[]).length === 0 || selectedIndices.size === 0) {
				return NONE_SELECTED_FILES;
			}
			if ((files as TaskFileRow[]).length === selectedIndices.size) {
				return SELECTED_ALL_FILES;
			}
			const arr = Array.from(selectedIndices) as number[];
			arr.sort((a, b) => a - b);
			return arr.join(",");
		},
	},
	watch: {
		selectedFileIndex() {
			this.$emit("selection-change", this.selectedFileIndex);
		},
	},
	methods: {
		calcProgress,
		formatBytes: bytesToSize,
		formatExtension: removeExtensionDot,
		isSelected(row: TaskFileRow) {
			return this.selectedIndices.has(row.idx);
		},
		toggleAll(checked: boolean | "indeterminate") {
			this.selectedIndices =
				checked === true
					? new Set((this.files as TaskFileRow[]).map((f) => f.idx))
					: new Set();
		},
		toggleRow(row: TaskFileRow, selected: boolean | "indeterminate") {
			const next = new Set(this.selectedIndices);
			if (selected === true) {
				next.add(row.idx);
			} else {
				next.delete(row.idx);
			}
			this.selectedIndices = next;
		},
		toggleAllSelection() {
			this.selectedIndices = new Set(
				(this.files as TaskFileRow[]).map((f) => f.idx),
			);
		},
		clearSelection() {
			this.selectedIndices = new Set();
		},
		toggleSelection(rows: TaskFileRow[]) {
			this.selectedIndices = isEmpty(rows)
				? new Set()
				: new Set(rows.map((r) => r.idx));
		},
		toggleVideoSelection() {
			this.toggleSelection(
				filterFilesBySuffix(this.files, [...VIDEO_SUFFIXES, ...SUB_SUFFIXES]),
			);
		},
		toggleAudioSelection() {
			this.toggleSelection(filterFilesBySuffix(this.files, AUDIO_SUFFIXES));
		},
		toggleImageSelection() {
			this.toggleSelection(filterFilesBySuffix(this.files, IMAGE_SUFFIXES));
		},
		toggleDocumentSelection() {
			this.toggleSelection(filterFilesBySuffix(this.files, DOCUMENT_SUFFIXES));
		},
	},
};
</script>
