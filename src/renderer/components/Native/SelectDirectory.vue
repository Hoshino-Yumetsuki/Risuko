<template>
	<ui-button
		v-bind="$attrs"
		variant="ghost"
		size="sm"
		class="select-directory"
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

// On Android, the system folder picker (ACTION_OPEN_DOCUMENT_TREE)
// returns a Storage Access Framework URI like
// `content://com.android.externalstorage.documents/tree/primary%3ADownload%2FRisuko`.
// Risuko's engine writes via aria2 which needs a filesystem path, so we
// translate the common external-storage tree URIs back to their
// `/storage/emulated/...` equivalent. Other SAF authorities (SD cards,
// USB drives, cloud providers) don't map cleanly and are returned as-is
// so the user can see what they picked.
function safUriToFilesystemPath(uri: string): string {
	if (typeof uri !== "string" || !uri.startsWith("content://")) {
		return uri;
	}
	const match = uri.match(
		/com\.android\.externalstorage\.documents\/(?:tree|document)\/([^?#/]+)/,
	);
	if (!match) {
		return uri;
	}
	const decoded = decodeURIComponent(match[1]);
	const sep = decoded.indexOf(":");
	if (sep < 0) {
		return uri;
	}
	const storage = decoded.slice(0, sep);
	const rel = decoded.slice(sep + 1);
	if (storage === "primary") {
		return rel ? `/storage/emulated/0/${rel}` : "/storage/emulated/0";
	}
	return rel ? `/storage/${storage}/${rel}` : `/storage/${storage}`;
}

export default {
	name: "mo-select-directory",
	inheritAttrs: false,
	components: {
		UiButton,
		Folder,
	},
	props: {},
	computed: {
		isAndroid() {
			return is.android();
		},
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
