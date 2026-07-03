<template>
  <div class="content-topbar">
    <div
      class="content-topbar-dragger"
      data-tauri-drag-region
      @mousedown.left.prevent="handleStartDragging"
    ></div>
    <ul v-if="showActions" class="window-controls">
      <li @click="handleMinimize">
        <Minus :size="12" />
      </li>
      <li @click="handleMaximize">
        <Maximize2 :size="12" />
      </li>
      <li @click="handleClose" class="win-close-btn">
        <X :size="12" />
      </li>
    </ul>
  </div>
</template>

<script lang="ts">
import { Maximize2, Minus, X } from "@lucide/vue";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

const appWindow = getCurrentWebviewWindow();

export default {
	name: "title-bar",
	components: {
		Minus,
		Maximize2,
		X,
	},
	props: {
		showActions: {
			type: Boolean,
		},
	},
	methods: {
		handleStartDragging() {
			appWindow.startDragging().catch(() => {});
		},
		handleMinimize() {
			appWindow.minimize();
		},
		async handleMaximize() {
			const maximized = await appWindow.isMaximized();
			if (maximized) {
				appWindow.unmaximize();
			} else {
				appWindow.maximize();
			}
		},
		handleClose() {
			appWindow.hide();
		},
	},
};
</script>
