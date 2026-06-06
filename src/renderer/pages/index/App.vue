<template>
  <div id="app">
    <mo-title-bar v-if="showTitleBar" :showActions="showWindowActions" />
    <router-view />
    <mo-engine-client :secret="rpcSecret" />
    <mo-ipc v-if="isRenderer" />
    <mo-dynamic-tray v-if="enableDynamicTray" :show-speed="traySpeedometer" />
    <mo-cloudflare-dialog />
    <Teleport to="body">
      <Toaster
        position="top-center"
        :theme="themeClass === 'dark' ? 'dark' : 'light'"
        rich-colors
      />
    </Teleport>
  </div>
</template>

<script lang="ts">
import { APP_RUN_MODE, APP_THEME } from "@shared/constants";
import { getLanguage } from "@shared/locales";
import { parseBooleanConfig } from "@shared/utils";
import { invoke } from "@tauri-apps/api/core";
import { getLocaleManager } from "@/components/Locale";
import DynamicTray from "@/components/Native/DynamicTray.vue";
import EngineClient from "@/components/Native/EngineClient.vue";
import Ipc from "@/components/Native/Ipc.vue";
import TitleBar from "@/components/Native/TitleBar.vue";
import CloudflareDialog from "@/components/Task/CloudflareDialog.vue";
import { Toaster } from "@/components/ui/sonner";
import is from "@/shims/platform";
import { useAppStore } from "@/store/app";
import { usePreferenceStore } from "@/store/preference";

const FONT_FAMILY_OPTIONS = ["system", "rounded", "serif", "mono"];
const FONT_SIZE_OPTIONS = ["small", "default", "large", "extra-large"];

const normalizeOption = (
	value: unknown,
	options: string[],
	fallback: string,
) => {
	return typeof value === "string" && options.includes(value)
		? value
		: fallback;
};

export default {
	name: "risuko-app",
	components: {
		[DynamicTray.name]: DynamicTray,
		[EngineClient.name]: EngineClient,
		[Ipc.name]: Ipc,
		[TitleBar.name]: TitleBar,
		[CloudflareDialog.name as string]: CloudflareDialog,
		Toaster,
	},
	computed: {
		isMac: () => is.macOS(),
		isAndroid: () => is.android(),
		isRenderer: () => is.renderer(),
		systemTheme() {
			return useAppStore().systemTheme;
		},
		showWindowActions() {
			return is.windows() || is.linux();
		},
		showTitleBar() {
			return this.isRenderer && !this.isAndroid;
		},
		traySpeedometer() {
			return parseBooleanConfig(usePreferenceStore().config.traySpeedometer);
		},
		runMode() {
			return (
				Number(usePreferenceStore().config.runMode) || APP_RUN_MODE.STANDARD
			);
		},
		rpcSecret() {
			return usePreferenceStore().config.rpcSecret;
		},
		theme() {
			return usePreferenceStore().theme;
		},
		locale() {
			return usePreferenceStore().locale;
		},
		direction() {
			return usePreferenceStore().direction;
		},
		themeClass() {
			const effectiveTheme =
				this.theme === APP_THEME.AUTO ? this.systemTheme : this.theme;
			return effectiveTheme === APP_THEME.DARK ? "dark" : "";
		},
		i18nClass() {
			return `i18n-${getLanguage(this.locale)}`;
		},
		directionClass() {
			return `dir-${this.direction}`;
		},
		fontFamilyClass() {
			if (this.isAndroid) {
				return "";
			}
			const family = normalizeOption(
				usePreferenceStore().config.fontFamily,
				FONT_FAMILY_OPTIONS,
				"system",
			);
			return `font-family-${family}`;
		},
		fontSizeClass() {
			const size = normalizeOption(
				usePreferenceStore().config.fontSize,
				FONT_SIZE_OPTIONS,
				"default",
			);
			return `font-size-${size}`;
		},
		enableDynamicTray() {
			return (
				this.isMac &&
				this.isRenderer &&
				!this.isAndroid &&
				this.runMode !== APP_RUN_MODE.HIDE_TRAY
			);
		},
		platformClass() {
			return this.isAndroid
				? "platform-android mobile-phone"
				: "platform-desktop";
		},
	},
	methods: {
		updateRootClassName() {
			const {
				themeClass = "",
				i18nClass = "",
				directionClass = "",
				platformClass = "",
				fontFamilyClass = "",
				fontSizeClass = "",
			} = this;
			const className = `${themeClass} ${i18nClass} ${directionClass} ${platformClass} ${fontFamilyClass} ${fontSizeClass}`;
			document.documentElement.className = className;
			this.syncAndroidSystemBars();
		},
		syncAndroidSystemBars() {
			if (!this.isRenderer || !this.isAndroid) {
				return;
			}
			invoke("set_android_system_bars", {
				darkMode: this.themeClass === "dark",
			}).catch(() => undefined);
		},
	},
	beforeMount() {
		this.updateRootClassName();
	},
	watch: {
		locale(val, oldVal) {
			const lng = getLanguage(val);
			getLocaleManager().changeLanguage(lng);
			if (!oldVal || oldVal === val) {
				return;
			}
			// Force a full renderer refresh so all views pick up the new locale.
			window.setTimeout(() => {
				window.location.reload();
			}, 0);
		},
		themeClass() {
			this.updateRootClassName();
		},
		i18nClass() {
			this.updateRootClassName();
		},
		directionClass() {
			this.updateRootClassName();
		},
		platformClass() {
			this.updateRootClassName();
		},
		fontFamilyClass() {
			this.updateRootClassName();
		},
		fontSizeClass() {
			this.updateRootClassName();
		},
	},
};
</script>
