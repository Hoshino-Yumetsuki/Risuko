<template>
  <div class="theme-switcher">
    <div
      v-for="item in themeOptions"
      :key="item.value"
      :class="['theme-option', { active: currentValue === item.value }]"
      @click.prevent="() => handleChange(item.value)"
    >
      <div :class="['theme-preview', item.className]">
        <template v-if="item.value === 'light' || item.value === 'dark'">
          <div :class="['preview-shell', `preview-shell--${item.value}`]">
            <div class="preview-side">
              <i></i>
              <i></i>
              <i></i>
            </div>
            <div class="preview-main">
              <div class="preview-row preview-row--accent"></div>
              <div class="preview-row"></div>
              <div class="preview-row"></div>
            </div>
          </div>
        </template>
        <template v-else>
          <div class="preview-split">
            <div class="preview-shell preview-shell--light">
              <div class="preview-side">
                <i></i>
                <i></i>
              </div>
              <div class="preview-main">
                <div class="preview-row preview-row--accent"></div>
                <div class="preview-row"></div>
              </div>
            </div>
            <div class="preview-shell preview-shell--dark">
              <div class="preview-side">
                <i></i>
                <i></i>
              </div>
              <div class="preview-main">
                <div class="preview-row preview-row--accent"></div>
                <div class="preview-row"></div>
              </div>
            </div>
          </div>
        </template>
      </div>
      <span class="theme-label">{{ item.text }}</span>
    </div>
  </div>
</template>

<script lang="ts">
import { APP_THEME } from "@shared/constants";

export default {
	name: "theme-switcher",
	props: {
		modelValue: {
			type: String,
			default: null,
		},
	},
	emits: ["update:modelValue", "change"],
	data() {
		return {
			currentValue: this.modelValue ?? APP_THEME.AUTO,
		};
	},
	computed: {
		themeOptions() {
			return [
				{
					className: "preview-auto",
					value: APP_THEME.AUTO,
					text: this.$t("preferences.theme-auto"),
				},
				{
					className: "preview-light",
					value: APP_THEME.LIGHT,
					text: this.$t("preferences.theme-light"),
				},
				{
					className: "preview-dark",
					value: APP_THEME.DARK,
					text: this.$t("preferences.theme-dark"),
				},
			];
		},
	},
	watch: {
		modelValue(val) {
			if (val !== null && val !== this.currentValue) {
				this.currentValue = val;
			}
		},
		currentValue(val) {
			this.$emit("update:modelValue", val);
			this.$emit("change", val);
		},
	},
	methods: {
		handleChange(theme) {
			this.currentValue = theme;
		},
	},
};
</script>
