<template>
  <div class="mo-task-files" v-if="files">
    <div class="mo-table-wrapper">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead class="w-10.5">
              <Checkbox :model-value="allSelected" @update:model-value="toggleAll" />
            </TableHead>
            <TableHead class="min-w-50">{{ $t('task.file-name') }}</TableHead>
            <TableHead class="w-20">{{ $t('task.file-extension') }}</TableHead>
            <TableHead v-if="mode === 'DETAIL'" class="w-12.5 text-right">%</TableHead>
            <TableHead v-if="mode === 'DETAIL'" class="w-21.25 text-right">{{
              $t('task.file-completed-size')
            }}</TableHead>
            <TableHead class="w-21.25 text-right">{{ $t('task.file-size') }}</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow
            v-for="row in files"
            :key="row.idx"
            @dblclick="handleRowDbClick(row)"
            :class="{ 'bg-muted/50': isSelected(row) }"
          >
            <TableCell>
              <Checkbox
                :model-value="isSelected(row)"
                @update:model-value="(val) => toggleRow(row, val)"
              />
            </TableCell>
            <TableCell class="truncate max-w-50" :title="row.path || row.name">
              {{ row.name }}
            </TableCell>
            <TableCell>{{ formatExtension(row.extension) }}</TableCell>
            <TableCell v-if="mode === 'DETAIL'" class="text-right">{{
              calcProgress(row.length, row.completedLength, 1)
            }}</TableCell>
            <TableCell v-if="mode === 'DETAIL'" class="text-right">{{
              formatBytes(row.completedLength)
            }}</TableCell>
            <TableCell class="text-right">{{ formatBytes(row.length) }}</TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </div>
    <div class="files-toolbar">
      <div class="files-toolbar-filters">
        <ui-button size="sm" variant="outline" @click="toggleVideoSelection()"
          ><Video :size="12"
        /></ui-button>
        <ui-button size="sm" variant="outline" @click="toggleAudioSelection()"
          ><Headphones :size="12"
        /></ui-button>
        <ui-button size="sm" variant="outline" @click="toggleImageSelection()"
          ><Image :size="12"
        /></ui-button>
        <ui-button size="sm" variant="outline" @click="toggleDocumentSelection()"
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
import { NONE_SELECTED_FILES, SELECTED_ALL_FILES } from "@shared/constants";
import {
	bytesToSize,
	calcProgress,
	filterAudioFiles,
	filterDocumentFiles,
	filterImageFiles,
	filterVideoFiles,
	removeExtensionDot,
} from "@shared/utils";
import { isEmpty } from "lodash";
import { FileText, Headphones, Image, Video } from "lucide-vue-next";
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
}

export default {
	name: "mo-task-files",
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
		height: { type: [Number, String] },
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
		formatBytes(value: string | number) {
			return bytesToSize(value);
		},
		formatExtension(value: string) {
			return removeExtensionDot(value);
		},
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
		toggleRowSelection(row: TaskFileRow, selected?: boolean) {
			const next = new Set(this.selectedIndices);
			if (selected === undefined) {
				if (next.has(row.idx)) {
					next.delete(row.idx);
				} else {
					next.add(row.idx);
				}
			} else if (selected) {
				next.add(row.idx);
			} else {
				next.delete(row.idx);
			}
			this.selectedIndices = next;
		},
		toggleSelection(rows: TaskFileRow[]) {
			this.selectedIndices = isEmpty(rows)
				? new Set()
				: new Set(rows.map((r) => r.idx));
		},
		toggleVideoSelection() {
			this.toggleSelection(filterVideoFiles(this.files));
		},
		toggleAudioSelection() {
			this.toggleSelection(filterAudioFiles(this.files));
		},
		toggleImageSelection() {
			this.toggleSelection(filterImageFiles(this.files));
		},
		toggleDocumentSelection() {
			this.toggleSelection(filterDocumentFiles(this.files));
		},
		handleRowDbClick(row: TaskFileRow) {
			this.toggleRowSelection(row);
		},
	},
};
</script>
