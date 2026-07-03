<template>
  <div class="app-info">
    <div class="app-info-logo">
      <img src="@static/logo.svg" width="40" height="40" alt="Risuko" />
    </div>
    <div class="app-info-name">Risuko</div>
    <span v-if="version" class="app-info-version">v{{ version }}</span>
    <div class="app-info-card" v-if="engine?.version">
      <div class="app-info-row">
        <span class="app-info-row-label">{{ $t('about.engine-version') }}</span>
        <span class="app-info-row-value">{{ engine.version }}</span>
      </div>
      <div
        v-if="!isMas() && engine?.enabledFeatures?.length"
        class="app-info-row app-info-row--stack"
      >
        <span class="app-info-row-label">{{ $t('about.features') }}</span>
        <div class="app-info-features">
          <span
            v-for="(feature, index) in engine.enabledFeatures"
            :key="`feature-${index}`"
            class="app-info-feature"
          >
            {{ feature }}
          </span>
        </div>
      </div>
    </div>
  </div>
</template>

<script lang="ts">
import is from "@/shims/platform";

export default {
	name: "app-info",
	props: {
		version: {
			type: String,
			default: "",
		},
		engine: {
			type: Object,
			default() {
				return {
					version: "",
					enabledFeatures: [],
				};
			},
		},
	},
	methods: {
		isMas: is.mas,
	},
};
</script>
