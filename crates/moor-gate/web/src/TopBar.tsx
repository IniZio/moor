import { useEffect, useRef, useState } from 'preact/hooks';

import type { BoxRow, Progress } from './api';
import { Glyph, STATES } from './Portal';
import { currentTheme, setTheme, type Theme } from './theme';

/** Where a Box sits in the switcher: the ones that need the person first. */
const ORDER = {
	needs_you: { rank: 0, label: 'Needs you' },
	working: { rank: 1, label: 'Working' },
	setting_up: { rank: 1, label: 'Working' },
	queued: { rank: 1, label: 'Working' },
	done: { rank: 2, label: 'Done' },
	failed: { rank: 2, label: 'Done' },
	asleep: { rank: 3, label: 'Asleep' },
	closed: { rank: 4, label: 'Closed' },
} as const satisfies Record<Progress, { rank: number; label: string }>;

/** The Boxes that match `query`, needs-you first; the order inside a group is the list's own. */
export function matches(rows: BoxRow[], query: string): BoxRow[] {
	const q = query.trim().toLowerCase();
	return rows
		.flatMap((t, at) => ([t.title, t.repo, t.line, t.box].join(' ').toLowerCase().includes(q) ? [{ t, at }] : []))
		.sort((a, b) => ORDER[a.t.state].rank - ORDER[b.t.state].rank || a.at - b.at)
		.map((x) => x.t);
}

const MAC = /Mac|iPhone|iPad/.test(navigator.platform);

function Switcher({ rows, open, onClose }: { rows: BoxRow[]; open: boolean; onClose: () => void }) {
	const dialog = useRef<HTMLDialogElement>(null);
	const [query, setQuery] = useState('');
	const [at, setAt] = useState(0);
	const found = matches(rows, query);
	useEffect(() => {
		const d = dialog.current;
		if (!d) return;
		if (open && !d.open) {
			setQuery('');
			setAt(0);
			d.showModal();
		} else if (!open && d.open) d.close();
	}, [open]);
	const go = (t: BoxRow | undefined) => {
		if (!t) return;
		onClose();
		location.hash = `#/box/${encodeURIComponent(t.box)}`;
	};
	const key = (e: KeyboardEvent) => {
		const down = e.key === 'ArrowDown' || (e.ctrlKey && e.key === 'j');
		const up = e.key === 'ArrowUp' || (e.ctrlKey && e.key === 'k');
		if (down || up) {
			e.preventDefault();
			const next = Math.max(0, Math.min(found.length - 1, at + (down ? 1 : -1)));
			setAt(next);
			dialog.current?.querySelector(`[data-at="${next}"]`)?.scrollIntoView({ block: 'nearest' });
		} else if (e.key === 'Enter') {
			e.preventDefault();
			go(found[at]);
		}
	};
	let last = '';
	return (
		<dialog
			ref={dialog}
			class="switcher"
			aria-label="Find a task"
			onClose={onClose}
			onClick={(e) => {
				if (e.target === dialog.current) onClose();
			}}
		>
			<input
				class="sw-q"
				placeholder="Find a task… needs you first"
				autocomplete="off"
				role="combobox"
				aria-expanded="true"
				aria-controls="sw-list"
				value={query}
				onInput={(e) => {
					setQuery(e.currentTarget.value);
					setAt(0);
				}}
				onKeyDown={key}
			/>
			<ul id="sw-list" role="listbox">
				{found.map((t, n) => {
					const group = ORDER[t.state].label;
					const head = group !== last;
					last = group;
					return (
						<>
							{head && <li class="sw-g" role="presentation" key={`g-${group}`}>{group}</li>}
							<li
								role="option"
								class={`sw-i t-${STATES[t.state].tone}`}
								aria-selected={n === at}
								data-at={n}
								key={t.box}
								onMouseMove={() => setAt(n)}
								onClick={() => go(t)}
							>
								<Glyph state={t.state} stale={t.stale} />
								<span class="sw-t">
									{t.title}
									<span class="t-mono muted">{t.repo ?? t.box}</span>
								</span>
								<span class="sw-n">{t.line}</span>
							</li>
						</>
					);
				})}
				{!found.length && <li class="sw-g" role="presentation">No task matches.</li>}
			</ul>
			<div class="sw-foot muted">
				<kbd>↑</kbd> <kbd>↓</kbd> move · <kbd>enter</kbd> open · <kbd>esc</kbd> close
			</div>
		</dialog>
	);
}

/** Closes the popover a link or button sits in, once it is used. */
function shut(e: Event) {
	(e.currentTarget as HTMLElement).closest<HTMLElement>('[popover]')?.hidePopover();
}

export function TopBar({ email, rows, onSignOut }: { email: string; rows: BoxRow[]; onSignOut: () => void }) {
	const [finding, setFinding] = useState(false);
	const [theme, setThemeState] = useState<Theme>(currentTheme());
	useEffect(() => {
		const on = (e: KeyboardEvent) => {
			if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
				e.preventDefault();
				setFinding(true);
			}
		};
		addEventListener('keydown', on);
		return () => removeEventListener('keydown', on);
	}, []);
	const needs = rows.filter((t) => t.state === 'needs_you');
	const flip = () => {
		const next: Theme = theme === 'dark' ? 'light' : 'dark';
		setTheme(next);
		setThemeState(next);
	};
	return (
		<>
			<header class="top">
				<a class="brand" href="#/">
					<span class="mark">◆</span> moor
				</a>
				<button class="find" onClick={() => setFinding(true)} aria-keyshortcuts="Meta+K Control+K">
					<span>Find a task</span>
					<kbd>{MAC ? '⌘K' : 'Ctrl K'}</kbd>
				</button>
				<span class="grow" />
				<a class="btn primary" href="#/new">
					{'+ New task'}
				</a>
				<button class="btn ghost" popovertarget="inbox">
					Inbox{needs.length > 0 && <b class="count">{needs.length}</b>}
				</button>
				<button class="btn ghost icon" onClick={flip} aria-label={`Switch to ${theme === 'dark' ? 'light' : 'dark'} theme`} title={`Switch to ${theme === 'dark' ? 'light' : 'dark'}`}>
					{theme === 'dark' ? '☀' : '☾'}
				</button>
				<button class="me" popovertarget="usermenu" aria-label="Account menu" title={email}>
					{email.slice(0, 2).toLowerCase()}
				</button>
			</header>
			<div id="inbox" class="pop inbox" popover="auto">
				<div class="pop-h">
					Inbox <span class="muted">what needs you now</span>
				</div>
				{needs.map((t) => (
					<a class="inbox-item" href={`#/box/${encodeURIComponent(t.box)}`} onClick={shut} key={t.box}>
						<Glyph state={t.state} stale={t.stale} />
						<span>
							<b>{t.title}</b> <span class="t-mono muted">{t.repo ?? ''}</span>
							<br />
							{t.line}
						</span>
					</a>
				))}
				{!needs.length && <p class="t-empty">Nothing needs you.</p>}
			</div>
			<div id="usermenu" class="pop menu usermenu" popover="auto">
				<div class="menu-who muted">{email}</div>
				<a href="#/workflows" onClick={shut}>
					Workflows
				</a>
				<a href="#/repos" onClick={shut}>
					Repos
				</a>
				<a href="#/settings" onClick={shut}>
					Settings
				</a>
				<button onClick={onSignOut}>Sign out</button>
			</div>
			<Switcher rows={rows} open={finding} onClose={() => setFinding(false)} />
		</>
	);
}
