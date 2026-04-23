<template>
  <Motion
    :as="tag"
    :initial="resolved.initial"
    :animate="resolved.animate"
    :transition="resolved.transition"
  >
    <slot />
  </Motion>
</template>

<script setup lang="ts">
// biome-ignore lint/correctness/noUnusedImports: used in template
import { Motion } from "motion-v";
import { computed } from "vue";

type Preset =
	| "fadeInLeft"
	| "fadeInRight"
	| "fadeInUp"
	| "fadeInDown"
	| "fade"
	| "spring";

const props = withDefaults(
	defineProps<{
		preset?: Preset;
		delay?: number;
		duration?: number;
		tag?: string;
	}>(),
	{
		preset: "fadeInUp",
		delay: 0,
		tag: "div",
	},
);

const decel = [0.22, 0.61, 0.36, 1] as const;
const spring = [0.34, 1.56, 0.64, 1] as const;

const presets: Record<
	Preset,
	{
		initial: Record<string, number>;
		animate: Record<string, number>;
		duration: number;
		ease: readonly number[];
	}
> = {
	fadeInLeft: {
		initial: { opacity: 0, x: -8 },
		animate: { opacity: 1, x: 0 },
		duration: 0.4,
		ease: decel,
	},
	fadeInRight: {
		initial: { opacity: 0, x: 8 },
		animate: { opacity: 1, x: 0 },
		duration: 0.4,
		ease: decel,
	},
	fadeInUp: {
		initial: { opacity: 0, y: 12 },
		animate: { opacity: 1, y: 0 },
		duration: 0.5,
		ease: decel,
	},
	fadeInDown: {
		initial: { opacity: 0, y: -10 },
		animate: { opacity: 1, y: 0 },
		duration: 0.4,
		ease: decel,
	},
	fade: {
		initial: { opacity: 0 },
		animate: { opacity: 1 },
		duration: 0.3,
		ease: decel,
	},
	spring: {
		initial: { opacity: 0, scale: 0.8 },
		animate: { opacity: 1, scale: 1 },
		duration: 0.5,
		ease: spring,
	},
};

const resolved = computed(() => {
	const p = presets[props.preset];
	return {
		initial: p.initial,
		animate: p.animate,
		transition: {
			duration: props.duration ?? p.duration,
			ease: p.ease,
			delay: props.delay,
		},
	};
});
</script>
