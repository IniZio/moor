export type Theme = 'dark' | 'light';

const KEY = 'moor.theme';

function stored(): Theme | null {
	try {
		const v = localStorage.getItem(KEY);
		return v === 'dark' || v === 'light' ? v : null;
	} catch {
		return null;
	}
}

/** The theme the page shows: the one the person chose, else the system's. */
export function currentTheme(): Theme {
	return document.documentElement.dataset.theme === 'dark' ? 'dark' : 'light';
}

/** Puts the chosen or system theme on the page; called once before the first render. */
export function startTheme(): void {
	document.documentElement.dataset.theme = stored() ?? (matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light');
}

/** Shows `theme` and remembers it for next time. */
export function setTheme(theme: Theme): void {
	document.documentElement.dataset.theme = theme;
	try {
		localStorage.setItem(KEY, theme);
	} catch {
		// the choice still holds for this visit
	}
}
