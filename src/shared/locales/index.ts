export const availableLanguages = [
	{
		value: "auto",
		label: "Auto",
	},
	{
		value: "ar",
		label: "عربي",
	},
	{
		value: "bg",
		label: "Българският език",
	},
	{
		value: "ca",
		label: "Català",
	},
	{
		value: "de",
		label: "Deutsch",
	},
	{
		value: "el",
		label: "Ελληνικά",
	},
	{
		value: "en-US",
		label: "English",
	},
	{
		value: "es",
		label: "Español",
	},
	{
		value: "fa",
		label: "فارسی",
	},
	{
		value: "fr",
		label: "Français",
	},
	{
		value: "hu",
		label: "Hungarian",
	},
	{
		value: "id",
		label: "Indonesia",
	},
	{
		value: "it",
		label: "Italiano",
	},
	{
		value: "ja",
		label: "日本語",
	},
	{
		value: "ko",
		label: "한국어",
	},
	{
		value: "nb",
		label: "Norsk Bokmål",
	},
	{
		value: "nl",
		label: "Nederlands",
	},
	{
		value: "pl",
		label: "Polski",
	},
	{
		value: "pt-BR",
		label: "Português (Brasil)",
	},
	{
		value: "ro",
		label: "Română",
	},
	{
		value: "ru",
		label: "Русский",
	},
	{
		value: "th",
		label: "แบบไทย",
	},
	{
		value: "tr",
		label: "Türkçe",
	},
	{
		value: "uk",
		label: "Українська",
	},
	{
		value: "vi",
		label: "Tiếng Việt",
	},
	{
		value: "zh-CN",
		label: "简体中文",
	},
	{
		value: "zh-TW",
		label: "繁體中文",
	},
];

const checkLngIsAvailable = (locale) => {
	return availableLanguages.some((lng) => lng.value === locale);
};

export const getLanguage = (locale = "en-US") => {
	if (locale === "auto") {
		const system = getSystemLocale();
		return getLanguage(system === "auto" ? "en-US" : system);
	}

	if (typeof locale !== "string" || !locale) {
		return "en-US";
	}

	if (checkLngIsAvailable(locale)) {
		return locale;
	}

	if (locale.startsWith("ar")) {
		return "ar";
	}

	if (locale.startsWith("de")) {
		return "de";
	}

	if (locale.startsWith("en")) {
		return "en-US";
	}

	if (locale.startsWith("es")) {
		return "es";
	}

	if (locale.startsWith("fr")) {
		return "fr";
	}

	if (locale.startsWith("it")) {
		return "it";
	}

	if (locale.startsWith("pt")) {
		return "pt-BR";
	}

	if (locale === "zh-HK") {
		return "zh-TW";
	}

	if (locale.startsWith("zh")) {
		return "zh-CN";
	}

	return "en-US";
};

const getSystemLocale = () => {
	if (typeof navigator === "undefined") {
		return "en-US";
	}
	const locales = Array.isArray(navigator.languages)
		? navigator.languages
		: [navigator.language];
	return (
		locales.find((locale) => typeof locale === "string" && locale) || "en-US"
	);
};
