import { APP_THEME } from "@shared/constants";
import { getLanguage } from "@shared/locales";
import type { AppConfig } from "@shared/types/config";
import logger from "@shared/utils/logger";
import { listen } from "@tauri-apps/api/event";
import { createApp } from "vue";
import Flyout from "@/components/Flyout/Flyout.vue";
import { getLocaleManager } from "@/components/Locale";
import UiProgress from "@/components/ui/compat/UiProgress.vue";
import store from "@/store";
import { useAppStore } from "@/store/app";
import { usePreferenceStore } from "@/store/preference";
import { useTaskStore } from "@/store/task";
import { getSystemTheme } from "@/utils/native";

import "@/styles/app.css";
import "@/styles/flyout.css";

const POLL_INTERVAL = 1000;

function applyTheme() {
	const pref = usePreferenceStore();
	const appStore = useAppStore();
	const theme = pref.theme;
	const effective =
		theme === APP_THEME.AUTO ? appStore.systemTheme : theme || APP_THEME.LIGHT;
	const className = effective === APP_THEME.DARK ? "dark" : "";
	document.documentElement.className = className;
}

function startPolling() {
	const appStore = useAppStore();
	const taskStore = useTaskStore();
	let timer: number | null = null;
	let running = false;

	const scheduleNext = () => {
		if (running) {
			timer = window.setTimeout(loop, POLL_INTERVAL);
		}
	};

	const loop = async () => {
		if (!running || document.hidden) {
			scheduleNext();
			return;
		}
		try {
			await appStore.fetchGlobalStat();
			await taskStore.fetchList();
		} catch (err) {
			logger.warn("[Risuko] flyout poll failed:", (err as Error).message);
		}
		scheduleNext();
	};

	const tick = async () => {
		if (document.hidden) {
			return;
		}
		try {
			await appStore.fetchGlobalStat();
			await taskStore.fetchList();
		} catch (err) {
			logger.warn("[Risuko] flyout poll failed:", (err as Error).message);
		}
	};

	const start = () => {
		if (running) {
			return;
		}
		running = true;
		void tick();
		timer = window.setTimeout(loop, POLL_INTERVAL);
	};

	const stop = () => {
		running = false;
		if (timer === null) {
			return;
		}
		window.clearTimeout(timer);
		timer = null;
	};

	document.addEventListener("visibilitychange", () => {
		if (document.hidden) {
			stop();
		} else {
			start();
		}
	});

	if (!document.hidden) {
		start();
	}

	return { tick, start };
}

async function init(config: AppConfig) {
	const locale = getLanguage(config?.locale || "auto");
	const localeManager = getLocaleManager();
	await localeManager.changeLanguageByLocale(locale);
	const i18n = localeManager.getI18n();

	applyTheme();

	const app = createApp(Flyout);
	app.use(store);
	app.component("ui-progress", UiProgress);
	app.config.globalProperties.$t = (
		key: string,
		value?: Record<string, unknown>,
	) => i18n.t(key, value);
	app.mount("#app");

	const { tick } = startPolling();

	const media = window.matchMedia?.("(prefers-color-scheme: dark)");
	media?.addEventListener?.("change", () => {
		useAppStore().updateSystemTheme(getSystemTheme());
		applyTheme();
	});

	await listen("flyout:show", async () => {
		await usePreferenceStore().fetchPreference();
		applyTheme();
		void tick();
	});

	await listen<{ command?: string }>("command", (event) => {
		const command = event.payload?.command;
		if (
			command === "application:update-theme" ||
			command === "application:update-system-theme"
		) {
			void usePreferenceStore()
				.fetchPreference()
				.then(() => {
					useAppStore().updateSystemTheme(getSystemTheme());
					applyTheme();
				});
		}
	});
}

usePreferenceStore()
	.fetchPreference()
	.then((config) => {
		init(config);
	})
	.catch((err: unknown) => {
		logger.warn("[Risuko] flyout init failed:", err);
	});
