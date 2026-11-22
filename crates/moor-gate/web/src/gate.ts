import * as v from 'valibot';

/** What an open gate waits for. */
const NextSchema = v.picklist(['agent', 'run_checks', 'ask', 'decide']);

const ToneSchema = v.picklist(['good', 'bad', 'plain']);

export const VerdictSchema = v.object({ name: v.string(), tone: ToneSchema, says: v.nullish(v.string()) });

/** The stage after the last gate, where moor decides the outcome. */
export const CLOSING = 'outcome';

/** Where a Box stands against its gates, as moor derives it. */
const GateStatusSchema = v.variant('state', [
	v.object({ state: v.literal('open'), gate: v.string(), try_n: v.number(), next: NextSchema }),
	v.object({ state: v.literal('stuck'), gate: v.string(), why: v.string() }),
	v.object({ state: v.literal('failed'), gate: v.string(), why: v.string() }),
	v.object({ state: v.literal('passed'), outcome: v.optional(VerdictSchema) }),
]);

/** The gates of a Box, in the order they pass, and where it stands. */
export const GateViewSchema = v.object({
	gates: v.array(v.string()),
	status: GateStatusSchema,
	/** How many gates passed; fewer than the gates when an outcome that holds at any point ended the run first. */
	gates_passed: v.optional(v.number()),
	/** What the agent wrote with the last report moor accepted; none when it wrote nothing. */
	note: v.nullish(v.string()),
});

/** One outcome moor tried at the closing stage that did not hold, with the check that stopped it. */
const TriedSchema = v.object({
	outcome: v.string(),
	says: v.nullish(v.string()),
	command: v.nullish(v.string()),
	exit: v.nullish(v.number()),
	tail: v.string(),
});

/** A gate that waits for a person, with what they need to read to act on it. */
export const WaitSchema = v.object({
	gate: v.string(),
	kind: v.picklist(['approve', 'stuck']),
	seq: v.number(),
	try_n: v.nullish(v.number()),
	why: v.nullish(v.string()),
	/** The evidence names the agent handed in. */
	handed: v.optional(v.array(v.string()), []),
	/** What the agent wrote with its report. */
	note: v.nullish(v.string()),
	/** The check that failed, as written in the gate. */
	command: v.nullish(v.string()),
	/** What that check says must be true, when its author wrote it. */
	says: v.nullish(v.string()),
	/** The end of the failed check's output. */
	tail: v.nullish(v.string()),
	/** The evidence names that were not found. */
	missing: v.optional(v.array(v.string()), []),
	/** For the closing stage: each outcome tried and the check that stopped it. */
	tried: v.nullish(v.array(TriedSchema)),
	hint: v.nullish(v.string()),
});

/** A check as a gate is written: a bare command, or a command with the one sentence it stands for. */
const CheckDocSchema = v.union([v.string(), v.object({ run: v.string(), says: v.string() })]);

export const OutcomeDocSchema = v.object({
	name: v.string(),
	says: v.nullish(v.string()),
	tone: v.nullish(ToneSchema),
	checks: v.nullish(v.array(CheckDocSchema)),
	anytime: v.nullish(v.boolean()),
});

/** One gate as the workflow keeps it. */
export const GateDocSchema = v.object({
	name: v.string(),
	evidence: v.nullish(v.array(v.string())),
	checks: v.nullish(v.array(CheckDocSchema)),
	approval: v.nullish(v.string()),
	max_tries: v.nullish(v.number()),
});

export type GateDoc = v.InferOutput<typeof GateDocSchema>;
export type OutcomeDoc = v.InferOutput<typeof OutcomeDocSchema>;
export type Verdict = v.InferOutput<typeof VerdictSchema>;
export type Tried = v.InferOutput<typeof TriedSchema>;
export type CheckDoc = v.InferOutput<typeof CheckDocSchema>;
export type GateView = v.InferOutput<typeof GateViewSchema>;
export type Wait = v.InferOutput<typeof WaitSchema>;

export type GateState = 'passed' | 'working' | 'asking' | 'stuck' | 'failed' | 'later' | 'bad' | 'plain';

/** One line of the gate list. */
export type GateRow = { name: string; state: GateState; words: string };

/** The glyph and the tone class of each gate state, from the same set the Box list uses. */
export const GATE_LOOK: Record<GateState, { glyph: string; tone: string }> = {
	passed: { glyph: '✓', tone: 'done' },
	working: { glyph: '◐', tone: 'working' },
	asking: { glyph: '●', tone: 'needs' },
	stuck: { glyph: '●', tone: 'needs' },
	failed: { glyph: '✕', tone: 'failed' },
	later: { glyph: '○', tone: 'quiet' },
	bad: { glyph: '!', tone: 'failed' },
	plain: { glyph: '✓', tone: 'quiet' },
};

/** The state of the gate the Box is at. Gates are passed in order, so the ones before it passed and the ones after it have not started. */
function atGate(name: string, status: Exclude<GateView['status'], { state: 'passed' }>): GateRow {
	switch (status.state) {
		case 'stuck':
			return { name, state: 'stuck', words: 'stuck · needs you' };
		case 'failed':
			return { name, state: 'failed', words: 'failed' };
		case 'open': {
			const again = status.try_n > 1 ? ` · try ${status.try_n}` : '';
			if (status.next === 'ask') return { name, state: 'asking', words: `waiting for you${again}` };
			const waits = status.next === 'agent' ? "waiting for the agent's evidence" : 'moor is running its checks';
			return { name, state: 'working', words: `in progress · ${waits}${again}` };
		}
	}
}

/** Each gate by name with its state, in the order they pass. */
export function gateRows(view: GateView): GateRow[] {
	const { status } = view;
	if (status.state === 'passed') {
		const reached = view.gates_passed ?? view.gates.length;
		return view.gates.map((name, i): GateRow => (i < reached ? { name, state: 'passed', words: 'passed' } : { name, state: 'later', words: 'not reached' }));
	}
	const named = view.gates.indexOf(status.gate);
	const at = named === -1 ? view.gates.length : named;
	return view.gates.map((name, i) =>
		i < at ? { name, state: 'passed', words: 'passed' } : i === at ? atGate(name, status) : { name, state: 'later', words: 'not started' },
	);
}

/** An outcome as one line: its name, then what it says when its author wrote it. */
export function outcomeLine(o: { name: string; says?: string | null }): string {
	return o.says ? `${o.name} · ${o.says}` : o.name;
}

/** The outcome of a workflow that names none, which a person reads as the run finishing. */
const isImplicit = (o: Verdict) => o.name === 'done' && o.tone === 'good' && !o.says;

/** The row after the gates where moor decides how the run ends; none while a gate is in hand, and none for the implicit outcome. */
export function closingRow(view: GateView): GateRow | null {
	const { status } = view;
	if (status.state === 'passed') {
		const o = status.outcome;
		if (!o || isImplicit(o)) return null;
		return { name: 'Outcome', state: o.tone === 'good' ? 'passed' : o.tone, words: outcomeLine(o) };
	}
	if (status.state === 'open' && status.next === 'decide') return { name: 'Outcome', state: 'working', words: 'moor is checking which outcome holds' };
	if (status.state === 'stuck' && status.gate === CLOSING) return { name: 'Outcome', state: 'stuck', words: 'stuck · needs you' };
	return null;
}

/** A gate's name as a sentence quotes it. */
export function quoted(name: string): string {
	return `“${name}”`;
}

/** A sentence moor wrote, without Markdown code marks: a gate it names between them is quoted as the page quotes gates, any other word is left bare. */
export function plain(text: string, gates: readonly string[] = []): string {
	return text.replace(/`([^`]*)`/g, (_, word: string) => (gates.includes(word) ? quoted(word) : word)).replace(/`/g, '');
}

/** A reason moor wrote for a gate that ended short of passing, in plain words: no code marks, the command left to the card's own line for it, the exit code said out. */
export function whyWords(why: string, says?: string | null): string {
	const spent = /^out of tries: /i.test(why.trim());
	let text = why.trim().replace(/^out of tries: /i, '').replace(/\s*\(`[^`]*`\)/, '');
	if (says) text = text.replace(/\s*\(“[^”]*”\)/, '');
	text = text.replace(/`/g, '').replace(/\bexited (-?\d+)/, 'exited with code $1');
	text = text.charAt(0).toUpperCase() + text.slice(1);
	const sentence = /[.!?]$/.test(text) ? text : `${text}.`;
	return spent ? `${sentence} No tries left.` : sentence;
}

/** The one line a list row or a card says about its gates when a person has something to do or it failed; none otherwise. */
export function gateLine(view: GateView | null | undefined): string | null {
	const status = view?.status;
	switch (status?.state) {
		case 'stuck':
			return status.gate === CLOSING ? `No outcome holds yet: ${whyWords(status.why)}` : `${quoted(status.gate)} is stuck: ${whyWords(status.why)}`;
		case 'failed':
			return `Failed at ${quoted(status.gate)}: ${whyWords(status.why)}`;
		case 'open':
			return status.next === 'ask' ? `Waiting for your yes or no on ${quoted(status.gate)}` : null;
		default:
			return null;
	}
}

/** A check's output cut to its last `keep` lines for the card, and the whole of it when it was cut. */
export function tailView(tail: string, keep = 4): { shown: string; all: string | null } {
	const text = tail.replace(/\s+$/, '');
	const lines = text.split('\n');
	return lines.length <= keep ? { shown: text, all: null } : { shown: lines.slice(-keep).join('\n'), all: text };
}

/** What a check is, for a reader: the sentence it says, or a plain stand-in for a bare command. */
export function checkWords(check: CheckDoc): string {
	return v.is(v.string(), check) ? 'a command exits with code 0' : check.says;
}

/** The command of a check as written. */
export function checkRun(check: CheckDoc): string {
	return v.is(v.string(), check) ? check : check.run;
}

const TRIES = ['', 'One try.', 'Two tries.', 'Three tries.', 'Four tries.', 'Five tries.'];

/** The files a gate asks for, in words: a prefix is "files named pr-*". */
function handIn(names: readonly string[]): string {
	const items = names.map((n) => (n.endsWith('*') ? `files named ${n}` : n));
	return items.length < 2 ? (items[0] ?? '') : `${items.slice(0, -1).join(', ')} and ${items[items.length - 1]}`;
}

/** One gate in a sentence a teammate can read: what the agent hands in, what moor checks, who must say yes, and how many tries. */
export function ruleSentence(gate: GateDoc): string {
	const files = gate.evidence ?? [];
	const checks = gate.checks ?? [];
	const owner = gate.approval === 'owner';
	const first: string[] = [];
	if (files.length > 0) first.push(`the agent must hand in ${handIn(files)}`);
	if (checks.length > 0) first.push(`moor checks: ${checks.map((c) => checkWords(c).trim().replace(/\.$/, '')).join('; ')}`);
	const sentences: string[] = [];
	if (first.length > 0) sentences.push(`${first.join(', and ')}.`);
	if (owner) sentences.push(first.length > 0 ? "Then the workflow's owner must say yes." : "the workflow's owner must say yes.");
	if (checks.length > 0 || owner) sentences.push(TRIES[gate.max_tries ?? 2] ?? '');
	return sentences.filter(Boolean).join(' ');
}
