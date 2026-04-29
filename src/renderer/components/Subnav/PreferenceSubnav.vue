<template>
  <nav class="subnav-inner">
    <mo-enter tag="h3" preset="fadeInDown">{{ title }}</mo-enter>
    <LayoutGroup id="preference-subnav">
      <ul>
        <mo-enter
          tag="li"
          preset="fadeInLeft"
          :delay="0.09"
          @click="() => nav('basic')"
          :class="[current === 'basic' ? 'active' : '']"
        >
          <Motion
            v-if="current === 'basic'"
            layout-id="preference-subnav-pill"
            class="subnav-active-bg"
            :initial="false"
            :transition="pillTransition"
          />
          <i class="subnav-icon">
            <SlidersHorizontal :size="20" />
          </i>
          <span>{{ $t('preferences.basic') }}</span>
        </mo-enter>
        <mo-enter
          tag="li"
          preset="fadeInLeft"
          :delay="0.13"
          @click="() => nav('advanced')"
          :class="[current === 'advanced' ? 'active' : '']"
        >
          <Motion
            v-if="current === 'advanced'"
            layout-id="preference-subnav-pill"
            class="subnav-active-bg"
            :initial="false"
            :transition="pillTransition"
          />
          <i class="subnav-icon">
            <Wrench :size="20" />
          </i>
          <span>{{ $t('preferences.advanced') }}</span>
        </mo-enter>
        <mo-enter
          tag="li"
          preset="fadeInLeft"
          :delay="0.17"
          @click="() => nav('cloud-sinks')"
          :class="[current === 'cloud-sinks' ? 'active' : '']"
        >
          <Motion
            v-if="current === 'cloud-sinks'"
            layout-id="preference-subnav-pill"
            class="subnav-active-bg"
            :initial="false"
            :transition="pillTransition"
          />
          <i class="subnav-icon">
            <Cloud :size="20" />
          </i>
          <span>{{ $t('preferences.cloudSinks') }}</span>
        </mo-enter>
      </ul>
    </LayoutGroup>
  </nav>
</template>

<script lang="ts">
import logger from "@shared/utils/logger";
import { Cloud, SlidersHorizontal, Wrench } from "lucide-vue-next";
import { LayoutGroup, Motion } from "motion-v";

export default {
	name: "mo-preference-subnav",
	components: {
		Cloud,
		LayoutGroup,
		Motion,
		SlidersHorizontal,
		Wrench,
	},
	props: {
		current: {
			type: String,
			default: "basic",
		},
	},
	computed: {
		title() {
			return this.$t("subnav.preferences");
		},
		pillTransition() {
			return { type: "spring", stiffness: 380, damping: 32 };
		},
	},
	methods: {
		nav(category = "basic") {
			this.$router
				.push({
					path: `/preference/${category}`,
				})
				.catch((err) => {
					logger.log(err);
				});
		},
	},
};
</script>
