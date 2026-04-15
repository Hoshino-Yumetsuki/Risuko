<template>
  <div style="display: none"></div>
</template>

<script lang="ts">
import { commands } from "@/components/CommandManager/instance";

export default {
	name: "mo-ipc",
	data() {
		return {
			unlisten: null as (() => void) | null,
			alive: true,
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
				if (path?.endsWith(".torrent")) {
					commands.execute("application:new-bt-task-with-file", { path });
				}
			});

			if (!this.alive || token !== this.bindToken) {
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
		this.bindIpcEvents().catch(() => {
			/* noop */
		});
	},
	beforeUnmount() {
		this.alive = false;
		this.bindToken += 1;
		this.unbindIpcEvents();
	},
};
</script>
