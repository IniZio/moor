/** A schedule as the form offers it: a preset in words, or raw cron under Advanced. */
export type Schedule =
	| { kind: 'none' }
	| { kind: 'weekdays'; at: string }
	| { kind: 'daily'; at: string }
	| { kind: 'weekly'; day: number; at: string }
	| { kind: 'hours'; every: number }
	| { kind: 'cron'; cron: string };

export const DAYS = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'];

/** The gaps "Every N hours" offers: those that divide a day, so every gap is the same. */
export const EVERY = [1, 2, 3, 4, 6, 8, 12];

export const FIRST: Schedule = { kind: 'weekdays', at: '09:00' };

function clock(at: string): [number, number] {
	const [h, m] = at.split(':').map(Number);
	return [h || 0, m || 0];
}

export function toCron(s: Schedule): string {
	if (s.kind === 'none') return '';
	if (s.kind === 'cron') return s.cron.trim();
	if (s.kind === 'hours') return s.every === 1 ? '0 * * * *' : `0 */${s.every} * * *`;
	const [h, m] = clock(s.at);
	const day = s.kind === 'weekdays' ? '1-5' : s.kind === 'daily' ? '*' : String(s.day);
	return `${m} ${h} * * ${day}`;
}

/** The preset a stored cron is, so an edit opens on it; raw cron for any other. */
export function fromCron(stored: string | null | undefined): Schedule {
	if (stored == null) return { kind: 'none' };
	const cron = stored;
	const f = cron.trim().split(/\s+/);
	const asCron: Schedule = { kind: 'cron', cron: cron.trim() };
	if (f.length !== 5 || f[2] !== '*' || f[3] !== '*') return asCron;
	const [m, h, , , d] = f;
	if (m === '0' && d === '*' && (h === '*' || /^\*\/\d+$/.test(h))) {
		const every = h === '*' ? 1 : Number(h.slice(2));
		if (EVERY.includes(every)) return { kind: 'hours', every };
	}
	if (!/^\d+$/.test(m) || !/^\d+$/.test(h) || Number(m) > 59 || Number(h) > 23) return asCron;
	const at = `${h.padStart(2, '0')}:${m.padStart(2, '0')}`;
	if (d === '*') return { kind: 'daily', at };
	if (d === '1-5') return { kind: 'weekdays', at };
	if (/^[0-7]$/.test(d)) return { kind: 'weekly', day: Number(d) % 7, at };
	return asCron;
}

export function browserZone(): string {
	return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC';
}

export function zones(): string[] {
	try {
		return Intl.supportedValuesOf('timeZone');
	} catch {
		return [];
	}
}

/** What fires an automation, as a person reads it: the words the gate gave, else the raw cron, else the pull request trigger. */
export function firesOn(w: { words?: string | null; cron?: string | null; on_pull_request?: boolean }): string {
	return w.words ?? w.cron ?? (w.on_pull_request ? 'On every pull request' : 'By hand');
}
