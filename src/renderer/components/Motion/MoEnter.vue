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

const easeOut = [0.25, 1, 0.5, 1] as const;

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
		initial: { opacity: 0, x: -5 },
		animate: { opacity: 1, x: 0 },
		duration: 0.18,
		ease: easeOut,
	},
	fadeInRight: {
		initial: { opacity: 0, x: 5 },
		animate: { opacity: 1, x: 0 },
		duration: 0.18,
		ease: easeOut,
	},
	fadeInUp: {
		initial: { opacity: 0, y: 5 },
		animate: { opacity: 1, y: 0 },
		duration: 0.2,
		ease: easeOut,
	},
	fadeInDown: {
		initial: { opacity: 0, y: -5 },
		animate: { opacity: 1, y: 0 },
		duration: 0.18,
		ease: easeOut,
	},
	fade: {
		initial: { opacity: 0 },
		animate: { opacity: 1 },
		duration: 0.15,
		ease: easeOut,
	},
	spring: {
		initial: { opacity: 0, scale: 0.97 },
		animate: { opacity: 1, scale: 1 },
		duration: 0.2,
		ease: easeOut,
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
