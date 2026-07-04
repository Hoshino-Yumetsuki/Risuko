<template>
  <div style="display: none"></div>
</template>

<script lang="ts">
import { commands } from "@/components/CommandManager/instance";
import { TASK_FILE_RE } from "@/store/batchQueue";

export default {
	name: "ipc-bridge",
	data() {
		return {
			unlisten: null as (() => void) | null,
			bindToken: 0,
		};
	},
	methods: {
		async bindIpcEvents() {
			const token = ++this.bindToken;
			const { listen } = await import("@tauri-apps/api/event");
			const unlisten = await listen("command", (event) => {
				const payload = event.payload as
					| { command?: string; args?: unknown[] }
					| string;
				if (typeof payload === "string") {
					commands.execute(payload);
				} else if (payload?.command) {
					commands.execute(payload.command, ...(payload.args || []));
				}
			});

			const unlistenFile = await listen("open-file", (event) => {
				const path = event.payload as string;
				if (path && TASK_FILE_RE.test(path)) {
					commands.execute("application:new-bt-task-with-file", { path });
				}
			});

			if (token !== this.bindToken) {
				unlisten();
				unlistenFile();
				return;
			}

			this.unlisten = () => {
				unlisten();
				unlistenFile();
			};
		},
		unbindIpcEvents() {
			if (this.unlisten) {
				this.unlisten();
				this.unlisten = null;
			}
		},
	},
	created() {
		this.bindIpcEvents().catch(() => {});
	},
	beforeUnmount() {
		this.bindToken += 1;
		this.unbindIpcEvents();
	},
};
</script>
