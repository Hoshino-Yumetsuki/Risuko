<template>
  <div class="content-topbar">
    <div
      class="content-topbar-dragger"
      data-tauri-drag-region
      @mousedown.left.prevent="handleStartDragging"
    ></div>
    <ul v-if="showActions" class="window-controls">
      <li>
        <button
          type="button"
          class="window-control"
          :title="$t('window.minimize')"
          :aria-label="$t('window.minimize')"
          @click="handleMinimize"
        >
          <Minus :size="12" aria-hidden="true" />
        </button>
      </li>
      <li>
        <button
          type="button"
          class="window-control"
          :title="$t('window.zoom')"
          :aria-label="$t('window.zoom')"
          @click="handleMaximize"
        >
          <Maximize2 :size="12" aria-hidden="true" />
        </button>
      </li>
      <li>
        <button
          type="button"
          class="window-control win-close-btn"
          :title="$t('window.close')"
          :aria-label="$t('window.close')"
          @click="handleClose"
        >
          <X :size="12" aria-hidden="true" />
        </button>
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
