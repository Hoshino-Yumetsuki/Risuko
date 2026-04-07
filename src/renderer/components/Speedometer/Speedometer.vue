<template>
  <Transition name="speedometer-panel">
    <div v-if="isVisible" class="mo-speedometer">
      <div class="mode" @click="toggleEngineMode">
        <i>
          <Gauge :size="24" />
        </i>
        <em>{{ engineMode }}</em>
      </div>
      <Transition name="speedometer-value" mode="out-in">
        <div class="value" key="active">
          <em>
            <CloudUpload :size="14" />
            {{ formatBytes(stat.uploadSpeed) }}/s
          </em>
          <span>
            <CloudDownload :size="14" />
            {{ formatBytes(stat.downloadSpeed) }}/s
          </span>
        </div>
      </Transition>
    </div>
  </Transition>
</template>

<script lang="ts">
import { bytesToSize } from "@shared/utils";
import { CloudDownload, CloudUpload, Gauge } from "lucide-vue-next";
import { useAppStore } from "@/store/app";
import { usePreferenceStore } from "@/store/preference";

export default {
	name: "mo-speedometer",
	components: {
		Gauge,
		CloudUpload,
		CloudDownload,
	},
	computed: {
		stat() {
			return useAppStore().stat;
		},
		engineMode() {
			return usePreferenceStore().engineMode;
		},
		isStopped() {
			return this.stat.numActive === 0;
		},
		isTaskPage() {
			const path = this.$router.currentRoute.value?.path;
			return !path?.startsWith("/preference");
		},
		isVisible() {
			return this.isTaskPage && !this.isStopped;
		},
	},
	methods: {
		toggleEngineMode() {
			usePreferenceStore().toggleEngineMode();
		},
		formatBytes(value) {
			return bytesToSize(value);
		},
	},
};
</script>
