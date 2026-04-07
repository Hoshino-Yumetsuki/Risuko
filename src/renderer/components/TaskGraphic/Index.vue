<template>
  <svg
    version="1.1"
    xmlns="http://www.w3.org/2000/svg"
    class="svg-task-graphic"
    :width="width"
    :height="height"
    :viewBox="box"
  >
    <g v-for="(row, index) in atoms" :key="`g-${index}`">
      <mo-task-graphic-atom
        v-for="atom in row"
        :key="`atom-${atom.id}`"
        :status="atom.status"
        :width="atomWidth"
        :height="atomHeight"
        :radius="atomRadius"
        :x="atom.x"
        :y="atom.y"
      />
    </g>
  </svg>
</template>

<script lang="ts">
import Atom from "./Atom.vue";

const MAX_GRAPHIC_CELLS = 512;

export default {
	name: "mo-task-graphic",
	components: {
		[Atom.name]: Atom,
	},
	props: {
		bitfield: {
			type: String,
			default: "",
		},
		outerWidth: {
			type: Number,
			default: 240,
		},
		atomWidth: {
			type: Number,
			default: 10,
		},
		atomHeight: {
			type: Number,
			default: 10,
		},
		atomGutter: {
			type: Number,
			default: 3,
		},
		atomRadius: {
			type: Number,
			default: 2,
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
		normalizedBitfield() {
			return `${this.bitfield || ""}`.replace(/[^0-9a-fA-F]/g, "");
		},
		rawNibbleLen() {
			return this.normalizedBitfield.length;
		},
		rawBitLen() {
			return this.rawNibbleLen * 4;
		},
		len() {
			const count = Number(this.cellCount);
			if (Number.isFinite(count) && count > 0) {
				// Keep visual box count strictly aligned with configured split segments.
				return Math.min(Math.trunc(count), MAX_GRAPHIC_CELLS);
			}
			const { rawBitLen } = this;
			if (rawBitLen <= 0) {
				return 0;
			}
			return Math.min(rawBitLen, MAX_GRAPHIC_CELLS);
		},
		atomWG() {
			return this.atomWidth + this.atomGutter;
		},
		atomHG() {
			return this.atomHeight + this.atomGutter;
		},
		columnCount() {
			const { outerWidth, atomWidth, atomWG } = this;
			const result = Math.trunc((outerWidth - atomWidth) / atomWG) + 1;
			return result > 0 ? result : 1;
		},
		gridColumnCount() {
			const { len, columnCount } = this;
			if (len <= 0) {
				return 1;
			}

			// For small piece counts, prefer a balanced two-row layout (e.g. 16 => 8 + 8).
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
			const { outerWidth, atomWidth, atomWG, gridColumnCount } = this;
			const totalWidth = atomWG * (gridColumnCount - 1) + atomWidth;
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
			const { atomHeight, atomHG, rowCount, yOffset } = this;
			const contentHeight =
				rowCount > 0 ? atomHG * (rowCount - 1) + atomHeight : 0;
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

			if (Array.isArray(cellPercents) && cellPercents.length >= len) {
				return cellPercents
					.slice(0, len)
					.map((percent) => percentToStatus(percent));
			}

			const { normalizedBitfield, rawBitLen } = this;
			if (rawBitLen <= 0) {
				return [];
			}

			// Aggregate at bit granularity to avoid periodic hex-nibble artifacts.
			const sums = new Array(len).fill(0);
			const counts = new Array(len).fill(0);

			for (let bitIndex = 0; bitIndex < rawBitLen; bitIndex++) {
				const nibbleIndex = Math.trunc(bitIndex / 4);
				const bitInNibble = 3 - (bitIndex % 4);
				const nibble = parseInt(normalizedBitfield[nibbleIndex] || "0", 16);
				const bit = Number.isNaN(nibble) ? 0 : (nibble >> bitInNibble) & 1;
				const bucket = Math.min(
					Math.floor((bitIndex * len) / rawBitLen),
					len - 1,
				);
				sums[bucket] += bit;
				counts[bucket] += 1;
			}

			return sums.map((sum, idx) => {
				const count = counts[idx] || 1;
				const percent = Math.round((sum / count) * 100);
				return percentToStatus(percent);
			});
		},
		atoms() {
			const { len, gridColumnCount } = this;
			const result = [];
			let row = [];
			for (let i = 0; i < len; i++) {
				row.push(this.buildAtom(i));

				if ((i + 1) % gridColumnCount === 0) {
					result.push(row);
					row = [];
				}
			}
			if (row.length > 0) {
				result.push(row);
			}

			return result;
		},
	},
	methods: {
		buildAtom(index) {
			const {
				xOffset,
				yOffset,
				atomWG,
				atomHG,
				gridColumnCount,
				cellStatuses,
			} = this;
			const hIndex = index + 1;
			let chIndex = index % gridColumnCount;
			let rhIndex = Math.trunc(index / gridColumnCount);
			chIndex = chIndex < 0 ? 0 : chIndex;
			rhIndex = rhIndex < 0 ? 0 : rhIndex;
			const result = {
				id: `${hIndex}`,
				status: cellStatuses[index] ?? 0,
				x: xOffset + chIndex * atomWG,
				y: yOffset + rhIndex * atomHG,
			};

			return result;
		},
	},
};
</script>
