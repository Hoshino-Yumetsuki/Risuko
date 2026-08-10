<template>
  <Dialog :open="open" @update:open="onDialogOpenChange">
    <DialogContent class="health-log-dialog sm:max-w-[min(1100px,calc(100%-2rem))]">
      <DialogHeader>
        <DialogTitle>{{ $t('health.logs-title') }}</DialogTitle>
      </DialogHeader>

      <div class="health-log-toolbar">
        <label class="health-log-field">
          <span>{{ $t('health.logs-file') }}</span>
          <select
            class="health-log-select"
            :value="logFileName || ''"
            :disabled="logsLoading || logFiles.length === 0"
            @change="onFileChange"
          >
            <option value="" disabled>{{ $t('health.logs-no-files') }}</option>
            <option v-for="file in logFiles" :key="file.name" :value="file.name">
              {{ file.name }} · {{ formatFileSize(file.sizeBytes) }}
            </option>
          </select>
        </label>
        <label class="health-log-field">
          <span>{{ $t('health.logs-level') }}</span>
          <select
            class="health-log-select"
            :value="levelFilter"
            :disabled="logsLoading"
            @change="onLevelChange"
          >
            <option value="all">{{ $t('health.logs-all-levels') }}</option>
            <option v-for="level in levels" :key="level" :value="level">
              {{ $t(`health.log-levels.${level}`) }}
            </option>
          </select>
        </label>
        <label class="health-log-field health-log-search">
          <span>{{ $t('health.logs-search') }}</span>
          <input
            v-model="searchText"
            class="health-log-input"
            type="search"
            :placeholder="$t('health.logs-search-placeholder')"
            :aria-label="$t('health.logs-search')"
          />
        </label>
        <Button
          size="sm"
          variant="outline"
          class="health-log-refresh"
          :disabled="logsLoading"
          :title="$t('health.logs-refresh')"
          :aria-label="$t('health.logs-refresh')"
          @click="refreshLogs"
        >
          <RefreshCw :size="14" :class="{ 'animate-spin': logsLoading }" />
          <span>{{ $t('health.logs-refresh') }}</span>
        </Button>
      </div>

      <div v-if="logsError" class="health-log-error" role="alert">
        <AlertCircle :size="15" />
        <span>{{ $t('health.logs-error', { error: logsError }) }}</span>
      </div>
      <div v-else-if="logsLoading && logEntries.length === 0" class="health-log-state">
        <RefreshCw :size="16" class="animate-spin" />
        <span>{{ $t('health.logs-loading') }}</span>
      </div>
      <div v-else-if="logFiles.length === 0" class="health-log-state">
        <FileText :size="18" />
        <span>{{ $t('health.logs-no-files') }}</span>
      </div>
      <div v-else-if="displayEntries.length === 0" class="health-log-state">
        <Search :size="18" />
        <span>{{ $t('health.logs-empty') }}</span>
      </div>
      <div v-else class="health-log-list" role="log" aria-live="polite">
        <div
          v-for="entry in displayEntries"
          :key="`${entry.lineNumber}-${entry.raw}`"
          class="health-log-entry"
          :class="`health-log-entry--${entry.level}`"
        >
          <span class="health-log-entry-line">{{ entry.lineNumber }}</span>
          <span class="health-log-entry-level">{{ $t(`health.log-levels.${entry.level}`) }}</span>
          <span v-if="entry.timestamp" class="health-log-entry-time">{{ entry.timestamp }}</span>
          <code class="health-log-entry-message">{{ entry.message || entry.raw }}</code>
        </div>
      </div>

      <p v-if="logsTruncated" class="health-log-truncated">
        {{ $t('health.logs-truncated') }}
      </p>

      <DialogFooter>
        <Button variant="outline" @click="close">{{ $t('window.close') }}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>

<script lang="ts">
import { AlertCircle, FileText, RefreshCw, Search } from "@lucide/vue";
import type { LogEntry, LogLevel } from "@shared/types/log";
import { mapState } from "pinia";
import { Button } from "@/components/ui/button";
import {
	Dialog,
	DialogContent,
	DialogFooter,
	DialogHeader,
	DialogTitle,
} from "@/components/ui/dialog";
import { useHealthStore } from "@/store/health";

const LOG_LEVELS: LogLevel[] = ["trace", "debug", "info", "warn", "error"];

export default {
	name: "health-log-inspector",
	components: {
		AlertCircle,
		Button,
		Dialog,
		DialogContent,
		DialogFooter,
		DialogHeader,
		DialogTitle,
		FileText,
		RefreshCw,
		Search,
	},
	props: {
		open: {
			type: Boolean,
			default: false,
		},
	},
	emits: ["update:open"],
	data() {
		return {
			levelFilter: "all" as LogLevel | "all",
			searchText: "",
			levels: LOG_LEVELS,
		};
	},
	computed: {
		...mapState(useHealthStore, [
			"logFiles",
			"logEntries",
			"logFileName",
			"logsLoading",
			"logsError",
			"logsTruncated",
		]),
		displayEntries(): LogEntry[] {
			const query = this.searchText.trim().toLocaleLowerCase();
			if (!query) {
				return this.logEntries as LogEntry[];
			}
			return (this.logEntries as LogEntry[]).filter((entry) =>
				`${entry.raw} ${entry.message}`.toLocaleLowerCase().includes(query),
			);
		},
	},
	watch: {
		open(value: boolean) {
			if (value) {
				this.refreshLogs();
			}
		},
	},
	methods: {
		onDialogOpenChange(value: boolean) {
			this.$emit("update:open", value);
		},
		close() {
			this.$emit("update:open", false);
		},
		requestLevels(): LogLevel[] | undefined {
			return this.levelFilter === "all" ? undefined : [this.levelFilter];
		},
		async refreshLogs() {
			const store = useHealthStore();
			await store.fetchLogFiles();
			if (store.logFileName) {
				await store.readLogFile(store.logFileName, this.requestLevels());
			} else {
				await store.readLogFile("");
			}
		},
		onFileChange(event: Event) {
			const name = (event.target as HTMLSelectElement).value;
			void useHealthStore().readLogFile(name, this.requestLevels());
		},
		onLevelChange(event: Event) {
			this.levelFilter = (event.target as HTMLSelectElement).value as
				| LogLevel
				| "all";
			if (this.logFileName) {
				void useHealthStore().readLogFile(
					this.logFileName,
					this.requestLevels(),
				);
			}
		},
		formatFileSize(value: number): string {
			if (!Number.isFinite(value) || value < 1024) {
				return `${Math.max(0, Number(value) || 0)} B`;
			}
			const units = ["KB", "MB", "GB"];
			let size = value;
			let unit = "B";
			for (const candidate of units) {
				size /= 1024;
				unit = candidate;
				if (size < 1024 || candidate === units[units.length - 1]) {
					break;
				}
			}
			return `${size.toFixed(size >= 10 ? 0 : 1)} ${unit}`;
		},
	},
};
</script>

<style scoped>
.health-log-dialog {
	max-height: min(86vh, 900px);
	grid-template-rows: auto auto auto minmax(0, 1fr) auto auto;
}
.health-log-toolbar {
	display: flex;
	align-items: end;
	flex-wrap: wrap;
	gap: 8px;
}
.health-log-field {
	display: flex;
	min-width: 150px;
	flex: 0 1 220px;
	flex-direction: column;
	gap: 4px;
	font-size: 11px;
	font-weight: 600;
	color: var(--muted-foreground);
}
.health-log-search {
	flex: 1 1 240px;
}
.health-log-select,
.health-log-input {
	min-width: 0;
	height: 34px;
	padding: 0 9px;
	border: 1px solid var(--border);
	border-radius: calc(var(--radius) - 2px);
	background: var(--background);
	color: var(--foreground);
	font-size: 12px;
}
.health-log-select:focus-visible,
.health-log-input:focus-visible {
	outline: 2px solid var(--primary);
	outline-offset: 1px;
}
.health-log-refresh {
	flex: 0 0 auto;
}
.health-log-error,
.health-log-state {
	display: flex;
	align-items: center;
	justify-content: center;
	gap: 8px;
	min-height: 120px;
	padding: 20px;
	color: var(--muted-foreground);
	font-size: 13px;
	text-align: center;
}
.health-log-error {
	justify-content: flex-start;
	min-height: 0;
	padding: 10px 12px;
	border-radius: calc(var(--radius) - 2px);
	background: color-mix(in srgb, var(--danger) 12%, transparent);
	color: var(--danger);
}
.health-log-list {
	min-height: 0;
	max-height: min(54vh, 560px);
	overflow: auto;
	border: 1px solid var(--border);
	border-radius: calc(var(--radius) - 2px);
	background: color-mix(in srgb, var(--background) 80%, var(--muted));
	font-family: var(--font-mono, ui-monospace, SFMono-Regular, Menlo, monospace);
}
.health-log-entry {
	display: grid;
	grid-template-columns: 52px 62px minmax(130px, auto) minmax(0, 1fr);
	align-items: baseline;
	gap: 8px;
	min-width: max-content;
	padding: 6px 10px;
	border-bottom: 1px solid color-mix(in srgb, var(--border) 55%, transparent);
	font-size: 11px;
	line-height: 1.45;
}
.health-log-entry:last-child {
	border-bottom: 0;
}
.health-log-entry-line,
.health-log-entry-time {
	color: var(--muted-foreground);
	font-variant-numeric: tabular-nums;
}
.health-log-entry-level {
	font-weight: 700;
	text-transform: uppercase;
}
.health-log-entry--trace .health-log-entry-level,
.health-log-entry--debug .health-log-entry-level {
	color: var(--muted-foreground);
}
.health-log-entry--info .health-log-entry-level {
	color: var(--primary);
}
.health-log-entry--warn .health-log-entry-level {
	color: var(--warning);
}
.health-log-entry--error .health-log-entry-level {
	color: var(--danger);
}
.health-log-entry-message {
	white-space: pre-wrap;
	word-break: break-word;
	color: var(--foreground);
}
.health-log-truncated {
	margin: 0;
	color: var(--muted-foreground);
	font-size: 11px;
}
@media (max-width: 640px) {
	.health-log-dialog {
		padding: 16px;
	}
	.health-log-field {
		flex-basis: 100%;
	}
	.health-log-refresh {
		width: 100%;
	}
	.health-log-entry {
		grid-template-columns: 42px 54px minmax(0, 1fr);
	}
	.health-log-entry-time {
		display: none;
	}
}
</style>
