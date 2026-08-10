<template>
  <div class="content panel panel-layout panel-layout--v">
    <main class="panel-content">
      <div class="form-preference">
        <div class="settings-section" data-preference-search-target="preferences.appearance-theme preferences.theme-auto preferences.theme-light preferences.theme-dark">
          <div class="settings-section-header">
            <div class="section-icon"><Palette :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('preferences.appearance-theme') }}</h3>
            </div>
          </div>
          <div class="settings-section-content">
            <theme-switcher :model-value="theme" @change="onThemeChange" />
          </div>
        </div>

        <div class="settings-section" data-preference-search-target="preferences.appearance-typography preferences.font-family preferences.font-family-system preferences.font-family-rounded preferences.font-family-serif preferences.font-family-mono preferences.font-size preferences.font-size-small preferences.font-size-default preferences.font-size-large preferences.font-size-extra-large">
          <div class="settings-section-header">
            <div class="section-icon"><Type :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('preferences.appearance-typography') }}</h3>
            </div>
          </div>
          <div class="settings-section-content">
            <div class="typography-controls">
              <div v-if="!isAndroid" class="typography-row typography-row--font">
                <div class="typography-row-main">
                  <label class="settings-select-item-label">{{ $t('preferences.font-family') }}</label>
                  <Select :model-value="fontFamily" class="typography-font-select" @update:model-value="onFontFamilyChange">
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem v-for="item in fontFamilyOptions" :key="item.value" :value="item.value">
                        {{ item.label }}
                      </SelectItem>
                    </SelectContent>
                  </Select>
                </div>
                <div
                  class="typography-sample"
                  :class="`typography-sample--${fontFamily}`"
                >
                  {{ $t('preferences.font-family-sample') }}
                </div>
              </div>
              <div class="typography-row typography-row--size">
                <div class="typography-row-main">
                  <span class="settings-select-item-label">{{ $t('preferences.font-size') }}</span>
                </div>
                <div class="font-size-segmented" role="radiogroup" :aria-label="$t('preferences.font-size')">
                  <button
                    v-for="item in fontSizeOptions"
                    :key="item.value"
                    type="button"
                    role="radio"
                    class="font-size-segment"
                    :class="{ 'font-size-segment--active': fontSize === item.value }"
                    :aria-label="item.label"
                    :aria-checked="fontSize === item.value"
                    @click="onFontSizeChange(item.value)"
                  >
                    {{ item.shortLabel }}
                  </button>
                </div>
              </div>
            </div>
          </div>
        </div>

        <div class="settings-section" data-preference-search-target="preferences.task-list-style preferences.task-list-style-compact preferences.task-list-style-card">
          <div class="settings-section-header">
            <div class="section-icon"><ListTodo :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('preferences.task-list-style') }}</h3>
              <p class="section-desc">{{ $t('preferences.task-list-style-description') }}</p>
            </div>
          </div>
          <div class="settings-section-content">
            <div class="font-size-segmented task-style-segmented" role="radiogroup" :aria-label="$t('preferences.task-list-style')">
              <button
                v-for="item in taskListStyleOptions"
                :key="item.value"
                type="button"
                role="radio"
                class="font-size-segment"
                :class="{ 'font-size-segment--active': taskListStyle === item.value }"
                :aria-checked="taskListStyle === item.value"
                @click="onTaskListStyleChange(item.value)"
              >
                <component :is="item.icon" :size="13" />
                {{ item.label }}
              </button>
            </div>
          </div>
        </div>

        <div
          v-if="isRenderer && !isAndroid"
          class="settings-section"
          data-preference-search-target="preferences.appearance-window preferences.sidebar-collapsed preferences.hide-app-menu preferences.auto-hide-window preferences.tray-speedometer"
        >
          <div class="settings-section-header">
            <div class="section-icon"><AppWindow :size="16" /></div>
            <div class="section-title">
              <h3>{{ $t('preferences.appearance-window') }}</h3>
            </div>
          </div>
          <div class="settings-section-content">
            <div class="settings-row">
              <div class="settings-row-content">
                <span class="settings-row-title">{{ $t('preferences.sidebar-collapsed') }}</span>
                <div class="settings-row-description">
                  {{ $t('preferences.sidebar-collapsed-description') }}
                </div>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="sidebarCollapsed"
                  @change="(val) => onSidebarCollapsedChange(val)"
                />
              </div>
            </div>
            <div v-if="showHideAppMenuOption" class="settings-row">
              <div class="settings-row-content">
                <span class="settings-row-title">{{ $t('preferences.hide-app-menu') }}</span>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="!!config.hideAppMenu"
                  @change="(val) => onHideAppMenuChange(val)"
                />
              </div>
            </div>
            <div class="settings-row">
              <div class="settings-row-content">
                <span class="settings-row-title">{{ $t('preferences.auto-hide-window') }}</span>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="!!config.autoHideWindow"
                  @change="(val) => onAutoHideWindowChange(val)"
                />
              </div>
            </div>
            <div v-if="isMac" class="settings-row">
              <div class="settings-row-content">
                <span class="settings-row-title">{{ $t('preferences.tray-speedometer') }}</span>
              </div>
              <div class="settings-row-action">
                <ui-checkbox
                  :model-value="!!config.traySpeedometer"
                  @change="(val) => saveBoolean('traySpeedometer', val)"
                />
              </div>
            </div>
          </div>
        </div>
      </div>
    </main>
  </div>
</template>

<script lang="ts">
import {
	AppWindow,
	LayoutGrid,
	ListTodo,
	Palette,
	Rows3,
	Type,
} from "@lucide/vue";
import {
	DEFAULT_FONT_FAMILY,
	DEFAULT_FONT_SIZE,
	FONT_FAMILY_OPTIONS,
	FONT_SIZE_OPTIONS,
	normalizeConfigOption,
} from "@shared/types/config";
import { invoke } from "@tauri-apps/api/core";
import ThemeSwitcher from "@/components/Preference/ThemeSwitcher.vue";
import {
	Select,
	SelectContent,
	SelectItem,
	SelectTrigger,
	SelectValue,
} from "@/components/ui/select";
import is from "@/shims/platform";
import { usePreferenceStore } from "@/store/preference";

export default {
	name: "preference-appearance",
	components: {
		[ThemeSwitcher.name]: ThemeSwitcher,
		Select,
		SelectContent,
		SelectItem,
		SelectTrigger,
		SelectValue,
		AppWindow,
		ListTodo,
		Palette,
		Type,
	},
	computed: {
		isRenderer: () => is.renderer(),
		isMac: () => is.macOS(),
		isAndroid: () => is.android(),
		showHideAppMenuOption() {
			return is.windows() || is.linux();
		},
		config() {
			return usePreferenceStore().config;
		},
		theme() {
			return this.config.theme;
		},
		fontFamily() {
			return normalizeConfigOption(
				this.config.fontFamily,
				FONT_FAMILY_OPTIONS,
				DEFAULT_FONT_FAMILY,
			);
		},
		fontSize() {
			return normalizeConfigOption(
				this.config.fontSize,
				FONT_SIZE_OPTIONS,
				DEFAULT_FONT_SIZE,
			);
		},
		taskListStyle() {
			return usePreferenceStore().taskListStyle;
		},
		sidebarCollapsed() {
			return usePreferenceStore().sidebarCollapsed;
		},
		fontFamilyOptions() {
			return FONT_FAMILY_OPTIONS.map((value) => ({
				label: this.$t(`preferences.font-family-${value}`),
				value,
			}));
		},
		fontSizeOptions() {
			return FONT_SIZE_OPTIONS.map((value) => ({
				label: this.$t(`preferences.font-size-${value}`),
				shortLabel: this.$t(`preferences.font-size-${value}-short`),
				value,
			}));
		},
		taskListStyleOptions() {
			return [
				{
					value: "compact",
					label: this.$t("preferences.task-list-style-compact"),
					icon: Rows3,
				},
				{
					value: "card",
					label: this.$t("preferences.task-list-style-card"),
					icon: LayoutGrid,
				},
			];
		},
	},
	methods: {
		save(patch) {
			usePreferenceStore()
				.save(patch)
				.catch(() => {
					this.$msg.error(this.$t("preferences.save-fail-message"));
				});
		},
		saveBoolean(key, value) {
			this.save({ [key]: !!value });
		},
		onThemeChange(theme) {
			this.save({ theme });
		},
		onFontFamilyChange(value) {
			this.save({
				fontFamily: normalizeConfigOption(
					value,
					FONT_FAMILY_OPTIONS,
					DEFAULT_FONT_FAMILY,
				),
			});
		},
		onFontSizeChange(value) {
			this.save({
				fontSize: normalizeConfigOption(
					value,
					FONT_SIZE_OPTIONS,
					DEFAULT_FONT_SIZE,
				),
			});
		},
		onTaskListStyleChange(value) {
			usePreferenceStore().setTaskListStyle(value);
		},
		onSidebarCollapsedChange(value) {
			usePreferenceStore().setSidebarCollapsed(!!value);
		},
		onHideAppMenuChange(value) {
			this.saveBoolean("hideAppMenu", value);
			invoke("toggle_app_menu", { hidden: !!value }).catch(() => {});
		},
		onAutoHideWindowChange(value) {
			this.saveBoolean("autoHideWindow", value);
		},
	},
};
</script>
