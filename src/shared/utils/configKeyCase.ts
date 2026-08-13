export const toKebabCasePreserveNumbers = (key = "") => {
	return `${key}`
		.replace(/([A-Z])([A-Z][a-z])/g, "$1-$2")
		.replace(/([a-z0-9])([A-Z])/g, "$1-$2")
		.replace(/[_\s]+/g, "-")
		.toLowerCase()
		.replace(/^ed2-k(?=-|$)/, "ed2k");
};
