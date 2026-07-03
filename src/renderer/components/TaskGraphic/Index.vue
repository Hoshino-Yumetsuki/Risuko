<template>
  <svg
    version="1.1"
    xmlns="http://www.w3.org/2000/svg"
    class="svg-task-graphic"
    :width="width"
    :height="height"
    :viewBox="box"
  >
    <rect
      v-for="atom in atoms"
      :key="`atom-${atom.id}`"
      :class="`graphic-atom graphic-atom-s${atom.status}`"
      width="10"
      height="10"
      rx="2"
      ry="2"
      :x="atom.x"
      :y="atom.y"
    />
  </svg>
</template>

<script lang="ts">
const MAX_GRAPHIC_CELLS = 512;
const ATOM_W = 10;
const ATOM_H = 10;
const ATOM_GUTTER = 3;
const ATOM_WG = ATOM_W + ATOM_GUTTER;
const ATOM_HG = ATOM_H + ATOM_GUTTER;

export default {
	name: "task-graphic",
	props: {
		outerWidth: {
			type: Number,
			default: 240,
		},
		cellCount: {
			type: Number,
			default: 0,
		},
		cellPercents: {
			type: Array,
			default() {
				return [];
			},
		},
	},
	computed: {
		len() {
			return Math.min(Math.trunc(Number(this.cellCount)), MAX_GRAPHIC_CELLS);
		},
		columnCount() {
			const result = Math.trunc((this.outerWidth - ATOM_W) / ATOM_WG) + 1;
			return result > 0 ? result : 1;
		},
		gridColumnCount() {
			const { len, columnCount } = this;
			if (len <= 0) {
				return 1;
			}

			if (len > 8 && len <= columnCount * 2) {
				return Math.min(Math.ceil(len / 2), columnCount);
			}

			return Math.min(len, columnCount);
		},
		rowCount() {
			const { len, gridColumnCount } = this;
			const result = len > 0 ? Math.ceil(len / gridColumnCount) : 0;
			return result;
		},
		xOffset() {
			const { outerWidth, gridColumnCount } = this;
			const totalWidth = ATOM_WG * (gridColumnCount - 1) + ATOM_W;
			const result = (outerWidth - totalWidth) / 2;
			return parseFloat(Math.max(result, 0).toFixed(2));
		},
		yOffset() {
			return 4;
		},
		width() {
			const result = this.outerWidth;
			return Math.trunc(result);
		},
		height() {
			const { rowCount, yOffset } = this;
			const contentHeight =
				rowCount > 0 ? ATOM_HG * (rowCount - 1) + ATOM_H : 0;
			const result = contentHeight + yOffset * 2;
			return Math.trunc(result);
		},
		box() {
			return `0 0 ${this.width} ${this.height}`;
		},
		cellStatuses() {
			const { len, cellPercents } = this;
			if (len <= 0) {
				return [];
			}

			const percentToStatus = (percent) => {
				const p = Number(percent);
				if (!Number.isFinite(p) || p <= 0) {
					return 0;
				}
				if (p >= 75) {
					return 4;
				}
				if (p >= 50) {
					return 3;
				}
				if (p >= 25) {
					return 2;
				}
				return 1;
			};

			return cellPercents
				.slice(0, len)
				.map((percent) => percentToStatus(percent));
		},
		atoms() {
			return Array.from({ length: this.len }, (_, i) => ({
				id: i,
				status: this.cellStatuses[i] ?? 0,
				x: this.xOffset + (i % this.gridColumnCount) * ATOM_WG,
				y: this.yOffset + Math.trunc(i / this.gridColumnCount) * ATOM_HG,
			}));
		},
	},
};
</script>
