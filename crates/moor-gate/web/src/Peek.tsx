import { useEffect } from 'preact/hooks';

import { type BoxDetail, boxGet, type BoxRow, type Evidence } from './api';
import { type ApproveProps, ApproveView, StuckView, useApproval } from './Gates';
import { gateLine, type Peekable, peekable } from './gate';
import { exactTime, RepoCell, reposOf, Source } from './HomeList';
import { peekOf, setPeek } from './hashQuery';
import { useLoad } from './useLoad';
import './peek.css';

/** The newest version of each image the agent handed in, as the detail lists them. */
export function handedImages(names: readonly string[], detail: Pick<BoxDetail, 'artifacts'> | null): Evidence[] {
	const newest = new Map<string, Evidence>();
	for (const a of detail?.artifacts ?? []) {
		if (a.image && names.includes(a.name) && a.version >= (newest.get(a.name)?.version ?? 0)) newest.set(a.name, a);
	}
	return [...newest.values()];
}

/** The panel's content without its hooks: where the task is and what it says, then the same approval or stuck card the task page shows. */
export function PeekView({ t, now, approve, onClose }: { t: Peekable; now: number; approve: ApproveProps; onClose: () => void }) {
	const wait = t.needs.gate;
	return (
		<aside class={`t-peek${wait.kind === 'approve' ? '' : ' t-peek-stuck'}`} aria-label="Peek">
			<div class="t-peek-head">
				<RepoCell repos={reposOf(t)} only="" />
				<Source t={t} />
				<span class="grow" />
				<a class="btn" href={`#/box/${encodeURIComponent(t.box)}`}>
					Open task
				</a>
				<button type="button" class="btn ghost" aria-label="Close" onClick={onClose}>
					✕
				</button>
			</div>
			<h2 class="t-peek-title">{t.title}</h2>
			<p class="t-peek-line">
				<span class="t-peek-now">{gateLine(t.gate) ?? t.line}</span> · <time dateTime={t.last_event_at ?? t.updated_at}>{exactTime(t.last_event_at ?? t.updated_at, now)}</time>
			</p>
			{wait.kind === 'approve' ? <ApproveView {...approve} /> : <StuckView wait={wait} inPeek />}
			<p class="muted t-peek-hint">Esc closes · j and k move through the list</p>
		</aside>
	);
}

/** One open panel. The detail is read once it opens, only for the thumbnails; the card itself comes from the row. */
function PeekPanel({ t, now, csrf, onSent, onClose }: { t: Peekable; now: number; csrf: string; onSent: () => void; onClose: () => void }) {
	const { value } = useLoad(() => boxGet(t.box), t.box);
	const approve = useApproval(t.box, t.needs, t.needs.gate, csrf, onSent, handedImages(t.needs.gate.handed, value), t.pr?.number ?? null);
	return <PeekView t={t} now={now} approve={approve} onClose={onClose} />;
}

const typing = (el: Element | null) => el instanceof HTMLElement && (el.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(el.tagName));

const rowsOnPage = () => [...document.querySelectorAll<HTMLElement>('[data-row]')];

/** j and k move between rows, Space or the right arrow or a click on "peek" opens a row that needs you, and Esc closes the panel. */
function usePeekControls() {
	useEffect(() => {
		const onKey = (e: KeyboardEvent) => {
			const open = peekOf(location.hash);
			const at = document.activeElement;
			if (e.key === 'Escape') {
				const busy = document.querySelector('dialog[open], :popover-open, details.t-rmenu[open]');
				if (!open || e.defaultPrevented || busy || at?.closest('#t-wform')) return;
				setPeek('');
				rowsOnPage().find((r) => r.dataset.box === open)?.focus();
				return;
			}
			if (e.metaKey || e.ctrlKey || e.altKey || typing(at) || document.querySelector('dialog[open]')) return;
			if (e.key === 'j' || e.key === 'k') {
				const rows = rowsOnPage();
				if (!rows.length) return;
				let from = rows.indexOf(at as HTMLElement);
				if (from < 0 && open) from = rows.findIndex((r) => r.dataset.box === open);
				const next = rows[e.key === 'j' ? Math.min(rows.length - 1, from + 1) : Math.max(0, from - 1)];
				next?.focus();
				if (open && next) setPeek(next.dataset.peek ?? '');
			} else if ((e.key === ' ' || e.key === 'ArrowRight') && at instanceof HTMLElement && at.dataset.peek) {
				e.preventDefault();
				setPeek(at.dataset.peek);
			}
		};
		const onClick = (e: MouseEvent) => {
			const button = e.target instanceof Element ? e.target.closest<HTMLElement>('[data-peekbtn]') : null;
			if (!button) return;
			e.preventDefault();
			setPeek(button.dataset.peekbtn ?? '');
			button.closest<HTMLElement>('[data-row]')?.focus();
		};
		addEventListener('keydown', onKey);
		addEventListener('click', onClick);
		return () => {
			removeEventListener('keydown', onKey);
			removeEventListener('click', onClick);
		};
	}, []);
}

/** The peek panel of the Tasks home: the open row from the address, while it still needs the person. */
export function Peek({ rows, now, hash, csrf, onSent }: { rows: BoxRow[] | null; now: number; hash: string; csrf: string; onSent: () => void }) {
	usePeekControls();
	const open = peekOf(hash);
	const t = rows?.find((r) => r.box === open);
	const live = t && peekable(t) ? t : null;
	useEffect(() => {
		if (open && rows && !live) setPeek('');
	}, [open, rows, live]);
	return live ? <PeekPanel key={`${live.box}:${live.needs.id}`} t={live} now={now} csrf={csrf} onSent={onSent} onClose={() => setPeek('')} /> : null;
}
