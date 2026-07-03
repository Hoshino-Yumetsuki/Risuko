<template>
  <Transition name="overlay-fade">
    <div
      v-if="show"
      class="loading-overlay"
      :class="{ 'loading-overlay--blocking': block }"
      role="status"
      aria-live="polite"
      aria-busy="true"
    >
      <div class="loading-overlay-content">
        <svg
          class="loading-spinner"
          width="24"
          height="24"
          viewBox="0 0 24 24"
          fill="none"
          xmlns="http://www.w3.org/2000/svg"
        >
          <circle
            class="loading-spinner-track"
            cx="12"
            cy="12"
            r="10"
            stroke="currentColor"
            stroke-width="2.5"
          />
          <path
            class="loading-spinner-arc"
            d="M22 12a10 10 0 0 0-10-10"
            stroke="currentColor"
            stroke-width="2.5"
            stroke-linecap="round"
          />
        </svg>
        <span v-if="text" class="loading-overlay-text">{{ text }}</span>
      </div>
    </div>
  </Transition>
</template>

<script lang="ts">
export default {
	name: "loading-overlay",
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
.loading-overlay {
  position: absolute;
  inset: 0;
  z-index: 40;
  display: flex;
  align-items: center;
  justify-content: center;
  pointer-events: none;
  background: rgba(15, 23, 42, 0.4);
}

.loading-overlay--blocking {
  pointer-events: auto;
}

.loading-overlay-content {
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

.loading-overlay-text {
  white-space: normal;
  max-width: 220px;
}

.loading-spinner {
  color: #f8fafc;
}

.loading-spinner-track {
  opacity: 0.15;
}

.loading-spinner-arc {
  animation: spinner-spin 0.75s linear infinite;
  transform-origin: center;
}

@keyframes spinner-spin {
  to {
    transform: rotate(360deg);
  }
}

.overlay-fade-enter-active {
  transition: opacity 0.15s ease-out;
}
.overlay-fade-leave-active {
  transition: opacity 0.15s ease-in;
}
.overlay-fade-enter-from,
.overlay-fade-leave-to {
  opacity: 0;
}
</style>
