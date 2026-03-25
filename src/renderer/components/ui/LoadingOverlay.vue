<template>
  <div
    v-if="show"
    class="mo-loading-overlay"
    :class="{ 'mo-loading-overlay--blocking': block }"
    role="status"
    aria-live="polite"
    aria-busy="true"
  >
    <div class="mo-loading-overlay-content">
      <span v-if="text" class="mo-loading-overlay-text">{{ text }}</span>
    </div>
  </div>
</template>

<script lang="ts">
export default {
  name: 'mo-loading-overlay',
  props: {
    show: {
      type: Boolean,
      default: false,
    },
    text: {
      type: String,
      default: '',
    },
    block: {
      type: Boolean,
      default: false,
    },
  },
}
</script>

<style scoped>
.mo-loading-overlay {
  position: absolute;
  inset: 0;
  z-index: 40;
  display: flex;
  align-items: center;
  justify-content: center;
  pointer-events: none;
  background: rgba(15, 23, 42, 0.4);
}

.mo-loading-overlay--blocking {
  pointer-events: auto;
}

.mo-loading-overlay-content {
  display: inline-flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  color: #f8fafc;
  font-size: 12px;
  line-height: 16px;
  text-align: center;
}

.mo-loading-overlay-text {
  white-space: normal;
  max-width: 220px;
}
</style>
