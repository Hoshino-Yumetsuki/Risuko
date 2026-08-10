<script setup lang="ts">
import { computed, useAttrs } from "vue";
import { Checkbox } from "../checkbox";

defineOptions({ inheritAttrs: false });

const attrs = useAttrs();

const props = withDefaults(
	defineProps<{
		modelValue?: boolean;
		disabled?: boolean;
	}>(),
	{
		modelValue: false,
		disabled: false,
	},
);

const emit = defineEmits<{
	"update:modelValue": [value: boolean];
	change: [value: boolean];
}>();

const checked = computed({
	get: () => props.modelValue,
	set: (val: boolean) => {
		emit("update:modelValue", val);
		emit("change", val);
	},
});
</script>

<template>
  <label class="ui-checkbox" :class="{ 'is-disabled': disabled }">
    <Checkbox
      v-bind="attrs"
      :model-value="checked"
      :disabled="disabled"
      @update:model-value="
        (val: boolean) => {
          checked = val;
        }
      "
    />
    <span class="ui-checkbox__label">
      <slot />
    </span>
  </label>
</template>
