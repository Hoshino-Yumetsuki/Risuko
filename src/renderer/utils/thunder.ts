import { invoke } from "@tauri-apps/api/core";

export const decodeThunderLink = async (uri = ""): Promise<string> => {
	const decoded = await invoke<string | null>("decode_thunder_uri", {
		uri,
	}).catch(() => null);
	return decoded ?? uri;
};
