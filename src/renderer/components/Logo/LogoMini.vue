<template>
  <mo-enter tag="h1" preset="spring" :delay="0.2" class="logo-mini">
    <a target="_blank" href="https://risuko.vercel.app">
      <img src="@static/logo.svg" class="w-7 h-7" alt="Risuko" />
    </a>
  </mo-enter>
</template>

<script lang="ts">
export default {
	name: "mo-logo-mini",
};
</script>
