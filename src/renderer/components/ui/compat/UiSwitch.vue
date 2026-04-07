<script setup lang="ts">
import { computed } from "vue";
import { Switch } from "../switch";

const props = withDefaults(
	defineProps<{
		modelValue?: boolean;
		disabled?: boolean;
		activeText?: string;
		inactiveText?: string;
	}>(),
	{
		modelValue: false,
		disabled: false,
		activeText: "",
		inactiveText: "",
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
  <label class="ui-switch" :class="{ 'is-disabled': disabled }">
    <Switch
      :model-value="checked"
      :disabled="disabled"
      @update:model-value="
        (val: boolean) => {
          checked = val;
        }
      "
    />
    <span v-if="activeText || inactiveText" class="ui-switch__text">
      {{ modelValue ? activeText : inactiveText }}
    </span>
  </label>
</template>
