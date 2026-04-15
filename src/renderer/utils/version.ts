import { getVersion } from "@tauri-apps/api/app";

export const getRisukoVersion = async (): Promise<string> => {
	try {
		const version = await getVersion();
		return version ? `v${version}` : "";
	} catch {
		return "";
	}
};
