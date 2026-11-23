import type { BoxRow, InboxItem } from './api';
import './inbox.css';
import { gateLine } from './gate';
import { exactTime, RepoCell, reposOf } from './HomeList';

/** What the inbox shows beside an item's wording: its mark, by state and kind. */
function mark(item: InboxItem) {
	if (item.state === 'resolved') return { glyph: '✓', word: 'answered', tone: 'done' };
	if (item.kind === 'run_failed') return { glyph: '✕', word: 'failed', tone: 'failed' };
	return { glyph: '●', word: item.state === 'unread' ? 'unread' : 'read', tone: item.state === 'unread' ? 'needs' : 'quiet' };
}

/** Unread items first, each part keeping the newest-first order the controller sends. */
export function inboxOrder(items: readonly InboxItem[]): InboxItem[] {
	return [...items.filter((i) => i.state === 'unread'), ...items.filter((i) => i.state !== 'unread')];
}

export type InboxProps = {
	items: readonly InboxItem[];
	/** The person's tasks, to list every repo of an item's task; an item itself names one. */
	rows: readonly BoxRow[];
	now: number;
	unread: number;
	onReadAll: () => void;
	onOpen: (item: InboxItem, e: Event) => void;
	/** Takes `item` out of the list; `at` is where it sat, so focus can move to the item after it. */
	onRemove: (item: InboxItem, at: number) => void;
	/** Takes every item up to `through`, the newest listed, out of the list. */
	onClearAll: (through: number) => void;
};

/** The inbox dropdown's content: one dense line per item, as the Tasks home shows a task, unread first. */
export function InboxView({ items, rows, now, unread, onReadAll, onOpen, onRemove, onClearAll }: InboxProps) {
	return (
		<>
			<div class="pop-h">
				<span>Inbox</span>
				{unread > 0 ? (
					<button type="button" class="inbox-all" onClick={onReadAll}>
						Mark all read
					</button>
				) : (
					items.length > 0 && <span class="muted inbox-all">All read</span>
				)}
				{items.length > 0 && (
					<button type="button" class="inbox-all" onClick={() => onClearAll(Math.max(...items.map((i) => i.id)))}>
						Clear all
					</button>
				)}
			</div>
			{inboxOrder(items).map((item, at) => {
				const m = mark(item);
				const task = rows.find((r) => r.box === item.box);
				return (
					<div
						class="inbox-row"
						key={item.id}
						onKeyDown={(e) => {
							if (e.key !== 'Delete' && e.key !== 'Backspace') return;
							e.preventDefault();
							onRemove(item, at);
						}}
					>
					<a class={`inbox-item inbox-${item.state}`} href={`#/box/${encodeURIComponent(item.box)}`} onClick={(e) => onOpen(item, e)}>
						<span class={`t-glyph t-${m.tone}`} role="img" aria-label={m.word} title={m.word}>
							{m.glyph}
						</span>
						<RepoCell repos={task ? reposOf(task) : item.repo ? [item.repo] : []} only="" />
						<span class="inbox-title">{item.title}</span>
						<time class="inbox-time" dateTime={item.at}>
							{exactTime(item.at, now)}
						</time>
						<span class="inbox-now">
							{item.state === 'resolved' ? `Answered: ${item.text}` : ((task?.state === 'needs_you' && gateLine(task.gate)) || item.text)}
						</span>
					</a>
					<button type="button" class="inbox-rm" aria-label="Remove" title="Remove" onClick={() => onRemove(item, at)}>
						×
					</button>
					</div>
				);
			})}
			{items.length > 0 ? <p class="inbox-note muted">Removing an item does not change its task.</p> : <p class="t-empty">Nothing new.</p>}
		</>
	);
}
