import { useEffect, useState } from 'preact/hooks';

import { boxChanges, type Changes, type Entry, type FileChange, type RepoChanges, failure, type Tally } from './api';

/** A file diff longer than this many lines starts folded. */
const FOLD_LINES = 400;

/** One file's part of a unified diff: its header lines and its hunks. */
type FilePatch = { head: string[]; body: string[] };

/** The path a file's diff header names: the new side, else the old one for a deleted file. */
function pathOf(head: string[]): string {
	const side = (prefix: string) => head.find((l) => l.startsWith(prefix))?.slice(prefix.length);
	const plus = side('+++ b/');
	if (plus) return plus;
	const renamed = side('rename to ');
	if (renamed) return renamed;
	const minus = side('--- a/');
	if (minus) return minus;
	const top = head[0] ?? '';
	return top.slice(top.lastIndexOf(' b/') + 3);
}

/** Splits a unified diff into one part per file, keyed by path. */
export function splitPatch(patch: string): Map<string, FilePatch> {
	const out = new Map<string, FilePatch>();
	let cur: FilePatch | null = null;
	const done = () => cur && out.set(pathOf(cur.head), cur);
	for (const line of patch.split('\n')) {
		if (line.startsWith('diff --git ')) {
			done();
			cur = { head: [line], body: [] };
		} else if (cur && (cur.body.length || line.startsWith('@@'))) {
			cur.body.push(line);
		} else if (cur) {
			cur.head.push(line);
		}
	}
	done();
	return out;
}

/** "+3 −1", green and red as the spec allows for diffs. */
export function Counts({ added, removed }: { added: number | null; removed: number | null }) {
	if (added === null || removed === null) return <span class="t-tag">binary</span>;
	return (
		<span class="t-counts">
			{added > 0 && <span class="t-add">+{added}</span>}
			{added > 0 && removed > 0 && ' '}
			{removed > 0 && <span class="t-del">−{removed}</span>}
		</span>
	);
}

function files(n: number): string {
	return `${n} file${n === 1 ? '' : 's'}`;
}

/** The tab label's "+N −M": what the last receipt saw against the base. */
export function changesLabel(entries: Entry[]) {
	const last = [...entries].reverse().find((e) => e.receipt)?.receipt;
	if (!last || !last.total.files) return 'Changes';
	return (
		<>
			Changes <Counts added={last.total.added} removed={last.total.removed} />
		</>
	);
}

function took(from: string | undefined, to: string): string {
	const ms = from ? Date.parse(to) - Date.parse(from) : NaN;
	if (!(ms >= 0)) return '';
	const s = Math.round(ms / 1000);
	return s < 60 ? ` · ${s}s` : ` · ${Math.floor(s / 60)}m ${s % 60}s`;
}

/** A turn's end, with what it changed in git; the receipt opens the Changes tab. */
export function TurnLine({ e, entries, href }: { e: Entry; entries: Entry[]; href: string }) {
	const r = e.receipt;
	if (!r) return <div class="t-receipt">{e.text}</div>;
	const at = entries.indexOf(e);
	const asked = entries.slice(0, at).reverse().find((x) => x.class === 'you')?.at;
	const changed: Tally = r.changed;
	return (
		<a class="t-receipt t-receipt-link" href={href} title="Open the changes">
			<span>
				turn {r.turn} · {changed.files ? files(changed.files) : 'no changes'}{' '}
				{changed.files > 0 && <Counts added={changed.added} removed={changed.removed} />}
				{e.kind === 'turn_done' && took(asked, e.at)}
			</span>
		</a>
	);
}

function Line({ line, nums }: { line: string; nums: [number | null, number | null] }) {
	const kind = line.startsWith('@@') ? 'hunk' : line.startsWith('+') ? 'add' : line.startsWith('-') ? 'del' : line.startsWith('\\') ? 'note' : 'ctx';
	return (
		<div class={`t-dl t-dl-${kind}`}>
			<span class="t-ln">{nums[0] ?? ''}</span>
			<span class="t-ln">{nums[1] ?? ''}</span>
			<span class="t-dt">{line || ' '}</span>
		</div>
	);
}

/** Each line of a file's hunks with its old and new line numbers. */
function numbered(body: string[]): { line: string; nums: [number | null, number | null] }[] {
	let [a, b] = [0, 0];
	return body.map((line) => {
		const hunk = /^@@ -(\d+)(?:,\d+)? \+(\d+)/.exec(line);
		if (hunk) {
			[a, b] = [Number(hunk[1]), Number(hunk[2])];
			return { line, nums: [null, null] };
		}
		if (line.startsWith('+')) return { line, nums: [null, b++] };
		if (line.startsWith('-')) return { line, nums: [a++, null] };
		if (line.startsWith('\\')) return { line, nums: [null, null] };
		return { line, nums: [a++, b++] };
	});
}

function FileDiff({ f, part, id }: { f: FileChange; part: FilePatch | undefined; id: string }) {
	const body = part?.body.filter((l, i, all) => l !== '' || i < all.length - 1) ?? [];
	const fresh = part?.head.some((l) => l.startsWith('new file mode'));
	const gone = part?.head.some((l) => l.startsWith('deleted file mode'));
	return (
		<details class="t-file-diff" id={id} open={body.length <= FOLD_LINES}>
			<summary>
				<span class="t-chev">▸</span>
				<span class="t-mono t-path">{f.path}</span>
				{f.from && <span class="t-tag">renamed from {f.from}</span>}
				{fresh && <span class="t-tag">new</span>}
				{gone && <span class="t-tag">deleted</span>}
				<span class="grow" />
				<Counts added={f.added} removed={f.removed} />
			</summary>
			{f.added === null ? (
				<p class="t-dnote muted">Binary file, not shown.</p>
			) : !part ? (
				<p class="t-dnote muted">Past the size cut, not shown.</p>
			) : body.length ? (
				<div class="t-diff">
					{numbered(body).map((l, i) => (
						<Line line={l.line} nums={l.nums} key={i} />
					))}
				</div>
			) : (
				<p class="t-dnote muted">Only the name or mode changed.</p>
			)}
		</details>
	);
}

function Repo({ r, many, n }: { r: RepoChanges; many: boolean; n: number }) {
	const parts = r.patch != null ? splitPatch(r.patch) : null;
	const anchor = (i: number) => `t-f${n}-${i}`;
	return (
		<section class="t-repo-changes">
			{many && <h3 class="t-mono">{r.repo}</h3>}
			{r.problem && <p class="notice danger">{r.problem}</p>}
			{!r.problem && !r.files.length && <p class="t-empty">No changes against the base.</p>}
			{r.files.length > 0 && (
				<ul class="t-files">
					{r.files.map((f, i) => (
						<li key={f.path}>
							{parts ? <a class="t-mono t-path" href={`#${anchor(i)}`} onClick={(ev) => { ev.preventDefault(); document.getElementById(anchor(i))?.scrollIntoView(); }}>{f.path}</a> : <span class="t-mono t-path">{f.path}</span>}
							{f.from && <span class="t-tag">renamed</span>}
							<span class="grow" />
							<Counts added={f.added} removed={f.removed} />
						</li>
					))}
					{r.total.files > r.files.length && <li class="muted">and {r.total.files - r.files.length} more files</li>}
				</ul>
			)}
			{r.cut && <p class="t-stalebar">Diff too large: it passed 1 MB, so it stops there. Files past the cut show their counts only.</p>}
			{parts && r.files.map((f, i) => <FileDiff f={f} part={parts.get(f.path)} id={anchor(i)} key={f.path} />)}
		</section>
	);
}

/** The Box's diff against its base, file by file, each repo first one first. */
export function ChangesTab({ box, seen }: { box: string; seen: string }) {
	const [changes, setChanges] = useState<Changes | null>(null);
	const [error, setError] = useState('');
	const [round, setRound] = useState(0);
	useEffect(() => {
		let live = true;
		boxChanges(box).then(
			(c) => live && (setChanges(c), setError('')),
			(e: Error) => live && setError(failure(e)),
		);
		return () => {
			live = false;
		};
	}, [box, seen, round]);
	if (!changes) return error ? <p class="notice danger" role="alert">{error}</p> : <p class="muted" aria-busy="true">Reading the Box's changes…</p>;
	const total = changes.repos.reduce((t, r) => ({ files: t.files + r.total.files, added: t.added + r.total.added, removed: t.removed + r.total.removed }), { files: 0, added: 0, removed: 0 });
	const at = changes.at ? new Date(changes.at).toLocaleString([], { dateStyle: 'medium', timeStyle: 'short' }) : '';
	return (
		<div class="t-changes">
			{error && <p class="notice danger" role="alert">{error}</p>}
			{changes.live ? (
				<div class="t-term-bar">
					<span class="t-term-state t-term-live">Live</span>
					<span class="muted">
						{files(total.files)} <Counts added={total.added} removed={total.removed} /> against the base, committed and uncommitted
					</span>
					<span class="grow" />
					<button type="button" class="btn" onClick={() => setRound((n) => n + 1)}>
						Refresh
					</button>
				</div>
			) : (
				<p class="muted t-changes-old">
					{changes.repos.length ? (at ? `as of ${at}` : 'as of its last turn') : 'No turn has ended with changes on record yet.'}
				</p>
			)}
			{changes.repos.map((r, n) => (
				<Repo r={r} many={changes.repos.length > 1} n={n} key={r.repo} />
			))}
		</div>
	);
}
