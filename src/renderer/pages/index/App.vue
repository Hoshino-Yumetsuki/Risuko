<template>
  <div id="app">
    <router-view />
    <engine-client :secret="rpcSecret" />
    <ipc-bridge v-if="isRenderer" />
    <dynamic-tray v-if="enableDynamicTray" :show-speed="traySpeedometer" />
    <cloudflare-dialog />
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
import {
	DEFAULT_FONT_FAMILY,
	DEFAULT_FONT_SIZE,
	FONT_FAMILY_OPTIONS,
	FONT_SIZE_OPTIONS,
	normalizeConfigOption,
} from "@shared/types/config";
import { parseBooleanConfig } from "@shared/utils";
import { invoke } from "@tauri-apps/api/core";
import { getLocaleManager } from "@/components/Locale";
import DynamicTray from "@/components/Native/DynamicTray.vue";
import EngineClient from "@/components/Native/EngineClient.vue";
import Ipc from "@/components/Native/Ipc.vue";
import CloudflareDialog from "@/components/Task/CloudflareDialog.vue";
import { Toaster } from "@/components/ui/sonner";
import is from "@/shims/platform";
import { useAppStore } from "@/store/app";
import { usePreferenceStore } from "@/store/preference";

export default {
	name: "risuko-app",
	components: {
		[DynamicTray.name]: DynamicTray,
		[EngineClient.name]: EngineClient,
		[Ipc.name]: Ipc,
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
			const family = normalizeConfigOption(
				usePreferenceStore().config.fontFamily,
				FONT_FAMILY_OPTIONS,
				DEFAULT_FONT_FAMILY,
			);
			return `font-family-${family}`;
		},
		fontSizeClass() {
			const size = normalizeConfigOption(
				usePreferenceStore().config.fontSize,
				FONT_SIZE_OPTIONS,
				DEFAULT_FONT_SIZE,
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
		rootClassName() {
			return `${this.themeClass} ${this.i18nClass} ${this.directionClass} ${this.platformClass} ${this.fontFamilyClass} ${this.fontSizeClass}`;
		},
	},
	methods: {
		updateRootClassName() {
			document.documentElement.className = this.rootClassName;
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
			window.setTimeout(() => {
				window.location.reload();
			}, 0);
		},
		rootClassName() {
			this.updateRootClassName();
		},
	},
};
</script>
