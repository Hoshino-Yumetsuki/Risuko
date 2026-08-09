<template>
	<ui-button
		v-bind="$attrs"
		variant="ghost"
		size="sm"
		class="select-directory"
		:title="$t('app.browse')"
		:aria-label="$t('app.browse')"
		@click.stop="onFolderClick"
	>
		<Folder :size="14" />
	</ui-button>
</template>

<script lang="ts">
import { Folder } from "@lucide/vue";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import UiButton from "@/components/ui/compat/UiButton.vue";
import is from "@/shims/platform";
import { safUriToFilesystemPath } from "@/utils/native";

export default {
	name: "select-directory",
	inheritAttrs: false,
	components: {
		UiButton,
		Folder,
	},
	methods: {
		async onFolderClick() {
			try {
				if (is.android()) {
					const granted = await invoke<boolean>(
						"ensure_android_storage_access",
					);
					if (!granted) {
						this.$msg.warning(this.$t("app.android-storage-access-required"));
						return;
					}
				}
				const selected = is.android()
					? await invoke<string | null>("select_android_directory")
					: await open({
							directory: true,
							multiple: false,
						});
				if (selected) {
					const path = is.android()
						? safUriToFilesystemPath(String(selected))
						: String(selected);
					this.$emit("selected", path);
				}
			} catch (err) {
				this.$msg.error(`${err}`);
			}
		},
	},
};
</script>
