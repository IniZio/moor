import { useEffect, useRef, useState } from 'preact/hooks';

import { type BoxRow, type Inbox, inboxClear, inboxRead, type Progress } from './api';
import { Glyph, STATES } from './Glyph';
import { pathOf } from './hashQuery';
import { InboxView } from './Inbox';
import { sectionAt } from './Settings';
import { currentTheme, setTheme, type Theme } from './theme';
import { workflowsAt } from './Workflows';

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

type InboxPoll = { value: Inbox | null; now: number; reload: () => void };

export function TopBar({ email, csrf, rows, inbox, onSignOut }: { email: string; csrf: string; rows: BoxRow[]; inbox: InboxPoll; onSignOut: () => void }) {
	const [finding, setFinding] = useState(false);
	const [theme, setThemeState] = useState<Theme>(currentTheme());
	const [hash, setHash] = useState(location.hash);
	useEffect(() => {
		const on = () => setHash(location.hash);
		addEventListener('hashchange', on);
		return () => removeEventListener('hashchange', on);
	}, []);
	const path = pathOf(hash);
	const here = workflowsAt(path) ? 'automations' : sectionAt(path) ? 'team' : 'tasks';
	const current = (key: string) => (key === here ? 'page' : undefined);
	useEffect(() => {
		const on = (e: KeyboardEvent) => {
			const inTerminal = e.target instanceof Element && e.target.closest('.t-term-screen');
			if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k' && !inTerminal) {
				e.preventDefault();
				setFinding(true);
			}
		};
		addEventListener('keydown', on);
		return () => removeEventListener('keydown', on);
	}, []);
	const items = inbox.value?.items ?? [];
	const unread = inbox.value?.unread ?? 0;
	const markRead = (ids: number[]) => {
		void inboxRead(csrf, ids).then(inbox.reload, inbox.reload);
	};
	const focusAt = useRef<number | null>(null);
	useEffect(() => {
		const at = focusAt.current;
		if (at === null) return;
		focusAt.current = null;
		const left = document.querySelectorAll<HTMLElement>('#inbox .inbox-item');
		(left[Math.min(at, left.length - 1)] ?? document.querySelector<HTMLElement>('.nav.inbox'))?.focus();
	}, [inbox.value]);
	const clear = (which: { ids: number[] } | { through: number }, at: number) => {
		focusAt.current = at;
		void inboxClear(csrf, which).then(inbox.reload, inbox.reload);
	};
	const flip = () => {
		const next: Theme = theme === 'dark' ? 'light' : 'dark';
		setTheme(next);
		setThemeState(next);
	};
	return (
		<>
			<header class="top">
				<nav class="bar" aria-label="moor">
					<a class="brand" href="#/">
						<span class="mark">◆</span> moor
					</a>
					<a class="nav" href="#/" aria-current={current('tasks')}>
						Tasks
					</a>
					<a class="nav" href="#/workflows" aria-current={current('automations')}>
						Automations
					</a>
					<a class="nav" href="#/settings" aria-current={current('team')}>
						Team
					</a>
					<span class="grow" />
					<button class="find" onClick={() => setFinding(true)} aria-keyshortcuts="Meta+K Control+K" aria-label="Find a task">
						Find
						<kbd class="findkbd">{MAC ? '⌘K' : 'Ctrl K'}</kbd>
					</button>
					<button class="nav inbox" popovertarget="inbox" aria-haspopup="true">
						{`Inbox${unread > 0 ? ' ' : ''}`}
						{unread > 0 && <span class="count">{unread}</span>}
						{' ▾'}
					</button>
					<button class="avatar" popovertarget="usermenu" aria-label="Account menu" title={email}>
						{email.slice(0, 2).toLowerCase()}
					</button>
				</nav>
			</header>
			<div id="inbox" class="pop inbox" popover="auto">
				<InboxView
					items={items}
					rows={rows}
					now={inbox.now}
					unread={unread}
					onReadAll={() => markRead([])}
					onRemove={(item, at) => clear({ ids: [item.id] }, at)}
					onClearAll={(through) => clear({ through }, 0)}
					onOpen={(item, e) => {
						if (item.state === 'unread') markRead([item.id]);
						shut(e);
					}}
				/>
			</div>
			<div id="usermenu" class="pop menu usermenu" popover="auto">
				<a href="#/settings" onClick={shut} title={email}>
					You
				</a>
				<button
					onClick={(e) => {
						flip();
						shut(e);
					}}
				>
					{theme === 'dark' ? 'Light theme' : 'Dark theme'}
				</button>
				<button onClick={onSignOut}>Sign out</button>
			</div>
			<Switcher rows={rows} open={finding} onClose={() => setFinding(false)} />
		</>
	);
}
