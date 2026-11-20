import { useEffect, useState } from 'preact/hooks';

import { failure, schedulePreview } from './api';
import { DAYS, EVERY, type Schedule, toCron } from './schedule';

export function SchedulePicker({ value, onChange }: { value: Schedule; onChange: (s: Schedule) => void }) {
	const at = 'at' in value ? value.at : '09:00';
	const pick = (kind: string) => {
		if (kind === 'none') onChange({ kind });
		else if (kind === 'weekdays' || kind === 'daily') onChange({ kind, at });
		else if (kind === 'weekly') onChange({ kind, day: 1, at });
		else if (kind === 'hours') onChange({ kind, every: 6 });
		else onChange({ kind: 'cron', cron: toCron(value) });
	};
	return (
		<div class="wf-when">
			<select aria-label="How often" value={value.kind} onChange={(e) => pick(e.currentTarget.value)}>
				<option value="none">No schedule (run by hand)</option>
				<option value="weekdays">Every weekday</option>
				<option value="daily">Every day</option>
				<option value="weekly">Every week on</option>
				<option value="hours">Every few hours</option>
				<option value="cron">Advanced (cron)</option>
			</select>
			{value.kind === 'weekly' && (
				<select aria-label="Day" value={value.day} onChange={(e) => onChange({ ...value, day: Number(e.currentTarget.value) })}>
					{DAYS.map((d, i) => (
						<option value={i} key={d}>
							{d}
						</option>
					))}
				</select>
			)}
			{'at' in value && (
				<label>
					at <input type="time" aria-label="Time" required value={value.at} onInput={(e) => onChange({ ...value, at: e.currentTarget.value })} />
				</label>
			)}
			{value.kind === 'hours' && (
				<select aria-label="Hours apart" value={value.every} onChange={(e) => onChange({ kind: 'hours', every: Number(e.currentTarget.value) })}>
					{EVERY.map((n) => (
						<option value={n} key={n}>
							{n === 1 ? 'every hour' : `every ${n} hours`}
						</option>
					))}
				</select>
			)}
			{value.kind === 'cron' && (
				<input
					class="t-mono"
					aria-label="Cron"
					placeholder="minute hour day-of-month month day-of-week"
					required
					value={value.cron}
					onInput={(e) => onChange({ kind: 'cron', cron: e.currentTarget.value })}
				/>
			)}
		</div>
	);
}

export function Preview({ cron, zone }: { cron: string; zone: string }) {
	const [shown, setShown] = useState<{ words?: string | null; next: number[]; error?: string }>({ next: [] });
	useEffect(() => {
		let live = true;
		const timer = setTimeout(() => {
			schedulePreview(cron, zone).then(
				(p) => live && setShown(p),
				(e) => live && setShown({ next: [], error: failure(e) }),
			);
		}, 250);
		return () => {
			live = false;
			clearTimeout(timer);
		};
	}, [cron, zone]);
	if (shown.error) return <p class="notice danger" role="alert">{shown.error}</p>;
	let format: Intl.DateTimeFormat;
	try {
		format = new Intl.DateTimeFormat(undefined, { timeZone: zone, weekday: 'short', day: 'numeric', month: 'short', hour: 'numeric', minute: '2-digit', hourCycle: 'h23' });
	} catch {
		return null;
	}
	return (
		<div class="wf-next" aria-live="polite">
			<span class="muted">{shown.words ? `${shown.words}. ` : ''}Next runs in {zone}:</span>
			<ol>
				{shown.next.map((ms) => (
					<li key={ms}>{format.format(ms)}</li>
				))}
			</ol>
		</div>
	);
}
