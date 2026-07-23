export const bytesToSize = (bytes, precision = 1) => {
	const b = parseInt(bytes, 10);
	const sizes = ["B", "KB", "MB", "GB", "TB"];
	if (!Number.isFinite(b) || b <= 0) {
		return "0 KB";
	}
	const i = Math.min(
		Math.floor(Math.log(b) / Math.log(1024)),
		sizes.length - 1,
	);
	if (i === 0) {
		return `${b} ${sizes[i]}`;
	}
	return `${(b / 1024 ** i).toFixed(precision)} ${sizes[i]}`;
};
