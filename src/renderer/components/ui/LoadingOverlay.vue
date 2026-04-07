<template>
  <Transition name="mo-overlay-fade">
    <div
      v-if="show"
      class="mo-loading-overlay"
      :class="{ 'mo-loading-overlay--blocking': block }"
      role="status"
      aria-live="polite"
      aria-busy="true"
    >
      <div class="mo-loading-overlay-content">
        <svg
          class="mo-loading-spinner"
          width="24"
          height="24"
          viewBox="0 0 24 24"
          fill="none"
          xmlns="http://www.w3.org/2000/svg"
        >
          <circle
            class="mo-loading-spinner-track"
            cx="12"
            cy="12"
            r="10"
            stroke="currentColor"
            stroke-width="2.5"
          />
          <path
            class="mo-loading-spinner-arc"
            d="M22 12a10 10 0 0 0-10-10"
            stroke="currentColor"
            stroke-width="2.5"
            stroke-linecap="round"
          />
        </svg>
        <span v-if="text" class="mo-loading-overlay-text">{{ text }}</span>
      </div>
    </div>
  </Transition>
</template>

<script lang="ts">
export default {
	name: "mo-loading-overlay",
	props: {
		show: {
			type: Boolean,
			default: false,
		},
		text: {
			type: String,
			default: "",
		},
		block: {
			type: Boolean,
			default: false,
		},
	},
};
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

.mo-loading-spinner {
  color: #f8fafc;
}

.mo-loading-spinner-track {
  opacity: 0.15;
}

.mo-loading-spinner-arc {
  animation: mo-spin 0.75s linear infinite;
  transform-origin: center;
}

@keyframes mo-spin {
  to {
    transform: rotate(360deg);
  }
}

.mo-overlay-fade-enter-active {
  transition: opacity 0.15s ease-out;
}
.mo-overlay-fade-leave-active {
  transition: opacity 0.15s ease-in;
}
.mo-overlay-fade-enter-from,
.mo-overlay-fade-leave-to {
  opacity: 0;
}
</style>
