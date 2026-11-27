import { useEffect, useRef, useState } from 'preact/hooks';

import { type BoxDetail, Conflict, type RepoConfig, type RulesSeen, boxGet, repoGet, repoList, repoSet, failure, rulesAsk, toError } from './api';
import { phaseOf } from './AutomationBox';
import { useMenuDismiss } from './HomeList';
import { type DiffLine, diffLines, litRequests, proposalOf, scrubbed, textLines, unredact } from './rulesDiff';
import './rules.css';

/** The repo a `#/repos/owner/name/rules` address names; null for any other page. */
export function rulesAt(hash: string): string | null {
	const m = /^#?\/?repos\/([^/]+\/[^/]+)\/rules$/.exec(hash);
	return m ? decodeURI(m[1]) : null;
}

/** Where the rules page of `repo` is. */
export function rulesHref(repo: string): string {
	return `#/repos/${repo}/rules`;
}

/** The line under the repo's name: who shares the rules, and which revision they are at. */
export function rulesLine(rules: RepoConfig): string {
	const shared = `Shared by everyone who works on ${rules.repo}`;
	if (rules.revision === 0) return `${shared} · the default rules`;
	const day = rules.edited_at ? new Date(rules.edited_at) : null;
	const on = day && !Number.isNaN(day.getTime()) ? ` ${day.toLocaleDateString('en-GB', { day: 'numeric', month: 'short' })}` : '';
	const by = rules.edited_by ? ` · edited by ${rules.edited_by}${on}` : '';
	return `${shared} · revision ${rules.revision}${by}`;
}

/** The line numbers beside `text`, one per line; a closing newline starts no line of its own. */
export function lineNumbers(text: string): string {
	const lines = text.endsWith('\n') ? text.slice(0, -1).split('\n') : text.split('\n');
	return lines.map((_, i) => i + 1).join('\n');
}

/** The rules after a save: the written text, with the lists the page read, since a write does not list them. */
export function afterSave(read: RepoConfig, wrote: RepoConfig): RepoConfig {
	return { ...wrote, seen: wrote.seen ?? read.seen };
}

/** A host as the lists show it: the port only when it is not 443. */
export function hostPort(host: string, port: number): string {
	return port === 443 || port === 0 ? host : `${host}:${port}`;
}

/** What the repo's Boxes met under the rules: requests sent on without the secret, and hosts reached without the Proxy. */
export function Seen({ seen, lit, onPick }: { seen: RulesSeen; lit?: ReadonlySet<string>; onPick?: (w: RulesSeen['withheld'][number]) => void }) {
	return (
		<aside class="rules-seen">
			<h3 title="A Box reports each request once, so the number counts Boxes.">Withheld in the last {seen.days} days</h3>
			{seen.withheld.length ? (
				<ul class="seen-rows">
					{seen.withheld.map((w) => (
						<li key={`${w.request} ${w.host}`} class={lit?.has(w.request) ? 'lit' : undefined}>
							<button type="button" class="seen-row" onClick={() => onPick?.(w)}>
								<code>{w.request}</code>
								<code class="host">{w.host}</code>
								<span class="seen-n" title="Boxes that reported it">{w.boxes}</span>
							</button>
						</li>
					))}
				</ul>
			) : (
				<p class="muted">Nothing was withheld.</p>
			)}
			<h3 title="Docker Boxes only. An address is a connection that skipped the Box's own proxy.">Dialled directly</h3>
			{seen.dialled.length ? (
				<ul class="seen-rows">
					{seen.dialled.map((d) => (
						<li key={hostPort(d.host, d.port)} class="seen-row dim">
							<code>{hostPort(d.host, d.port)}</code>
							<span class="seen-n">{d.count}</span>
						</li>
					))}
				</ul>
			) : (
				<p class="muted">No host yet.</p>
			)}
		</aside>
	);
}

/** What the agent has said since the task was asked: nothing yet, an edit of the rules, or only words. */
export type Reply = { at: 'waiting' } | { at: 'proposed'; note: string; text: string } | { at: 'answered'; note: string };

/** The agent's reply in its place's task `detail`: the newest message with a yaml block, or once the agent stopped, its last words. */
export function replyOf(detail: Pick<BoxDetail, 'timeline' | 'task_at' | 'state'>, running: boolean): Reply {
	const since = detail.task_at ? Date.parse(detail.task_at) : Number.NEGATIVE_INFINITY;
	const said = detail.timeline.filter((e) => e.class === 'message' && !(Date.parse(e.at) < since));
	for (const e of [...said].reverse()) {
		const p = proposalOf(e.text);
		if (p) return { at: 'proposed', ...p };
	}
	const last = said.at(-1);
	return !running && last ? { at: 'answered', note: last.text } : { at: 'waiting' };
}

/** The rules as the change leaves them, numbered, with the lines it added lit and the ones it took out struck. */
export function RulesDiff({ lines }: { lines: DiffLine[] }) {
	let n = 0;
	return (
		<div class="rules-diff" aria-label="The rules with the change lit">
			{lines.map((l, i) => (
				<div key={i} class={`dl ${l.kind}`}>
					<span class="dn">{l.kind === 'del' ? '' : ++n}</span>
					<span class="dt">{l.text || ' '}</span>
					{l.kind !== 'same' && <span class="dtag">{l.kind === 'add' ? 'new' : 'removed'}</span>}
				</div>
			))}
		</div>
	);
}

type Ask = { at: 'idle' } | { at: 'sending' } | { at: 'waiting'; box: string; base: string } | { at: 'answered'; box: string; note: string } | { at: 'failed'; why: string };

/** How often the page looks at the agent's place while it works. */
const ASK_POLL_MS = 1500;

/** A repo's Proxy rules on their own page: the stored text, edited by hand or by the agent, read as a diff, then saved. */
/** The repo's name as the heading; with other repos to go to, it opens a menu of their rules pages. */
function RepoSwitch({ repo }: { repo: string }) {
	const [repos, setRepos] = useState<string[]>([]);
	useEffect(() => void repoList().then(setRepos, () => setRepos([])), []);
	useMenuDismiss();
	if (!repos.some((r) => r !== repo)) return <h1>{repo}</h1>;
	return (
		<h1>
			<details class="t-rmenu rules-switch" key={repo}>
				<summary>
					{repo} <span class="rules-caret">▾</span>
				</summary>
				<div class="t-rmenu-pop">
					{[repo, ...repos.filter((r) => r !== repo)].map((r) => (
						<a href={rulesHref(r)} aria-current={r === repo ? 'true' : undefined} key={r}>
							{r}
						</a>
					))}
				</div>
			</details>
		</h1>
	);
}

export function RulesPage({ csrf, repo }: { csrf: string; repo: string }) {
	const [rules, setRules] = useState<RepoConfig | null>(null);
	const [draft, setDraft] = useState('');
	const [read, setRead] = useState<string | null>(null);
	const [loadError, setLoadError] = useState('');
	const [busy, setBusy] = useState(false);
	const [problem, setProblem] = useState<{ text: string; stale: boolean } | null>(null);
	const [words, setWords] = useState('');
	const [ask, setAsk] = useState<Ask>({ at: 'idle' });
	const writeBox = useRef<HTMLTextAreaElement>(null);

	const load = async () => {
		setProblem(null);
		setLoadError('');
		try {
			const got = await repoGet(repo, 'rules');
			setRules(got);
			setDraft(got.text ?? '');
			setRead(null);
		} catch (err) {
			setLoadError(failure(toError(err)));
		}
	};
	useEffect(() => {
		setRules(null);
		void load();
	}, [repo]);

	useEffect(() => {
		if (ask.at !== 'waiting') return;
		let live = true;
		const look = async () => {
			try {
				const detail = await boxGet(ask.box);
				const reply = replyOf(detail, ['working', 'needs_you'].includes(detail.state));
				if (!live || reply.at === 'waiting') return;
				if (reply.at === 'proposed') {
					const text = unredact(ask.base, reply.text);
					setDraft(text);
					setRead(text);
				}
				setAsk({ at: 'answered', box: ask.box, note: reply.note });
			} catch {
				// The next look tries again; the link to the task stays.
			}
		};
		const timer = setInterval(() => void look(), ASK_POLL_MS);
		void look();
		return () => {
			live = false;
			clearInterval(timer);
		};
	}, [ask]);

	if (loadError) {
		return (
			<section class="rules-page">
				<p class="notice danger" role="alert">{loadError}</p>
				<button onClick={load}>Try again</button>
			</section>
		);
	}
	if (!rules) {
		return (
			<section class="rules-page">
				<p class="muted" aria-busy="true">Loading the rules of {repo}…</p>
			</section>
		);
	}

	const saved = rules.text ?? '';
	const phase = phaseOf(saved, draft, read);
	const hidden = scrubbed(draft);
	const save = async (e: Event) => {
		e.preventDefault();
		if (phase !== 'read' || hidden) return;
		setBusy(true);
		setProblem(null);
		try {
			const wrote = await repoSet(csrf, repo, draft, rules.revision, 'rules');
			setRules(afterSave(rules, wrote));
			setDraft(wrote.text ?? draft);
			setRead(null);
		} catch (err) {
			setProblem({ text: failure(toError(err)), stale: err instanceof Conflict });
		}
		setBusy(false);
	};
	const discard = () => {
		setProblem(null);
		setDraft(saved);
		setRead(null);
	};
	const send = async (e: Event) => {
		e.preventDefault();
		const said = words.trim();
		if (!said) return;
		setAsk({ at: 'sending' });
		const withheld = rules.seen?.withheld.map((w) => `${w.request} ${w.host}`) ?? [];
		try {
			const sent = await rulesAsk(csrf, repo, { words: said, text: draft, withheld });
			setWords('');
			setAsk({ at: 'waiting', box: sent.box, base: draft });
		} catch (err) {
			setAsk({ at: 'failed', why: failure(toError(err)) });
		}
	};
	const editable = rules.can_edit;
	const numbers = lineNumbers(draft);
	const lines = phase === 'read' ? diffLines(textLines(saved), textLines(draft)) : null;
	const lit = lines && rules.seen ? litRequests(lines.filter((l) => l.kind === 'add').map((l) => l.text), rules.seen.withheld.map((w) => w.request), repo) : undefined;
	const pick = (w: { request: string; host: string }) => {
		if (!editable || ask.at === 'sending' || ask.at === 'waiting') return;
		setWords(`Let ${w.request} through to ${w.host}.`);
		writeBox.current?.focus();
	};
	return (
		<section class="rules-page">
			<RepoSwitch repo={rules.repo} />
			<p class="rules-rev">
				{rulesLine(rules)}
				<span class="grow" />
				<a href="#/intents">Intent table</a>
			</p>
			{!editable && (
				<p class="notice warn" role="status">
					Your GitHub account cannot push to this repo, so you can read its rules here and not change them.
				</p>
			)}
			<div class="rules-grid">
				<form class="rules-file" onSubmit={save}>
					<div class="rules-head">rules.yaml</div>
					{phase === 'read' ? (
						<RulesDiff lines={lines ?? []} />
					) : (
						<div class={phase === 'writing' ? 'rules-body dim' : 'rules-body'}>
							<pre class="rules-gutter" aria-hidden="true">{numbers}</pre>
							<textarea
								aria-label={`Rules of ${rules.repo}`}
								spellcheck={false}
								wrap="off"
								readOnly={!editable}
								rows={numbers.split('\n').length + 1}
								value={draft}
								onInput={(e) => setDraft(e.currentTarget.value)}
							/>
						</div>
					)}
					{hidden && (
						<p class="notice danger rules-problem" role="alert">
							A line holds [redacted], where moor hid a secret-like part of the agent's words. Edit that line before you save.
						</p>
					)}
					{problem?.text && <pre class="notice danger rules-problem" role="alert">{problem.text}</pre>}
					{editable && (phase !== 'saved' || problem?.stale) && (
						<div class="rules-foot">
							{phase === 'read' ? (
								<button class="primary" disabled={busy || hidden}>Save</button>
							) : (
								<button type="button" class="primary" disabled={busy || phase === 'saved'} onClick={() => setRead(draft)}>
									Read it
								</button>
							)}
							{phase === 'read' && (
								<button type="button" disabled={busy} onClick={() => setRead(null)}>
									Edit
								</button>
							)}
							<button type="button" disabled={busy} onClick={problem?.stale ? load : discard}>
								{problem?.stale ? 'Discard mine and load theirs' : 'Discard'}
							</button>
						</div>
					)}
				</form>
				{rules.seen && <Seen seen={rules.seen} lit={lit} onPick={pick} />}
			</div>
			<div class="rules-dock">
				<AgentLine ask={ask} />
				{editable && (
					<form class="rules-ask" onSubmit={send}>
						<textarea
							ref={writeBox}
							rows={1}
							aria-label="Write to the agent"
							placeholder="Write to the agent…"
							value={words}
							disabled={ask.at === 'sending' || ask.at === 'waiting'}
							onInput={(e) => setWords(e.currentTarget.value)}
							onKeyDown={(e) => {
								if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
									e.preventDefault();
									e.currentTarget.form?.requestSubmit();
								}
							}}
						/>
						<button class="primary" disabled={ask.at === 'sending' || ask.at === 'waiting'}>
							Send
						</button>
					</form>
				)}
			</div>
		</section>
	);
}

/** `text` with each \`quoted\` part set as code, the way the agent marks a rule line. */
export function spans(text: string) {
	return text.split('`').map((part, i) => (i % 2 ? <code key={i}>{part}</code> : part));
}

/** The line under the file about the agent: working on it, what it said, or why the send failed. */
function AgentLine({ ask }: { ask: Ask }) {
	if (ask.at === 'idle') return null;
	if (ask.at === 'failed') return <p class="notice danger" role="alert">{ask.why}</p>;
	const task = ask.at === 'sending' ? null : <a href={`#/box/${encodeURIComponent(ask.box)}`}>Open the task</a>;
	const said =
		ask.at === 'answered' ? ask.note : 'The agent is reading the rules…';
	return (
		<p class="rules-agent" role="status" aria-busy={ask.at !== 'answered'}>
			<span class="rules-mark" aria-hidden="true">◆</span>
			<span>{spans(said)}</span>
			{task}
		</p>
	);
}
