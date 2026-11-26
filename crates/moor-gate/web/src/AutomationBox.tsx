import type { Workflow } from './api';
import { AutomationFile } from './AutomationFile';
import { fileLines, type Hover, litOf, marksOf } from './promptFile';
import { PromptMarks } from './PromptMarks';
import { RepoName } from './HomeList';
import { firesOn } from './schedule';

/** What fires it, on which repos, and who each run acts as, marked unless the person is writing; the zone and the next run sit in the hover title. */
function Trigger({ flow, marked }: { flow: Workflow; marked: boolean }) {
	const mark = marked ? 'psp k-enf' : undefined;
	const words = firesOn(flow);
	const next = flow.enabled && flow.next_text ? `next run ${flow.next_text}` : null;
	const title = flow.cron == null ? undefined : [flow.cron, flow.timezone ?? 'UTC', next].filter(Boolean).join(' · ');
	return (
		<p class="trig">
			<span class={mark} title={title}>
				{words}
			</span>
			{flow.cron != null && flow.on_pull_request && (
				<>
					{' and '}
					<span class={mark}>on every pull request</span>
				</>
			)}
			{' · on '}
			<span class={mark}>{flow.repos.join(', ')}</span>
			{flow.run_as && (
				<>
					{' · acts as '}
					<span class={mark}>{flow.run_as}</span>
				</>
			)}
			{!flow.enabled && ' · disabled'}
		</p>
	);
}

/** Puts the caret at the end of the prompt, ready to write. */
function editPrompt(e: { currentTarget: HTMLElement }) {
	const box = e.currentTarget.closest('form')?.querySelector<HTMLTextAreaElement>('textarea.promptbox');
	box?.focus();
	box?.setSelectionRange(box.value.length, box.value.length);
}

/** Where an edit of the prompt stands: as saved, being written, or read and ready to save. */
export type Phase = 'saved' | 'writing' | 'read';

export function phaseOf(saved: string, prompt: string, read: string | null): Phase {
	if (prompt === saved) return 'saved';
	return read === prompt ? 'read' : 'writing';
}

const MARKED = 'Solid: moor holds the agent to it. Dashed: only asked of the agent.';
const MARKS_OFF = 'Marks are off while you write.';

const LEGEND: Record<Phase, string> = {
	saved: 'The file is what each run gets.',
	writing: 'The file is dimmed while you write. Read it to see the lines you changed.',
	read: 'Lit lines are what you changed. Save keeps them.',
};

export type BoxProps = {
	flow: Workflow;
	prompt: string;
	read: string | null;
	showFile: boolean;
	busy: boolean;
	problem: { text: string; stale: boolean } | null;
	saved: boolean;
	onInput: (text: string) => void;
	onRead: () => void;
	onSave: () => void;
	onDiscard: () => void;
	onReload: () => void;
	onToggle: () => void;
	/** What the pointer is on while marks are drawn, and how it moves; the page keeps it to light the gate rows too. */
	hover?: Hover;
	onHover?: (on: Hover) => void;
};

/** The prompt edited in place beside its file: write, Read it to light the changed lines, then Save. */
export function AutomationBox(p: BoxProps) {
	const phase = phaseOf(p.flow.prompt, p.prompt, p.read);
	const lines = fileLines(p.flow, phase === 'read' ? p.prompt : p.flow.prompt);
	const held = marksOf(p.flow.reading, p.flow.prompt);
	const marks = phase === 'saved' ? held : null;
	const lit = litOf(lines, marks ? (p.hover ?? null) : null);
	const onHover = marks ? (p.onHover ?? (() => {})) : undefined;
	const legend = marks ? MARKED : held && phase === 'writing' ? MARKS_OFF : LEGEND[phase];
	return (
		<form
			class="pbox"
			onSubmit={(e) => {
				e.preventDefault();
				if (phase === 'read') p.onSave();
			}}
		>
			<Trigger flow={p.flow} marked={phase !== 'writing'} />
			<p class="rev">
				revision {p.flow.revision}
				{p.flow.plan_stale && <span class="pill warn">plan stale</span>}
				<span class="grow" />
				{phase === 'saved' && (
					<button type="button" class="lnk" onClick={editPrompt}>
						Edit
					</button>
				)}
			</p>
			<div class={p.showFile ? 'pgrid' : 'pgrid one'}>
				<div class={marks ? 'pleft marked' : 'pleft'}>
					{marks && onHover && (
						<PromptMarks
							reading={marks}
							lit={lit.phrases}
							onHover={onHover}
							onEdit={(e) => {
								onHover(null);
								editPrompt(e);
							}}
						/>
					)}
					<textarea
						class="promptbox"
						aria-label="Prompt"
						spellcheck={false}
						value={p.prompt}
						onInput={(e) => p.onInput(e.currentTarget.value)}
					/>
					<p class="legend">{legend}</p>
					{p.problem?.text && <pre class="notice danger" role="alert">{p.problem.text}</pre>}
					{p.saved && phase === 'saved' && <p class="notice ok" role="status">Saved. The workflow is at revision {p.flow.revision}.</p>}
					<h2 class="sub">Repos</h2>
					<div class="rows">
						{p.flow.repos.map((r) => (
							<a key={r} class="rrow" href={`#/repos/${r}`}>
								<RepoName repo={r} />
							</a>
						))}
					</div>
				</div>
				{p.showFile && <AutomationFile name={`${p.flow.name}.md`} lines={lines} dim={phase === 'writing'} lit={lit.lines} onHover={onHover} />}
			</div>
			<div class="pfoot">
				<span class="grow" />
				<button type="button" class="ftog" onClick={p.onToggle}>
					{p.showFile ? 'Hide file' : 'Show file'}
				</button>
				{p.problem?.stale && (
					<button type="button" class="btn" disabled={p.busy} onClick={p.onReload}>
						Discard mine and load theirs
					</button>
				)}
				{phase !== 'saved' && (
					<button type="button" class="btn" disabled={p.busy} onClick={p.onDiscard}>
						Discard
					</button>
				)}
				{phase === 'writing' && (
					<button type="button" class="btn primary" disabled={p.busy} onClick={p.onRead}>
						Read it
					</button>
				)}
				{phase === 'read' && (
					<button class="btn primary" disabled={p.busy}>
						Save
					</button>
				)}
			</div>
		</form>
	);
}
