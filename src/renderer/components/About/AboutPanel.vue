<template>
  <Dialog :open="visible" @update:open="handleDialogOpenChange">
    <DialogContent class="app-about-dialog" :show-close-button="true">
      <app-info :version="version" :engine="engineInfo" />
      <copyright />
    </DialogContent>
  </Dialog>
</template>

<script lang="ts">
import AppInfo from "@/components/About/AppInfo.vue";
import Copyright from "@/components/About/Copyright.vue";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { useAppStore } from "@/store/app";

export default {
	name: "about-panel",
	components: {
		Dialog,
		DialogContent,
		[AppInfo.name]: AppInfo,
		[Copyright.name]: Copyright,
	},
	props: {
		visible: {
			type: Boolean,
			default: false,
		},
	},
	data() {
		return {
			version: "",
		};
	},
	async created() {
		try {
			const { getVersion } = await import("@tauri-apps/api/app");
			this.version = await getVersion();
		} catch {
			this.version = "";
		}
	},
	computed: {
		engineInfo() {
			return useAppStore().engineInfo;
		},
	},
	watch: {
		visible(val) {
			if (val) {
				this.handleOpen();
			}
		},
	},
	methods: {
		handleOpen() {
			useAppStore().fetchEngineInfo();
		},
		handleDialogOpenChange(open) {
			if (!open) {
				this.handleClose();
			}
		},
		handleClose() {
			useAppStore().hideAboutPanel();
		},
	},
};
</script>
