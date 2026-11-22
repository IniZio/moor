import { useEffect, useRef, useState } from 'preact/hooks';
import * as v from 'valibot';

import { type Account, type BoxRow, boxStart, failure, type Node, repoList, toError } from './api';
import { exactTime } from './HomeList';
import { RepoPanel, RepoSlot, refocus, useDismiss } from './RepoMenu';
import { takeAgain } from './Retry';
import { useDraft } from './session';

/** Where the last start went, so the next one starts there. */
const LAST_REPOS = 'moor.lastRepos';

function lastRepos(): string[] {
	try {
		const read = v.safeParse(v.array(v.string()), JSON.parse(localStorage.getItem(LAST_REPOS) ?? '[]'));
		return read.success ? read.output : [];
	} catch {
		return [];
	}
}

export type Menu = '' | 'repo' | 'machine';

/** What the field shows. */
export type FieldState = {
	open: boolean;
	prompt: string;
	repos: string[];
	base: string;
	node: string;
	planFirst: boolean;
	busy: boolean;
	error: string;
	menu: Menu;
	machines: Node[];
	known: string[];
	rows: BoxRow[];
	now: number;
};

/** What a person can do in it. */
export type FieldActs = {
	open: () => void;
	close: () => void;
	typed: (text: string) => void;
	prompt: (text: string) => void;
	repos: (repos: string[]) => void;
	base: (base: string) => void;
	node: (node: string) => void;
	plan: (on: boolean) => void;
	menu: (menu: Menu) => void;
	start: () => void;
	settle: () => void;
};

const ready = (s: FieldState) => s.prompt.trim() !== '' && s.repos.length > 0 && !s.busy;

/** The machine a person may pick and what to say beside it. */
function machineNote(n: Node, now: number): string {
	if (n.online) return n.max_boxes != null ? `${Math.max(0, n.max_boxes - n.used)} free` : '';
	return n.offline_since ? `offline since ${exactTime(new Date(n.offline_since * 1000).toISOString(), now)}` : 'offline';
}

/** The machine menu without its hooks: any free machine, then the shared ones and the person's own. */
export function MachineMenuView({ nodes, value, now, onPick, onDone }: { nodes: Node[]; value: string; now: number; onPick: (node: string) => void; onDone: () => void }) {
	const group = (scope: Node['scope'], head: string) => {
		const own = nodes.filter((n) => n.scope === scope);
		if (!own.length) return null;
		return (
			<>
				<span class="t-mhd">{head}</span>
				{own.map((n) => (
					<button type="button" class="t-mitem" role="menuitemradio" aria-checked={value === n.name} disabled={!n.online} key={n.name} onClick={() => onPick(n.name)}>
						<span>{n.name}</span>
						<span class="t-msub">{machineNote(n, now)}</span>
					</button>
				))}
			</>
		);
	};
	return (
		<div
			class="t-pm"
			role="menu"
			aria-label="Machine"
			onKeyDown={(e) => {
				if (e.key === 'Escape') {
					e.stopPropagation();
					onDone();
				}
			}}
		>
			<button type="button" class="t-mitem" role="menuitemradio" aria-checked={value === ''} onClick={() => onPick('')}>
				<span>any free machine</span>
				<span class="t-msub">yours first, then shared</span>
			</button>
			{group('shared', 'Shared')}
			{group('personal', 'Yours')}
		</div>
	);
}

function MachinePanel({ nodes, value, now, onPick, onClose }: { nodes: Node[]; value: string; now: number; onPick: (node: string) => void; onClose: () => void }) {
	useDismiss('machine', onClose);
	return (
		<MachineMenuView
			nodes={nodes}
			value={value}
			now={now}
			onPick={(n) => {
				onPick(n);
				onClose();
				refocus('machine');
			}}
			onDone={() => {
				onClose();
				refocus('machine');
			}}
		/>
	);
}

/** "in acme/web · from main · on any free machine": each part opens its own control. */
export function TargetSentence({ s, act }: { s: FieldState; act: FieldActs }) {
	const flip = (m: Menu) => act.menu(s.menu === m ? '' : m);
	return (
		<p class="t-target">
			in{' '}
			<RepoSlot
				repos={s.repos}
				open={s.menu === 'repo'}
				onToggle={() => flip('repo')}
				panel={s.menu === 'repo' && <RepoPanel repos={s.repos} known={s.known} rows={s.rows} onChange={act.repos} onClose={() => act.menu('')} />}
			/>{' '}
			· from{' '}
			<input
				class="t-tw t-mono t-base"
				size={s.base ? s.base.length + 1 : 14}
				placeholder="default branch"
				aria-label="Branch to start from"
				spellcheck={false}
				value={s.base}
				onInput={(e) => act.base(e.currentTarget.value)}
			/>{' '}
			· on{' '}
			<span class="t-twwrap" data-menu="machine">
				<button type="button" class="t-tw t-mono" aria-haspopup="menu" aria-expanded={s.menu === 'machine'} onClick={() => flip('machine')}>
					{s.node || 'any free machine'}
				</button>
				{s.menu === 'machine' && <MachinePanel nodes={s.machines} value={s.node} now={s.now} onPick={act.node} onClose={() => act.menu('')} />}
			</span>
		</p>
	);
}

/** The write field without its hooks, so a test can read it: one line closed, the whole task form open. */
export function WriteFieldView({ s, act }: { s: FieldState; act: FieldActs }) {
	if (!s.open) {
		return (
			<div class="t-write">
				<input id="t-writeline" class="t-writeline" placeholder="Write a task…" aria-label="Write a task" autocomplete="off" value="" onFocus={act.open} onInput={(e) => act.typed(e.currentTarget.value)} />
			</div>
		);
	}
	return (
		<div class="t-write t-write-open">
			<form
				id="t-wform"
				class="t-wform"
				aria-label="New task"
				onSubmit={(e) => {
					e.preventDefault();
					if (ready(s)) act.start();
				}}
				onKeyDown={(e) => {
					if (e.key !== 'Escape') return;
					e.preventDefault();
					if (s.menu) act.menu('');
					else act.close();
				}}
				onFocusOut={(e) => {
					if (!(e.relatedTarget instanceof Node && e.currentTarget.contains(e.relatedTarget))) act.settle();
				}}
			>
				<label class="t-sr" for="t-prompt">
					What should the agent do?
				</label>
				<textarea
					id="t-prompt"
					rows={5}
					placeholder="What should the agent do?"
					value={s.prompt}
					onInput={(e) => act.prompt(e.currentTarget.value)}
					onKeyDown={(e) => {
						if ((e.metaKey || e.ctrlKey) && e.key === 'Enter' && ready(s)) act.start();
					}}
				/>
				{s.repos.length > 1 && (
					<p class="t-tnote muted">
						The agent starts in <span class="t-mono">{s.repos[0]}</span>. The others are cloned beside it.
					</p>
				)}
				<TargetSentence s={s} act={act} />
				<div class="t-wact">
					<label class="t-chip" title="The agent proposes a plan and waits for your approval before it changes anything.">
						<input type="checkbox" class="pf-check" checked={s.planFirst} onChange={(e) => act.plan(e.currentTarget.checked)} />
						Plan first
					</label>
					<span class="grow" />
					<span class="muted">Esc closes</span>
					<button class="btn primary" type="submit" disabled={!ready(s)}>
						{s.busy ? 'Starting…' : 'Start'} <kbd>{'⌘↵'}</kbd>
					</button>
				</div>
				{s.error && (
					<p class="notice danger" role="alert">
						{s.error}
					</p>
				)}
			</form>
		</div>
	);
}

const WRITE = 'moor:write';

/** The "+ New task" button: focus the field if the page has it, else open it by its old address. */
export function focusWrite() {
	if (document.getElementById('t-writeline') || document.getElementById('t-prompt')) dispatchEvent(new Event(WRITE));
	else location.hash = '#/new';
}

/** The write field on the Tasks home; `asked` is true at #/new, which opens it. */
export function WriteField({ account, rows, asked, onStarted }: { account: Account; rows: BoxRow[]; asked: boolean; onStarted: () => void }) {
	const [again] = useState(takeAgain);
	const [prompt, setPrompt, dropPrompt] = useDraft('new-task', again?.prompt);
	const [open, setOpen] = useState(asked || prompt.trim() !== '');
	const [repos, setRepos] = useState<string[]>(() => (again?.repos.length ? again.repos : lastRepos()));
	const [base, setBase] = useState(again?.base ?? '');
	const [node, setNode] = useState('');
	const [planFirst, setPlanFirst] = useState(false);
	const [known, setKnown] = useState<string[]>([]);
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState('');
	const [menu, setMenuState] = useState<Menu>('');
	const wantFocus = useRef(asked);
	const pressing = useRef(false);
	const live = useRef({ prompt, menu });
	live.current = { prompt, menu };
	useEffect(() => {
		repoList().then(setKnown, () => setKnown([]));
	}, []);
	const openUp = () => {
		wantFocus.current = true;
		setOpen(true);
	};
	useEffect(() => {
		addEventListener(WRITE, openUp);
		return () => removeEventListener(WRITE, openUp);
	}, []);
	const first = useRef(true);
	useEffect(() => {
		if (first.current) {
			first.current = false;
			return;
		}
		if (!asked) return;
		const next = takeAgain();
		if (next) {
			setPrompt(next.prompt);
			setRepos(next.repos);
			setBase(next.base);
		}
		openUp();
	}, [asked]);
	useEffect(() => {
		if (!open || !wantFocus.current) return;
		wantFocus.current = false;
		const box = document.getElementById('t-prompt') as HTMLTextAreaElement | null;
		box?.focus();
		box?.setSelectionRange(box.value.length, box.value.length);
	}, [open]);
	useEffect(() => {
		const up = () => {
			pressing.current = false;
		};
		addEventListener('mouseup', up);
		return () => removeEventListener('mouseup', up);
	}, []);

	const close = () => {
		setMenuState('');
		setOpen(false);
		if (asked) location.replace('#/');
	};
	// An empty form closes once focus has left it for good; a press inside it, or an open menu, is not leaving.
	const settle = () =>
		setTimeout(() => {
			const form = document.getElementById('t-wform');
			if (pressing.current || live.current.menu || live.current.prompt.trim() !== '' || !form || form.contains(document.activeElement)) return;
			setOpen(false);
		}, 0);
	const start = async () => {
		if (busy) return;
		setBusy(true);
		setError('');
		try {
			await boxStart(account.csrf, { repos, prompt, base, node, plan_first: planFirst });
			localStorage.setItem(LAST_REPOS, JSON.stringify(repos));
			dropPrompt();
			setBusy(false);
			close();
			onStarted();
		} catch (e) {
			setError(failure(toError(e)));
			setBusy(false);
		}
	};
	const act: FieldActs = {
		open: openUp,
		close,
		typed: (text) => {
			if (!prompt) setPrompt(text);
			openUp();
		},
		prompt: setPrompt,
		repos: setRepos,
		base: setBase,
		node: setNode,
		plan: setPlanFirst,
		menu: (m) => {
			live.current.menu = m;
			setMenuState(m);
			if (!m) settle();
		},
		start: () => void start(),
		settle,
	};
	const s: FieldState = { open, prompt, repos, base, node, planFirst, busy, error, menu, machines: account.machines.nodes, known, rows, now: Date.now() };
	return (
		<div onMouseDown={() => (pressing.current = true)}>
			<WriteFieldView s={s} act={act} />
		</div>
	);
}
