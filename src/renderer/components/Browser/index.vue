<template>
  <div ref="webviewViewport" class="webview-viewport">
    <div v-if="isLoading" class="webview-loading">
      <div class="webview-spinner" />
    </div>
    <iframe class="webview-frame" ref="iframe" :src="src" @load="onLoad"></iframe>
  </div>
</template>

<script lang="ts">
export default {
	name: "embedded-browser",
	props: {
		src: {
			type: String,
			default: "",
		},
	},
	data() {
		return {
			isLoading: true,
		};
	},
	watch: {
		src() {
			this.isLoading = true;
		},
	},
	methods: {
		onLoad() {
			this.isLoading = false;
		},
	},
};
</script>
