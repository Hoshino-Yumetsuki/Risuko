<template>
  <div class="main panel panel-layout panel-layout--v preference-page">
    <motion-enter tag="header" preset="fadeInDown" class="panel-header preference-header">
      <h4>{{ $t('subnav.preferences') }}</h4>
    </motion-enter>
    <nav v-if="isAndroid" class="settings-nav-mobile" :aria-label="$t('subnav.preferences')">
      <DropdownMenu>
        <DropdownMenuTrigger class="settings-nav-trigger" type="button">
          <component :is="currentTab.icon" :size="16" />
          <span class="settings-nav-trigger-label">{{ currentTab.title }}</span>
          <ChevronDown :size="16" class="settings-nav-chevron" />
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start">
          <DropdownMenuItem
            v-for="tab in tabs"
            :key="tab.key"
            @click="onSectionSelect(tab)"
          >
            <component :is="tab.icon" :size="14" />
            <span>{{ tab.title }}</span>
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </nav>
    <LayoutGroup v-else id="settings-tabs">
      <nav class="settings-tabs" :aria-label="$t('subnav.preferences')">
        <router-link
          v-for="tab in tabs"
          :key="tab.key"
          :to="tab.route"
          class="settings-tab"
          :class="{ active: current === tab.key }"
        >
          <Motion
            v-if="current === tab.key"
            layout-id="settings-tab-pill"
            class="settings-tab-pill"
            :initial="false"
            :transition="pillTransition"
          />
          <component :is="tab.icon" :size="14" class="settings-tab-icon" />
          <span class="settings-tab-label">{{ tab.title }}</span>
        </router-link>
      </nav>
    </LayoutGroup>
    <router-view v-slot="{ Component, route }">
      <Transition name="page" mode="out-in">
        <component :is="Component" :key="route.path" class="pref-form-view" />
      </Transition>
    </router-view>
  </div>
</template>

<script lang="ts">
import {
	ChevronDown,
	Cloud,
	Palette,
	Radio,
	RefreshCw,
	SlidersHorizontal,
	Wrench,
} from "@lucide/vue";
import { LayoutGroup, Motion } from "motion-v";
import {
	DropdownMenu,
	DropdownMenuContent,
	DropdownMenuItem,
	DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import is from "@/shims/platform";
import { usePreferenceStore } from "@/store/preference";

export default {
	name: "preference-page",
	components: {
		LayoutGroup,
		Motion,
		DropdownMenu,
		DropdownMenuContent,
		DropdownMenuItem,
		DropdownMenuTrigger,
		ChevronDown,
	},
	computed: {
		isAndroid() {
			return is.android();
		},
		tabs() {
			return [
				{
					key: "basic",
					title: this.$t("preferences.basic"),
					route: "/preference/basic",
					icon: SlidersHorizontal,
				},
				{
					key: "appearance",
					title: this.$t("preferences.appearance"),
					route: "/preference/appearance",
					icon: Palette,
				},
				{
					key: "advanced",
					title: this.$t("preferences.advanced"),
					route: "/preference/advanced",
					icon: Wrench,
				},
				{
					key: "usenet",
					title: this.$t("preferences.usenet"),
					route: "/preference/usenet",
					icon: Radio,
				},
				{
					key: "cloud-sinks",
					title: this.$t("preferences.cloudSinks"),
					route: "/preference/cloud-sinks",
					icon: Cloud,
				},
				{
					key: "sync",
					title: this.$t("subnav.sync"),
					route: "/preference/sync",
					icon: RefreshCw,
				},
			];
		},
		current() {
			return this.$route.path.split("/")[2] || "basic";
		},
		currentTab() {
			return this.tabs.find((tab) => tab.key === this.current) || this.tabs[0];
		},
		pillTransition() {
			return { duration: 0.16, ease: [0.25, 1, 0.5, 1] };
		},
	},
	created() {
		usePreferenceStore().fetchPreference();
	},
	methods: {
		onSectionSelect(tab) {
			this.$router.push({ path: tab.route }).catch(() => {});
		},
	},
};
</script>
