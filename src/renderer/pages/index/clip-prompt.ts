import { APP_THEME } from "@shared/constants";
import { getLanguage } from "@shared/locales";
import type { AppConfig } from "@shared/types/config";
import logger from "@shared/utils/logger";
import { listen } from "@tauri-apps/api/event";
import { createApp } from "vue";
import ClipPrompt from "@/components/ClipboardWatcher/ClipPrompt.vue";
import { getLocaleManager } from "@/components/Locale";
import store from "@/store";
import { useAppStore } from "@/store/app";
import { usePreferenceStore } from "@/store/preference";
import { getSystemTheme } from "@/utils/native";

import "@/styles/app.css";

function applyTheme() {
	const pref = usePreferenceStore();
	const appStore = useAppStore();
	const theme = pref.theme;
	const effective =
		theme === APP_THEME.AUTO ? appStore.systemTheme : theme || APP_THEME.LIGHT;
	document.documentElement.className =
		effective === APP_THEME.DARK ? "dark" : "";
}

async function init(config: AppConfig) {
	const locale = getLanguage(config?.locale || "auto");
	const localeManager = getLocaleManager();
	await localeManager.changeLanguageByLocale(locale);
	const i18n = localeManager.getI18n();

	useAppStore().updateSystemTheme(getSystemTheme());
	applyTheme();

	const app = createApp(ClipPrompt);
	app.use(store);
	app.config.globalProperties.$t = (
		key: string,
		value?: Record<string, unknown>,
	) => i18n.t(key, value);
	app.mount("#app");

	const media = window.matchMedia?.("(prefers-color-scheme: dark)");
	media?.addEventListener?.("change", () => {
		useAppStore().updateSystemTheme(getSystemTheme());
		applyTheme();
	});

	// Re-sync language and theme each show, in case either changed while running
	await listen("clip-prompt:show", async () => {
		const cfg = await usePreferenceStore().fetchPreference();
		await localeManager.changeLanguageByLocale(
			getLanguage(cfg?.locale || "auto"),
		);
		useAppStore().updateSystemTheme(getSystemTheme());
		applyTheme();
	});
}

usePreferenceStore()
	.fetchPreference()
	.then((config) => init(config))
	.catch((err: unknown) => {
		logger.warn("[Risuko] clip-prompt init failed:", err);
	});
