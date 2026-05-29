<template>
  <button type="button" class="show-in-folder" :aria-label="$t('task.show-in-folder')" @click.stop="onFolderClick">
    <FolderOpen :size="size" />
  </button>
</template>

<script lang="ts">
import { FolderOpen } from "@lucide/vue";
import { showItemInFolder } from "@/utils/native";

export default {
	name: "mo-show-in-folder",
	components: {
		FolderOpen,
	},
	props: {
		path: {
			type: String,
		},
		size: {
			type: Number,
			default: 10,
		},
	},
	computed: {},
	methods: {
		onFolderClick() {
			if (!this.path) {
				return;
			}
			showItemInFolder(this.path, {
				errorMsg: this.$t("task.file-not-exist"),
			});
		},
	},
};
</script>
