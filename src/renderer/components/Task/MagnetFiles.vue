<template>
  <div class="magnet-files">
    <mo-loading-overlay :show="resolving" :text="$t('task.loading-resolve-magnet')" />
    <div v-if="error" class="magnet-files-error">
      <span class="magnet-files-error-text">{{ errorMessage }}</span>
      <ui-button size="sm" variant="outline" @click="resolve">
        {{ $t('task.resolve-magnet-retry') }}
      </ui-button>
    </div>
    <mo-task-files
      v-if="resolved && files.length > 0"
      mode="ADD"
      :files="files"
      @selection-change="handleSelectionChange"
    />
  </div>
</template>

<script lang="ts">
import { SELECTED_ALL_FILES } from "@shared/constants";
import { invoke } from "@tauri-apps/api/core";
import TaskFiles from "@/components/TaskDetail/TaskFiles.vue";
import UiButton from "@/components/ui/compat/UiButton.vue";
import LoadingOverlay from "@/components/ui/LoadingOverlay.vue";

interface MagnetFileRaw {
	path: string;
	length: number;
	name: string;
	index: number;
}

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
	name: "mo-magnet-files",
	components: {
		[TaskFiles.name]: TaskFiles,
		[LoadingOverlay.name]: LoadingOverlay,
		UiButton,
	},
	props: {
		magnetUri: {
			type: String,
			default: "",
		},
	},
	emits: ["change"],
	data() {
		return {
			resolving: false,
			resolved: false,
			error: null as string | null,
			files: [] as TaskFileRow[],
		};
	},
	computed: {
		errorMessage(): string {
			if (!this.error) {
				return "";
			}
			if (this.error.includes("Timed out")) {
				return this.$t("task.resolve-magnet-timeout");
			}
			return this.$t("task.resolve-magnet-error");
		},
	},
	watch: {
		magnetUri: {
			immediate: true,
			handler(uri: string) {
				if (uri) {
					this.resolve();
				} else {
					this.reset();
				}
			},
		},
	},
	methods: {
		reset() {
			this.resolving = false;
			this.resolved = false;
			this.error = null;
			this.files = [];
			this.$emit("change", SELECTED_ALL_FILES);
		},
		async resolve() {
			const uri = this.magnetUri;
			if (!uri) {
				return;
			}

			this.resolving = true;
			this.resolved = false;
			this.error = null;
			this.files = [];

			try {
				const result = await invoke<{
					files: MagnetFileRaw[];
					fileCount: number;
				}>("resolve_magnet", { uri });
				// Guard against stale response if URI changed during resolve
				if (this.magnetUri !== uri) {
					return;
				}

				this.files = (result.files || []).map((f) => {
					const ext = extractExtension(f.name);
					return {
						idx: f.index,
						path: f.path,
						name: f.name,
						extension: ext,
						length: f.length,
						completedLength: "0",
						selected: true,
					};
				});
				this.resolved = true;
				this.$emit("change", SELECTED_ALL_FILES);
			} catch (err: unknown) {
				if (this.magnetUri !== uri) {
					return;
				}
				this.error = err instanceof Error ? err.message : String(err);
				this.$emit("change", SELECTED_ALL_FILES);
			} finally {
				if (this.magnetUri === uri) {
					this.resolving = false;
				}
			}
		},
		handleSelectionChange(selectFileIndex: string) {
			this.$emit("change", selectFileIndex);
		},
	},
};

function extractExtension(name: string): string {
	const dot = name.lastIndexOf(".");
	return dot > 0 ? name.slice(dot) : "";
}
</script>

<style scoped>
.magnet-files {
  position: relative;
  min-height: 60px;
}

.magnet-files-error {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 12px;
  font-size: 12px;
  color: var(--color-text-secondary, hsl(var(--muted-foreground)));
}

.magnet-files-error-text {
  flex: 1;
}
</style>
