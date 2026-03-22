<script setup lang="ts">
import { ref, watch } from 'vue'
import { TriangleAlert } from 'lucide-vue-next'
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogFooter,
} from '@/components/ui/dialog'
import { Checkbox } from '@/components/ui/checkbox'
import { Label } from '@/components/ui/label'
import { Button } from '@/components/ui/button'

const props = withDefaults(
  defineProps<{
    open: boolean
    title?: string
    message?: string
    kind?: 'info' | 'warning'
    confirmText?: string
    cancelText?: string
    checkboxLabel?: string
    checkboxChecked?: boolean
  }>(),
  {
    title: '',
    message: '',
    kind: 'info',
    confirmText: 'OK',
    cancelText: 'Cancel',
    checkboxLabel: '',
    checkboxChecked: false,
  },
)

const emit = defineEmits<{
  confirm: [checkboxChecked: boolean]
  cancel: []
  'update:open': [value: boolean]
}>()

const checked = ref(props.checkboxChecked)

watch(
  () => props.checkboxChecked,
  (val) => {
    checked.value = val
  },
)

watch(
  () => props.open,
  (val) => {
    if (val) checked.value = props.checkboxChecked
  },
)

function onConfirm() {
  emit('confirm', checked.value)
  emit('update:open', false)
}

function onCancel() {
  emit('cancel')
  emit('update:open', false)
}

function onOpenChange(val: boolean) {
  if (!val) onCancel()
}
</script>

<template>
  <Dialog :open="open" @update:open="onOpenChange">
    <DialogContent :show-close-button="false" class="sm:max-w-[400px] gap-0 p-0">
      <div class="flex flex-col gap-3 p-5 pb-4">
        <DialogHeader class="gap-3">
          <div
            v-if="kind === 'warning'"
            class="flex size-9 items-center justify-center rounded-full bg-amber-500/10"
          >
            <TriangleAlert class="size-5 text-amber-500" />
          </div>
          <DialogTitle class="text-[15px] font-semibold leading-snug">
            {{ title }}
          </DialogTitle>
        </DialogHeader>
        <p class="text-[13px] leading-relaxed text-muted-foreground">
          {{ message }}
        </p>
        <label
          v-if="checkboxLabel"
          class="flex items-center gap-2 pt-1 cursor-pointer select-none"
        >
          <Checkbox
            :model-value="checked"
            @update:model-value="(val: boolean) => (checked = val)"
          />
          <Label class="text-[13px] font-normal cursor-pointer">
            {{ checkboxLabel }}
          </Label>
        </label>
      </div>
      <DialogFooter class="flex-row justify-end gap-2 border-t px-5 py-3">
        <Button variant="outline" size="sm" @click="onCancel">
          {{ cancelText }}
        </Button>
        <Button
          :variant="kind === 'warning' ? 'destructive' : 'default'"
          size="sm"
          @click="onConfirm"
        >
          {{ confirmText }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
