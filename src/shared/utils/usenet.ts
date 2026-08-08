import type { UsenetRepairFailure } from "@shared/types/task";

export type UsenetTranslator = (
	key: string,
	params?: Record<string, unknown>,
) => string;

export const formatUsenetRepairFailure = (
	failure: UsenetRepairFailure | undefined,
	translate: UsenetTranslator,
): string => {
	if (!failure) {
		return "";
	}

	const summary = translate("task.usenet-repair-insufficient", {
		neededBlocks: failure.neededBlocks,
		availableBlocks: failure.availableBlocks,
	});
	const nextStep = translate(
		failure.partialsRetained
			? "task.usenet-repair-partials-retained"
			: "task.usenet-repair-partials-unavailable",
	);
	return `${summary} ${nextStep}`;
};
