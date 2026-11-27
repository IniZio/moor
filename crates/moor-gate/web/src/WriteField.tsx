import { useEffect, useRef, useState } from 'preact/hooks';
import * as v from 'valibot';

import { type Account, type BoxRow, boxStart, failure, repoList, toError } from './api';
import { RepoPanel, RepoSlot, refocus, useDismiss } from './RepoMenu';
import { recentRepos } from './repoSets';
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

export type Menu = '' | 'repo';

/** What the field shows. */
export type FieldState = {
	open: boolean;
	prompt: string;
	repos: string[];
	busy: boolean;
	error: string;
	menu: Menu;
	known: string[];
	rows: BoxRow[];
};

/** What a person can do in it. */
export type FieldActs = {
	open: () => void;
	example: (key: keyof typeof EXAMPLES) => void;
	close: () => void;
	typed: (text: string) => void;
	prompt: (text: string) => void;
	repos: (repos: string[]) => void;
	menu: (menu: Menu) => void;
	start: () => void;
	settle: () => void;
};

/** Prompts a person can start from, as the mock writes them; {repo} becomes the first repo picked. */
export const EXAMPLES = {
	bug: {
		label: 'bug fix with screenshots',
		text: 'Fix the cropped avatar on the profile page\nOn the profile page the avatar is cut off at the top on phones. Make the whole picture show, on phones and on desktop.\nTake a screenshot of the profile page before and after, named before.png and after.png, at 390 wide.\nMake sure the tests still pass.\nOpen one pull request on {repo} and check with me.\nIf the avatar already shows in full, stop and say so.\nKeep the fix small. Don’t change the avatar upload code.',
	},
	pr: {
		label: 'pull request evidence',
		text: 'Check the evidence on pull request #12\nRead what pull request #12 on {repo} says it fixes. Run the end-to-end tests on main and on its branch. Save both results as e2e-main.txt and e2e-branch.txt.\nCompare them test by test and say whether the branch is worse.\nPost one comment on the pull request with what you found.\nIf the pull request is closed or still a draft, there is nothing to do. Do not push anything.',
	},
};

/** What stops a start, in the words the field shows; none when it can start. */
export function blocker(s: Pick<FieldState, 'prompt' | 'repos'>): string | null {
	if (!s.prompt.trim()) return 'Write what the agent should do first.';
	return s.repos.length ? null : 'Pick a repo first.';
}

/** "in acme/web": the repos open their menu. The agent's own words carry anything else, such as a branch to start from. */
export function TargetSentence({ s, act }: { s: FieldState; act: FieldActs }) {
	const repo = (
		<RepoSlot
			repos={s.repos}
			open={s.menu === 'repo'}
			onToggle={() => act.menu(s.menu === 'repo' ? '' : 'repo')}
			panel={s.menu === 'repo' && <RepoPanel repos={s.repos} known={s.known} rows={s.rows} onChange={act.repos} onClose={() => act.menu('')} />}
		/>
	);
	return <p class="t-target">in {repo}</p>;
}

/** The write field without its hooks, so a test can read it: one line closed, the whole task form open. */
export function WriteFieldView({ s, act }: { s: FieldState; act: FieldActs }) {
	if (!s.open) {
		return (
			<div class="t-write">
				<input id="t-writeline" class="t-writeline" placeholder="Write a task, or paste an issue…" aria-label="Write a task, or paste an issue" autocomplete="off" value="" onFocus={act.open} onInput={(e) => act.typed(e.currentTarget.value)} />
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
					act.start();
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
				<div class="t-whead">
					<span class="grow" />
					<span class="muted t-small">Try an example:</span>{' '}
					<button type="button" class="t-lnk" onClick={() => act.example('bug')}>
						{EXAMPLES.bug.label}
					</button>{' '}
					·{' '}
					<button type="button" class="t-lnk" onClick={() => act.example('pr')}>
						{EXAMPLES.pr.label}
					</button>
				</div>
				<label class="t-sr" for="t-prompt">
					What should the agent do?
				</label>
				<textarea
					id="t-prompt"
					rows={1}
					placeholder="What should the agent do? The first line is the title."
					value={s.prompt}
					onInput={(e) => act.prompt(e.currentTarget.value)}
					onKeyDown={(e) => {
						if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') act.start();
					}}
				/>
				{s.repos.length > 1 && (
					<p class="t-tnote muted">
						The agent starts in <span class="t-mono">{s.repos[0]}</span>. The others are cloned beside it.
					</p>
				)}
				<TargetSentence s={s} act={act} />
				<div class="t-wact">
					<button class="btn primary" type="submit" disabled={s.busy}>
						{s.busy ? 'Starting…' : 'Start'}
					</button>
					<span class="grow" />
					<span class="muted t-wesc">{s.prompt.trim() ? 'Esc keeps the draft' : 'Esc to close'}</span>
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
	const chosen = useRef(repos.length > 0);
	useEffect(() => {
		if (chosen.current) return;
		const last = recentRepos(rows, [])[0];
		if (last) setRepos([last]);
	}, [rows]);
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
		const stop = blocker({ prompt, repos });
		if (stop) {
			setError(stop);
			document.getElementById('t-prompt')?.focus();
			return;
		}
		setBusy(true);
		setError('');
		try {
			const started = await boxStart(account.csrf, { repos, prompt });
			localStorage.setItem(LAST_REPOS, JSON.stringify(repos));
			dropPrompt();
			setBusy(false);
			close();
			onStarted();
			location.hash = `#/box/${encodeURIComponent(started)}`;
		} catch (e) {
			setError(failure(toError(e)));
			setBusy(false);
		}
	};
	const act: FieldActs = {
		open: openUp,
		example: (key) => {
			setPrompt(EXAMPLES[key].text.replaceAll('{repo}', repos[0] ?? 'the repo'));
			setError('');
			document.getElementById('t-prompt')?.focus();
		},
		close,
		typed: (text) => {
			if (!prompt) setPrompt(text);
			openUp();
		},
		prompt: setPrompt,
		repos: (next) => {
			chosen.current = true;
			setRepos(next);
		},
		menu: (m) => {
			live.current.menu = m;
			setMenuState(m);
			if (!m) settle();
		},
		start: () => void start(),
		settle,
	};
	const s: FieldState = { open, prompt, repos, busy, error, menu, known, rows };
	return (
		<div onMouseDown={() => (pressing.current = true)}>
			<WriteFieldView s={s} act={act} />
		</div>
	);
}
