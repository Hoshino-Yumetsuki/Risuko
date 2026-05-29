<template>
  <Select :model-value="currentRoute" @update:model-value="handleRoute">
    <SelectTrigger class="subnav-switch-trigger">
      <span class="subnav-switch-title">{{ title }}</span>
    </SelectTrigger>
    <SelectContent align="start">
      <SelectItem v-for="sn in subnavs" :key="sn.key" :value="sn.route">
        {{ sn.title }}
      </SelectItem>
    </SelectContent>
  </Select>
</template>

<script lang="ts">
import {
	Select,
	SelectContent,
	SelectItem,
	SelectTrigger,
} from "@/components/ui/select";

export default {
	name: "mo-subnav-switcher",
	components: {
		Select,
		SelectContent,
		SelectItem,
		SelectTrigger,
	},
	props: {
		title: {
			type: String,
		},
		subnavs: {
			type: Array,
		},
	},
	computed: {
		currentRoute() {
			const route = this.$route?.path;
			const exists = this.subnavs.find((item) => item.route === route);
			return exists ? route : this.subnavs[0]?.route || "/";
		},
	},
	methods: {
		handleRoute(route: string) {
			if (!route) {
				return;
			}
			this.$router
				.push({
					path: route,
				})
				.catch(() => {
					/* noop */
				});
		},
	},
};
</script>
