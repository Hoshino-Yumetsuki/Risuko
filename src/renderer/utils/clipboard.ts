import { invoke } from "@tauri-apps/api/core";

export async function copyText(text: string): Promise<void> {
	try {
		await invoke("mark_clipboard_self_write", { text });
	} catch {}
	try {
		const { writeText } = await import("@tauri-apps/plugin-clipboard-manager");
		await writeText(text);
	} catch {
		await navigator.clipboard.writeText(text);
	}
}
