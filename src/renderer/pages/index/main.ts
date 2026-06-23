import { getLanguage } from "@shared/locales";
import type { AppConfig } from "@shared/types/config";
import logger from "@shared/utils/logger";
import { invoke } from "@tauri-apps/api/core";
import axios from "axios";
import { createApp } from "vue";
import VueVirtualScroller from "vue-virtual-scroller";
import { commands } from "@/components/CommandManager/instance";
import { getLocaleManager } from "@/components/Locale";
import MoEnter from "@/components/Motion/MoEnter.vue";
import Msg from "@/components/Msg";
import UiCol from "@/components/ui/Col.vue";
import UiButton from "@/components/ui/compat/UiButton.vue";
import UiCheckbox from "@/components/ui/compat/UiCheckbox.vue";
import UiProgress from "@/components/ui/compat/UiProgress.vue";
import UiSwitch from "@/components/ui/compat/UiSwitch.vue";
import UiTooltip from "@/components/ui/compat/UiTooltip.vue";
import UiRow from "@/components/ui/Row.vue";
import router from "@/router";
import store from "@/store";
import { useAuthStore } from "@/store/auth";
import { usePreferenceStore } from "@/store/preference";
import { useSyncStore } from "@/store/sync";
import TrayWorker from "@/workers/tray.worker?worker";
import App from "./App.vue";
import "./commands";

import "@/styles/app.css";
import "vue-virtual-scroller/dist/vue-virtual-scroller.css";
import "vue-sonner/style.css";

const updateTray = (payload: {
	rgba?: Uint8Array;
	width?: number;
	height?: number;
}) => {
	const { rgba, width, height } = payload;
	if (!rgba) {
		return;
	}

	invoke("update_tray", { imageData: rgba, width, height }).catch((err) => {
		logger.warn("[Risuko] update_tray failed:", err);
	});
};

const updateTrayMenuLabels = (i18n: { t: (key: string) => string }) => {
	const labels = {
		"tray-new-task": i18n.t("task.new-task"),
		"tray-new-bt-task": i18n.t("task.new-bt-task"),
		"tray-open-file": i18n.t("task.open-file"),
		"tray-show": i18n.t("app.show"),
		"tray-quick-panel": i18n.t("app.quick-panel"),
		"tray-manual": i18n.t("help.manual"),
		"tray-check-updates": i18n.t("app.check-for-updates"),
		"tray-task-list": i18n.t("app.task-list"),
		"tray-preferences": i18n.t("app.preferences"),
		"tray-quit": i18n.t("app.quit"),
	};

	invoke("update_tray_menu_labels", { labels }).catch((err) => {
		logger.warn("[Risuko] update_tray_menu_labels failed:", err);
	});
};

const updateAppMenuLabels = (i18n: { t: (key: string) => string }) => {
	const labels = {
		"menu-app": i18n.t("menu.app"),
		"menu-file": i18n.t("menu.file"),
		"menu-task": i18n.t("menu.task"),
		"menu-edit": i18n.t("menu.edit"),
		"menu-window": i18n.t("menu.window"),
		"menu-help": i18n.t("menu.help"),
		about: i18n.t("app.about"),
		preferences: i18n.t("app.preferences"),
		"check-for-updates": i18n.t("app.check-for-updates"),
		"show-window": i18n.t("app.show"),
		quit: i18n.t("app.quit"),
		reload: i18n.t("window.reload"),
		front: i18n.t("window.front"),
		"new-task": i18n.t("task.new-task"),
		"new-bt-task": i18n.t("task.new-bt-task"),
		"open-file": i18n.t("task.open-file"),
		"task-list": i18n.t("app.task-list"),
		"pause-task": i18n.t("task.pause-task"),
		"resume-task": i18n.t("task.resume-task"),
		"delete-task": i18n.t("task.delete-task"),
		"move-task-up": i18n.t("task.move-task-up"),
		"move-task-down": i18n.t("task.move-task-down"),
		"pause-all-task": i18n.t("task.pause-all-task"),
		"resume-all-task": i18n.t("task.resume-all-task"),
		"select-all-task": i18n.t("task.select-all-task"),
		"clear-recent-tasks": i18n.t("task.clear-recent-tasks"),
		"official-website": i18n.t("help.official-website"),
		manual: i18n.t("help.manual"),
		"release-notes": i18n.t("help.release-notes"),
		"report-problem": i18n.t("help.report-problem"),
		"toggle-dev-tools": i18n.t("help.toggle-dev-tools"),
	};

	invoke("update_app_menu_labels", { labels }).catch((err) => {
		logger.warn("[Risuko] update_app_menu_labels failed:", err);
	});
};

function initTrayWorker() {
	const worker = new TrayWorker();

	worker.addEventListener("message", (event) => {
		const { type, payload } = event.data;

		switch (type) {
			case "initialized":
			case "log":
				logger.log("[Risuko] Log from Tray Worker: ", payload);
				break;
			case "tray:drawed":
				updateTray(payload);
				break;
			default:
				logger.warn(
					"[Risuko] Tray Worker unhandled message type:",
					type,
					payload,
				);
		}
	});

	return worker;
}

async function init(config: AppConfig) {
	const locale = getLanguage(config?.locale || "auto");
	const localeManager = getLocaleManager();
	await localeManager.changeLanguageByLocale(locale);
	const i18n = localeManager.getI18n();
	updateAppMenuLabels(i18n);
	updateTrayMenuLabels(i18n);

	const app = createApp(App);
	app.use(store);
	app.use(router);
	app.use(VueVirtualScroller);
	app.use(Msg, {
		showClose: true,
	});
	app.component("ui-row", UiRow);
	app.component("ui-col", UiCol);
	app.component("ui-progress", UiProgress);
	app.component("ui-tooltip", UiTooltip);
	app.component("ui-checkbox", UiCheckbox);
	app.component("ui-switch", UiSwitch);
	app.component("ui-button", UiButton);
	app.component("mo-enter", MoEnter);
	app.config.globalProperties.$http = axios;
	app.config.globalProperties.$t = (
		key: string,
		value?: Record<string, unknown>,
	) => i18n.t(key, value);

	router.isReady().then(async () => {
		window.__app = app.mount("#app") as unknown as RisukoApp;
		window.__app.commands = commands;
		window.__app.trayWorker = initTrayWorker();

		// Show the window after mount to avoid a white flash.
		// Skip showing if launched at login (auto-start).
		const isOpenedAtLogin = await invoke("is_opened_at_login").catch(
			() => false,
		);
		if (!isOpenedAtLogin) {
			const { getCurrentWebviewWindow } = await import(
				"@tauri-apps/api/webviewWindow"
			);
			getCurrentWebviewWindow()
				.show()
				.catch(() => {
					/* noop */
				});
		}

		// Initialize auth from saved config and run startup sync
		const authStore = useAuthStore();
		await authStore.initFromConfig();

		// Listen for deep link events
		const handleDeepLinkUrls = (urls: string[]) => {
			logger.info("[Risuko] deep-link urls received:", urls);
			for (const url of urls) {
				if (typeof url !== "string" || !url.startsWith("risuko://auth?")) {
					continue;
				}
				try {
					const parsed = new URL(url);
					const token = parsed.searchParams.get("token");
					if (token) {
						logger.info("[Risuko] deep-link token found, logging in");
						authStore.handleDeepLinkToken(token);
					} else {
						logger.warn("[Risuko] deep-link URL missing token");
					}
				} catch (err) {
					logger.warn("[Risuko] malformed deep-link URL:", url, err);
				}
			}
		};

		try {
			const { getCurrent, onOpenUrl } = await import(
				"@tauri-apps/plugin-deep-link"
			);
			const initialUrls = await getCurrent();
			if (initialUrls && initialUrls.length > 0) {
				logger.info("[Risuko] deep-link initial urls:", initialUrls);
				handleDeepLinkUrls(initialUrls);
			}
			await onOpenUrl((urls) => {
				handleDeepLinkUrls(urls);
			});
			logger.info("[Risuko] deep-link listener registered");
		} catch (err) {
			logger.warn("[Risuko] deep-link plugin not available:", err);
		}

		// Run startup sync if auto-sync is enabled
		const syncStore = useSyncStore();
		syncStore.syncOnStartup();
	});
}

usePreferenceStore()
	.fetchPreference()
	.then((config) => {
		logger.info("[Risuko] load preference:", config);
		init(config);
		// Defer the tracker auto-sync off the startup critical path. The
		// fetch+parse of multiple tracker source URLs lands ~15–25s after
		// launch and produces visible CPU spikes; running it during an idle
		// callback (or after a delay fallback) keeps the launch smooth
		const runAutoSyncTracker = () => {
			usePreferenceStore().autoSyncTracker();
		};
		const w = window as Window & {
			requestIdleCallback?: (
				cb: () => void,
				opts?: { timeout: number },
			) => void;
		};
		if (typeof w.requestIdleCallback === "function") {
			w.requestIdleCallback(runAutoSyncTracker, { timeout: 30_000 });
		} else {
			setTimeout(runAutoSyncTracker, 30_000);
		}
	})
	.catch((err: unknown) => {
		alert(err);
	});
