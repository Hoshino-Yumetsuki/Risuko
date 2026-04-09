<template>
  <div class="mo-task-activity" v-if="task">
    <div class="activity-speed-trend">
      <div class="activity-speed-trend-header">
        <span class="activity-speed-trend-title">
          {{ $t('task.task-download-speed') }}
          <template v-if="isBT"> / {{ $t('task.task-upload-speed') }}</template>
        </span>
        <div class="activity-speed-trend-legend">
          <span class="activity-speed-legend-item">
            <i class="speed-dot speed-dot-download"></i>
            {{ formatBytes(displayDownloadSpeed) }}/s
          </span>
          <span class="activity-speed-legend-item" v-if="isBT">
            <i class="speed-dot speed-dot-upload"></i>
            {{ formatBytes(displayUploadSpeed) }}/s
          </span>
        </div>
      </div>
      <div class="activity-speed-trend-chart">
        <div class="activity-speed-chart-main">
          <svg
            class="activity-speed-svg"
            viewBox="0 0 100 48"
            preserveAspectRatio="none"
            role="img"
            :aria-label="$t('task.task-speed-trend-aria-label')"
          >
            <line class="activity-speed-grid" x1="0" y1="4" x2="100" y2="4" />
            <line class="activity-speed-grid" x1="0" y1="24" x2="100" y2="24" />
            <line class="activity-speed-grid" x1="0" y1="44" x2="100" y2="44" />
            <line
              v-for="x in verticalGridLines"
              :key="`grid-x-${x}`"
              class="activity-speed-grid axis-x"
              :x1="x"
              y1="4"
              :x2="x"
              y2="44"
            />
            <path
              v-if="downloadSpeedPath"
              class="activity-speed-line download"
              :d="downloadSpeedPath"
            />
            <path
              v-if="isBT && uploadSpeedPath"
              class="activity-speed-line upload"
              :d="uploadSpeedPath"
            />
          </svg>
          <div class="activity-speed-time-axis">
            <span>{{ timeAxisStartLabel }}</span>
            <span class="activity-speed-axis-title">{{ $t('task.task-speed-time-axis') }}</span>
            <span>{{ timeAxisEndLabel }}</span>
          </div>
        </div>
        <div class="activity-speed-scale">
          <span class="activity-speed-axis-title">{{ $t('task.task-speed-axis') }}</span>
          <span>{{ formatBytes(speedHistoryMax) }}/s</span>
          <span>{{ formatBytes(Math.round(speedHistoryMax / 2)) }}/s</span>
          <span>0 B/s</span>
        </div>
      </div>
    </div>

    <!-- Piece graphic (BT or split HTTP) -->
    <div class="graphic-box" ref="graphicBox" v-if="isBT || hasChunkProgress">
      <mo-task-graphic
        :outerWidth="graphicWidth"
        :bitfield="task.bitfield"
        :cellCount="displayGraphicCellCount"
        :cellPercents="displaySplitPercents"
        v-if="graphicWidth > 0"
      />
    </div>

    <!-- Progress section -->
    <div class="activity-progress">
      <div class="activity-progress-header">
        <span class="activity-progress-percent">{{ percent }}</span>
        <span class="activity-progress-size">
          {{ formatBytes(task.completedLength, 2) }}
          <span v-if="task.totalLength > 0"> / {{ formatBytes(task.totalLength, 2) }}</span>
        </span>
        <span class="activity-progress-remaining" v-if="isActive && remaining > 0">
          {{ remainingText }}
        </span>
      </div>
      <mo-task-progress
        :completed="Number(task.completedLength)"
        :total="Number(task.totalLength)"
        :status="taskStatus"
      />
      <div class="split-progress-list" v-if="displaySplitProgressList.length > 0">
        <div
          class="split-progress-item"
          v-for="item in displaySplitProgressList"
          :key="`split-${item.index}`"
        >
          <span class="split-progress-label">{{ hasChunkProgress && !isBT ? 'T' : 'S' }}{{ item.index + 1 }}</span>
          <div class="split-progress-track">
            <div class="split-progress-fill" :style="{ width: `${item.percent}%` }"></div>
          </div>
          <span class="split-progress-value">{{ item.percentText }}</span>
        </div>
      </div>
    </div>

    <!-- Stats grid -->
    <div class="activity-stats">
      <div class="activity-stat-item" v-if="showDownloadSpeed">
        <span class="activity-stat-label">{{ $t('task.task-download-speed') }}</span>
        <span class="activity-stat-value">{{ formatBytes(displayDownloadSpeed) }}/s</span>
      </div>
      <div class="activity-stat-item" v-if="isBT">
        <span class="activity-stat-label">{{ $t('task.task-upload-speed') }}</span>
        <span class="activity-stat-value">{{ formatBytes(displayUploadSpeed) }}/s</span>
      </div>
      <div class="activity-stat-item">
        <span class="activity-stat-label">{{ $t('task.task-connections') }}</span>
        <span class="activity-stat-value">{{ task.connections }}</span>
      </div>
      <div class="activity-stat-item" v-if="isBT">
        <span class="activity-stat-label">{{ $t('task.task-num-seeders') }}</span>
        <span class="activity-stat-value">{{ task.numSeeders }}</span>
      </div>
      <div class="activity-stat-item" v-if="isBT">
        <span class="activity-stat-label">{{ $t('task.task-upload-length') }}</span>
        <span class="activity-stat-value">{{ formatBytes(task.uploadLength) }}</span>
      </div>
      <div class="activity-stat-item" v-if="isBT">
        <span class="activity-stat-label">{{ $t('task.task-ratio') }}</span>
        <span class="activity-stat-value">{{ ratio }}</span>
      </div>
    </div>
  </div>
</template>

<script lang="ts">
import { TASK_STATUS } from "@shared/constants";
import {
	bytesToSize,
	calcProgress,
	calcRatio,
	checkTaskIsBT,
	checkTaskIsSeeder,
	timeFormat,
	timeRemaining,
} from "@shared/utils";
import TaskProgress from "@/components/Task/TaskProgress.vue";
import TaskGraphic from "@/components/TaskGraphic/Index.vue";
import is from "@/shims/platform";
import { usePreferenceStore } from "@/store/preference";
import {
	deleteSpeedHistory,
	getSpeedHistory,
	SPEED_HISTORY_LIMIT,
	useTaskStore,
} from "@/store/task";

const DEFAULT_SPLIT_SEGMENTS = 32;
const MAX_SPLIT_SEGMENTS = 128;
const SPEED_VERTICAL_GRID_COUNT = 6;

export default {
	name: "mo-task-activity",
	components: {
		[TaskGraphic.name]: TaskGraphic,
		[TaskProgress.name]: TaskProgress,
	},
	props: {
		gid: {
			type: String,
		},
		task: {
			type: Object,
		},
		files: {
			type: Array,
			default() {
				return [];
			},
		},
		peers: {
			type: Array,
			default() {
				return [];
			},
		},
		visible: {
			type: Boolean,
			default: false,
		},
	},
	data() {
		return {
			graphicWidth: 0,
		};
	},
	computed: {
		speedHistory() {
			// Depend on store's reactive rev counter to recompute when samples change
			void useTaskStore().speedHistoryRev;
			const gid = this.task?.gid;
			if (!gid) {
				return [];
			}
			return getSpeedHistory(gid);
		},
		isRenderer: () => is.renderer(),
		isBT() {
			return checkTaskIsBT(this.task);
		},
		isSeeder() {
			return checkTaskIsSeeder(this.task);
		},
		displayUploadSpeed() {
			return Number(this.task?.uploadSpeed || 0);
		},
		displayDownloadSpeed() {
			if (this.isSeeder) {
				return 0;
			}
			return Number(this.task?.downloadSpeed || 0);
		},
		showDownloadSpeed() {
			return !this.isSeeder;
		},
		taskStatus() {
			const { task, isSeeder } = this;
			if (isSeeder) {
				return TASK_STATUS.SEEDING;
			} else {
				return task.status;
			}
		},
		isActive() {
			return this.taskStatus === TASK_STATUS.ACTIVE;
		},
		percent() {
			const { totalLength, completedLength } = this.task;
			const percent = calcProgress(totalLength, completedLength);
			return `${percent}%`;
		},
		remaining() {
			const { totalLength, completedLength } = this.task;
			return timeRemaining(
				totalLength,
				completedLength,
				this.displayDownloadSpeed,
			);
		},
		remainingText() {
			return timeFormat(this.remaining, {
				prefix: this.$t("task.remaining-prefix"),
				i18n: {
					gt1d: this.$t("app.gt1d"),
					hour: this.$t("app.hour"),
					minute: this.$t("app.minute"),
					second: this.$t("app.second"),
				},
			});
		},
		ratio() {
			if (!this.isBT) {
				return 0;
			}

			const { totalLength, uploadLength } = this.task;
			const ratio = calcRatio(totalLength, uploadLength);
			return ratio;
		},
		graphicCellCount() {
			const task = this.task || {};
			const preferenceSplit = Number(usePreferenceStore().config?.split || 0);
			const taskSplit = Number(
				task.split || task.options?.split || task.option?.split || 0,
			);
			const picked =
				Number.isFinite(preferenceSplit) && preferenceSplit > 0
					? preferenceSplit
					: taskSplit;
			if (Number.isFinite(picked) && picked > 0) {
				return Math.max(1, Math.min(Math.trunc(picked), MAX_SPLIT_SEGMENTS));
			}
			return DEFAULT_SPLIT_SEGMENTS;
		},
		splitProgressList() {
			const segmentCount = this.graphicCellCount;
			if (!Number.isFinite(segmentCount) || segmentCount <= 0) {
				return [];
			}

			const totalLength = Number(this.task?.totalLength || 0);
			const completedLength = Number(this.task?.completedLength || 0);
			const isFullyCompleted =
				totalLength > 0 && completedLength >= totalLength;
			if (isFullyCompleted) {
				return new Array(segmentCount).fill(0).map((_, index) => ({
					index,
					percent: 100,
					percentText: "100%",
				}));
			}

			const normalizedBitfield = `${this.task?.bitfield || ""}`.replace(
				/[^0-9a-fA-F]/g,
				"",
			);
			const rawBitLen = normalizedBitfield.length * 4;
			const numPieces = Number(this.task?.numPieces || 0);
			const validBitLen =
				Number.isFinite(numPieces) && numPieces > 0
					? Math.min(Math.trunc(numPieces), rawBitLen)
					: rawBitLen;
			const sums = new Array(segmentCount).fill(0);
			const counts = new Array(segmentCount).fill(0);

			for (let bitIndex = 0; bitIndex < validBitLen; bitIndex++) {
				const nibbleIndex = Math.trunc(bitIndex / 4);
				const bitInNibble = 3 - (bitIndex % 4);
				const nibble = parseInt(normalizedBitfield[nibbleIndex] || "0", 16);
				const bit = Number.isNaN(nibble) ? 0 : (nibble >> bitInNibble) & 1;
				const bucket = Math.min(
					Math.floor((bitIndex * segmentCount) / validBitLen),
					segmentCount - 1,
				);
				sums[bucket] += bit;
				counts[bucket] += 1;
			}

			return sums.map((sum, index) => {
				const count = counts[index] || 1;
				const percent = validBitLen > 0 ? Math.round((sum / count) * 100) : 0;
				return {
					index,
					percent,
					percentText: `${percent}%`,
				};
			});
		},
		splitProgressPercents() {
			return this.splitProgressList.map((item) => item.percent);
		},
		hasChunkProgress() {
			return (
				Array.isArray(this.task?.chunkProgress) &&
				this.task.chunkProgress.length > 1
			);
		},
		httpChunkProgressList() {
			if (!this.hasChunkProgress) {
				return [];
			}
			return this.task.chunkProgress.map((chunk, index) => {
				const total = Number(chunk.totalLength || 0);
				const completed = Number(chunk.completedLength || 0);
				const percent = calcProgress(total, completed);
				return {
					index,
					percent,
					percentText: `${percent}%`,
				};
			});
		},
		displaySplitProgressList() {
			if (this.isBT) {
				return this.splitProgressList;
			}
			return this.httpChunkProgressList;
		},
		displaySplitPercents() {
			return this.displaySplitProgressList.map((item) => item.percent);
		},
		displayGraphicCellCount() {
			if (this.isBT) {
				return this.graphicCellCount;
			}
			return this.httpChunkProgressList.length;
		},
		speedHistoryMax() {
			const maxSpeed = this.speedHistory.reduce((max, sample) => {
				const downloadSpeed = Number(sample.download || 0);
				const uploadSpeed = this.isBT ? Number(sample.upload || 0) : 0;
				return Math.max(max, downloadSpeed, uploadSpeed);
			}, 0);
			return Math.max(1, maxSpeed);
		},
		downloadSpeedPath() {
			return this.buildSpeedPath("download");
		},
		uploadSpeedPath() {
			return this.buildSpeedPath("upload");
		},
		verticalGridLines() {
			const interval = 100 / SPEED_VERTICAL_GRID_COUNT;
			return new Array(SPEED_VERTICAL_GRID_COUNT + 1)
				.fill(0)
				.map((_, index) => Number((index * interval).toFixed(2)));
		},
		timeAxisStartLabel() {
			return this.$t("task.task-speed-time-ago", {
				seconds: SPEED_HISTORY_LIMIT,
			});
		},
		timeAxisEndLabel() {
			return this.$t("task.task-speed-now");
		},
	},
	mounted() {
		this.$nextTick(() => {
			this.updateGraphicWidth();
		});
	},
	methods: {
		resetSpeedHistory() {
			const gid = this.task?.gid;
			if (gid) {
				deleteSpeedHistory(gid);
				useTaskStore().speedHistoryRev++;
			}
		},
		buildSpeedPath(type) {
			if (!Array.isArray(this.speedHistory) || this.speedHistory.length < 2) {
				return "";
			}
			const maxSpeed = this.speedHistoryMax;
			const historyLength = this.speedHistory.length;
			const limitDenominator = Math.max(SPEED_HISTORY_LIMIT - 1, 1);
			const startOffset = Math.max(SPEED_HISTORY_LIMIT - historyLength, 0);
			const minY = 4;
			const maxY = 44;
			const ySpan = maxY - minY;
			return this.speedHistory
				.map((sample, index) => {
					const value = Math.max(0, Number(sample[type] || 0));
					const ratio = Math.min(1, value / maxSpeed);
					const x = ((startOffset + index) / limitDenominator) * 100;
					const y = maxY - ratio * ySpan;
					const point = `${x.toFixed(2)} ${y.toFixed(2)}`;
					return index === 0 ? `M ${point}` : `L ${point}`;
				})
				.join(" ");
		},
		updateGraphicWidth() {
			if (!this.$refs.graphicBox) {
				return;
			}
			this.graphicWidth = this.calcInnerWidth(this.$refs.graphicBox);
		},
		calcInnerWidth(ele) {
			if (!ele) {
				return 0;
			}

			const style = getComputedStyle(ele, null);
			const width = parseInt(style.width, 10);
			const paddingLeft = parseInt(style.paddingLeft, 10);
			const paddingRight = parseInt(style.paddingRight, 10);
			return width - paddingLeft - paddingRight;
		},
		formatBytes(value, precision) {
			return bytesToSize(value, precision);
		},
	},
};
</script>
