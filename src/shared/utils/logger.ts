// biome-ignore-all lint/suspicious/noConsole: this is a logger utility
const isDev = process.env.NODE_ENV !== "production";

const logger = {
	log: (...args: unknown[]) => {
		if (isDev) {
			console.log(...args);
		}
	},
	debug: (...args: unknown[]) => {
		if (isDev) {
			console.debug(...args);
		}
	},
	info: (...args: unknown[]) => {
		if (isDev) {
			console.info(...args);
		}
	},
	warn: (...args: unknown[]) => {
		console.warn(...args);
	},
	error: (...args: unknown[]) => {
		console.error(...args);
	},
};

export default logger;
