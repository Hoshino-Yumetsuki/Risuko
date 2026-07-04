<template>
  <div class="task-general" v-if="task">
    <div class="activity-stats general-stats">
      <div class="activity-stat-item" v-if="!isSeeder">
        <span class="activity-stat-label">{{ $t('task.task-download-speed') }}</span>
        <span class="activity-stat-value">{{ formatBytes(displayDownloadSpeed) }}/s</span>
      </div>
      <div class="activity-stat-item" v-if="isBT">
        <span class="activity-stat-label">{{ $t('task.task-upload-speed') }}</span>
        <span class="activity-stat-value">{{ formatBytes(Number(task.uploadSpeed || 0)) }}/s</span>
      </div>
      <div class="activity-stat-item">
        <span class="activity-stat-label">{{ $t('task.remaining-prefix') }}</span>
        <span class="activity-stat-value">{{ remainingText }}</span>
      </div>
      <div class="activity-stat-item">
        <span class="activity-stat-label">{{ $t('task.task-connections') }}</span>
        <span class="activity-stat-value">{{ task.connections }}</span>
      </div>
      <div class="activity-stat-item" v-if="isBT">
        <span class="activity-stat-label">{{ $t('task.task-num-seeders') }}</span>
        <span class="activity-stat-value">{{ task.numSeeders }}</span>
      </div>
      <div class="activity-stat-item">
        <span class="activity-stat-label">{{ $t('task.task-file-size') }}</span>
        <span class="activity-stat-value">
          {{ formatBytes(task.completedLength) }} / {{ formatBytes(task.totalLength) }}
        </span>
      </div>
    </div>
    <div class="general-card">
      <div class="general-card-row">
        <span class="general-card-label">{{ $t('task.task-gid') }}</span>
        <span class="general-card-value mono">{{ task.gid }}</span>
      </div>
      <div class="general-card-row">
        <span class="general-card-label">{{ $t('task.task-name') }}</span>
        <span class="general-card-value">{{ taskFullName }}</span>
      </div>
      <div class="general-card-row">
        <span class="general-card-label">{{ $t('task.task-dir') }}</span>
        <span class="general-card-value general-card-value--dir">
          <span class="dir-text">{{ path }}</span>
          <show-in-folder v-if="isRenderer" :path="revealPath" />
        </span>
      </div>
      <div class="general-card-row">
        <span class="general-card-label">{{ $t('task.task-status') }}</span>
        <span class="general-card-value"
          ><task-status :theme="currentTheme" :status="taskStatus"
        /></span>
      </div>
      <div class="general-card-row" v-if="task.errorCode && task.errorCode !== '0'">
        <span class="general-card-label">{{ $t('task.task-error-info') }}</span>
        <span class="general-card-value general-card-value--error"
          >{{ task.errorCode }} {{ task.errorMessage }}</span
        >
      </div>
    </div>
    <div v-if="isBT" class="general-section-header">
      <Magnet :size="14" /><span>{{ $t('task.task-bittorrent-info') }}</span>
    </div>
    <div v-if="isBT" class="general-card">
      <div class="general-card-row">
        <span class="general-card-label">{{ $t('task.task-info-hash') }}</span>
        <span class="general-card-value mono"
          >{{ task.infoHash }} <i class="copy-link" @click="handleCopyClick"><Link :size="12" /></i
        ></span>
      </div>
      <div v-if="task.infoHashV2" class="general-card-row">
        <span class="general-card-label">{{ $t('task.task-info-hash-v2') }}</span>
        <span class="general-card-value mono">{{ task.infoHashV2 }}</span>
      </div>
      <div v-if="task.metaVersion" class="general-card-row">
        <span class="general-card-label">{{ $t('task.task-meta-version') }}</span>
        <span class="general-card-value">{{ task.metaVersion }}</span>
      </div>
      <div class="general-card-row">
        <span class="general-card-label">{{ $t('task.task-piece-length') }}</span>
        <span class="general-card-value">{{ pieceLengthText }}</span>
      </div>
      <div class="general-card-row">
        <span class="general-card-label">{{ $t('task.task-num-pieces') }}</span>
        <span class="general-card-value">{{ task.numPieces }}</span>
      </div>
      <div class="general-card-row">
        <span class="general-card-label">{{ $t('task.task-bittorrent-creation-date') }}</span>
        <span class="general-card-value">{{ creationDateText }}</span>
      </div>
      <div class="general-card-row">
        <span class="general-card-label">{{ $t('task.task-bittorrent-comment') }}</span>
        <span class="general-card-value">{{ task.bittorrent.comment }}</span>
      </div>
    </div>
  </div>
</template>
<script lang="ts">
import { Link, Magnet } from "@lucide/vue";
import { APP_THEME, TASK_STATUS } from "@shared/constants";
import {
	bytesToSize,
	checkTaskIsBT,
	checkTaskIsSeeder,
	getTaskName,
	getTaskUri,
	localeDateTimeFormat,
	timeFormat,
	timeRemaining,
} from "@shared/utils";
import ShowInFolder from "@/components/Native/ShowInFolder.vue";
import TaskStatus from "@/components/Task/TaskStatus.vue";
import is from "@/shims/platform";
import { useAppStore } from "@/store/app";
import { usePreferenceStore } from "@/store/preference";
import { copyText } from "@/utils/clipboard";
import { getTaskRevealDir, getTaskRevealPath } from "@/utils/native";

export default {
	name: "task-general",
	components: {
		[ShowInFolder.name]: ShowInFolder,
		[TaskStatus.name]: TaskStatus,
		Magnet,
		Link,
	},
	props: {
		task: {
			type: Object,
		},
	},
	computed: {
		isRenderer: () => is.renderer(),
		systemTheme() {
			return useAppStore().systemTheme;
		},
		theme() {
			return usePreferenceStore().config.theme;
		},
		currentTheme() {
			return this.theme === APP_THEME.AUTO ? this.systemTheme : this.theme;
		},
		taskFullName() {
			return getTaskName(this.task, {
				defaultName: this.$t("task.get-task-name"),
				maxLen: -1,
			});
		},
		isSeeder() {
			return checkTaskIsSeeder(this.task);
		},
		taskStatus() {
			return this.isSeeder ? TASK_STATUS.SEEDING : this.task.status;
		},
		path() {
			return getTaskRevealPath(this.task);
		},
		revealPath() {
			return is.android() ? getTaskRevealDir(this.task) : this.path;
		},
		isBT() {
			return checkTaskIsBT(this.task);
		},
		displayDownloadSpeed() {
			return Number(this.task?.downloadSpeed || 0);
		},
		remaining() {
			return timeRemaining(
				this.task?.totalLength,
				this.task?.completedLength,
				this.displayDownloadSpeed,
			);
		},
		remainingText() {
			if (this.task?.status !== TASK_STATUS.ACTIVE || this.remaining <= 0) {
				return "—";
			}
			return timeFormat(this.remaining, {
				prefix: "",
				i18n: {
					gt1d: this.$t("app.gt1d"),
					hour: this.$t("app.hour"),
					minute: this.$t("app.minute"),
					second: this.$t("app.second"),
				},
			});
		},
		pieceLengthText() {
			return bytesToSize(this.task?.pieceLength);
		},
		creationDateText() {
			const locale = usePreferenceStore().config?.locale || "en-US";
			const creationDate = this.task?.bittorrent?.creationDate;
			return localeDateTimeFormat(creationDate, locale);
		},
	},
	methods: {
		formatBytes: bytesToSize,
		handleCopyClick() {
			const uri = getTaskUri(this.task);
			copyText(uri).then(() => {
				this.$msg.success(this.$t("task.copy-link-success"));
			});
		},
	},
};
</script>
