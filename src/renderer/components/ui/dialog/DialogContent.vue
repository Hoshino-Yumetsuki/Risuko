<script setup lang="ts">
import { X } from "@lucide/vue";
import { reactiveOmit } from "@vueuse/core";
import type { DialogContentEmits, DialogContentProps } from "reka-ui";
import {
	DialogClose,
	DialogContent,
	DialogPortal,
	useForwardPropsEmits,
} from "reka-ui";
import type { HTMLAttributes } from "vue";
import { cn } from "@/lib/utils";
import DialogOverlay from "./DialogOverlay.vue";

defineOptions({
	inheritAttrs: false,
});

const props = withDefaults(
	defineProps<
		DialogContentProps & {
			class?: HTMLAttributes["class"];
			showCloseButton?: boolean;
		}
	>(),
	{
		showCloseButton: true,
	},
);
const emits = defineEmits<DialogContentEmits>();

const delegatedProps = reactiveOmit(props, "class");

const forwarded = useForwardPropsEmits(delegatedProps, emits);
</script>

<template>
  <DialogPortal>
    <DialogOverlay />
    <DialogContent
      data-slot="dialog-content"
      v-bind="{ ...$attrs, ...forwarded }"
      :class="
        cn(
          'dialog-anim bg-background fixed top-[50%] left-[50%] z-50 grid w-full max-w-[calc(100%-2rem)] translate-x-[-50%] translate-y-[-50%] gap-4 rounded-lg border p-6 shadow-lg sm:max-w-lg',
          props.class,
        )
      "
    >
      <slot />

      <DialogClose
        v-if="showCloseButton"
        data-slot="dialog-close"
        class="focus:ring-ring/50 data-[state=open]:bg-accent data-[state=open]:text-muted-foreground absolute top-4 right-4 rounded-xs opacity-70 transition-opacity hover:opacity-100 focus:ring-2 focus:outline-hidden disabled:pointer-events-none [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4"
        :title="$t('window.close')"
        :aria-label="$t('window.close')"
      >
        <X />
        <span class="sr-only">{{ $t('window.close') }}</span>
      </DialogClose>
    </DialogContent>
  </DialogPortal>
</template>
