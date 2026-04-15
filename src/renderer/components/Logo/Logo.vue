<template>
  <div class="logo">
    <a target="_blank" href="https://risuko.vercel.app">
      <img src="@static/logo.svg" :width="width" :height="height" alt="Risuko" />
    </a>
  </div>
</template>

<script lang="ts">
export default {
	name: "mo-logo",
	props: {
		width: {
			type: Number,
			default: 48,
		},
		height: {
			type: Number,
			default: 48,
		},
	},
};
</script>
