// biome-ignore-all lint/suspicious/noConsole: this is a logger utility
const isDev = process.env.NODE_ENV !== "production";

let nativeLog: ((level: string, message: string) => void) | undefined;
async function toNativeLog(level: string, args: unknown[]) {
	try {
		if (!nativeLog) {
			const { invoke } = await import("@tauri-apps/api/core");
			nativeLog = (lvl: string, msg: string) =>
				invoke("log_frontend", { level: lvl, message: msg }).catch(() => {});
		}
		nativeLog(
			level,
			args
				.map((a) => (typeof a === "string" ? a : JSON.stringify(a)))
				.join(" "),
		);
	} catch {
		// ignore
	}
}

function log(
	level: "log" | "debug" | "info" | "warn" | "error",
	args: unknown[],
) {
	const consoleMethod = console[level] ?? console.log;
	consoleMethod(...args);
	if (!isDev) {
		toNativeLog(level, args);
	}
}

const logger = {
	log: (...args: unknown[]) => {
		log("log", args);
	},
	debug: (...args: unknown[]) => {
		log("debug", args);
	},
	info: (...args: unknown[]) => {
		log("info", args);
	},
	warn: (...args: unknown[]) => {
		log("warn", args);
	},
	error: (...args: unknown[]) => {
		log("error", args);
	},
};

export default logger;
