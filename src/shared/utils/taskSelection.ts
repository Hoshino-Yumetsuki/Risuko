type SelectableTaskRow = {
	_displayKey?: string;
	gid?: string;
};

export const getSelectableTaskKeys = <T extends SelectableTaskRow>(
	tasks: T[],
): string[] =>
	tasks
		.map((task) => task._displayKey || task.gid || "")
		.filter((key): key is string => key.length > 0);
