<template>
  <div class="task-peers">
    <div v-if="peerRows.length === 0" class="peers-empty">
      {{ $t('task.no-peers') }}
    </div>
    <recycle-scroller
      v-else
      class="peers-scroller"
      :items="peerRows"
      :item-size="68"
      key-field="key"
    >
      <template #default="{ item }">
        <div class="peer-card">
          <div class="peer-card-header">
            <span class="peer-card-host">{{ item.ip }}:{{ item.port }}</span>
            <span class="peer-card-progress">{{ item.percent }}%</span>
          </div>
          <div v-if="item.peerClientName" class="peer-card-client">
            {{ item.peerClientName }}
          </div>
        </div>
      </template>
    </recycle-scroller>
  </div>
</template>

<script lang="ts">
export default {
	name: "task-peers",
	props: {
		peers: {
			type: Array,
			default: () => [],
		},
	},
	computed: {
		peerRows() {
			return this.peers.map((row) => ({
				...row,
				key: `${row.ip}:${row.port}`,
			}));
		},
	},
};
</script>
