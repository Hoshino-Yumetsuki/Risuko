<template>
  <ui-row class="copyright">
    <ui-col :span="6" class="copyright-left">
      <a target="_blank" rel="noopener noreferrer" href="https://risuko.vercel.app">
        &copy;{{ year }} Risuko
      </a>
    </ui-col>
    <ui-col :span="18" class="copyright-right">
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
    </ui-col>
  </ui-row>
</template>

<script lang="ts">
export default {
	name: "mo-copyright",
	data() {
		const year = new Date().getFullYear();
		return {
			year,
		};
	},
};
</script>
