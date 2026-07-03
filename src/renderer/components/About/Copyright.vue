<template>
  <div class="copyright">
    <a
      target="_blank"
      rel="noopener noreferrer"
      href="https://risuko.vercel.app"
      class="copyright-brand"
    >
      &copy;{{ year }} Risuko
    </a>
    <nav class="copyright-links">
      <a
        target="_blank"
        rel="noopener noreferrer"
        href="https://github.com/YueMiyuki/Risuko/blob/master/LICENSE"
      >
        {{ $t('about.license') }}
      </a>
      <a
        target="_blank"
        rel="noopener noreferrer"
        href="https://github.com/YueMiyuki/Risuko/blob/master/README.md"
      >
        {{ $t('about.about') }}
      </a>
      <a
        target="_blank"
        rel="noopener noreferrer"
        href="https://github.com/YueMiyuki/Risuko/issues"
      >
        {{ $t('about.support') }}
      </a>
      <a
        target="_blank"
        rel="noopener noreferrer"
        href="https://github.com/YueMiyuki/Risuko/releases"
      >
        {{ $t('about.release') }}
      </a>
    </nav>
  </div>
</template>

<script lang="ts">
export default {
	name: "CopyrightBar",
	data() {
		const year = new Date().getFullYear();
		return {
			year,
		};
	},
};
</script>
