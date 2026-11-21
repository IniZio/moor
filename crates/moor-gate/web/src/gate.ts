import * as v from 'valibot';

/** What an open gate waits for. */
const NextSchema = v.picklist(['agent', 'run_checks', 'ask']);

/** Where a Box stands against its gates, as moor derives it. */
const GateStatusSchema = v.variant('state', [
	v.object({ state: v.literal('open'), gate: v.string(), try_n: v.number(), next: NextSchema }),
	v.object({ state: v.literal('stuck'), gate: v.string(), why: v.string() }),
	v.object({ state: v.literal('failed'), gate: v.string(), why: v.string() }),
	v.object({ state: v.literal('passed') }),
]);

/** The gates of a Box, in the order they pass, and where it stands. */
export const GateViewSchema = v.object({ gates: v.array(v.string()), status: GateStatusSchema });

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
	/** The end of the failed check's output. */
	tail: v.nullish(v.string()),
	/** The evidence names that were not found. */
	missing: v.optional(v.array(v.string()), []),
	hint: v.nullish(v.string()),
});

export type GateView = v.InferOutput<typeof GateViewSchema>;
export type Wait = v.InferOutput<typeof WaitSchema>;

export type GateState = 'passed' | 'working' | 'asking' | 'stuck' | 'failed' | 'later';

/** One line of the gate list. */
export type GateRow = { name: string; state: GateState; words: string };

/** The glyph and the tone class of each gate state, from the same set the task list uses. */
export const GATE_LOOK: Record<GateState, { glyph: string; tone: string }> = {
	passed: { glyph: '✓', tone: 'done' },
	working: { glyph: '◐', tone: 'working' },
	asking: { glyph: '●', tone: 'needs' },
	stuck: { glyph: '●', tone: 'needs' },
	failed: { glyph: '✕', tone: 'failed' },
	later: { glyph: '○', tone: 'quiet' },
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
	if (status.state === 'passed') return view.gates.map((name) => ({ name, state: 'passed', words: 'passed' }));
	const at = view.gates.indexOf(status.gate);
	return view.gates.map((name, i) =>
		i < at ? { name, state: 'passed', words: 'passed' } : i === at ? atGate(name, status) : { name, state: 'later', words: 'not started' },
	);
}

/** A gate's name as a sentence quotes it. */
export function quoted(name: string): string {
	return `“${name}”`;
}

/** A reason moor wrote for a gate that ended short of passing, in plain words: no code marks, the command left to the card's own line for it, the exit code said out. */
export function whyWords(why: string): string {
	const spent = /^out of tries: /i.test(why.trim());
	let text = why.trim().replace(/^out of tries: /i, '').replace(/\s*\(`[^`]*`\)/, '');
	text = text.replace(/`/g, '').replace(/\bexited (-?\d+)/, 'exited with code $1');
	text = text.charAt(0).toUpperCase() + text.slice(1);
	const sentence = /[.!?]$/.test(text) ? text : `${text}.`;
	return spent ? `${sentence} No tries left.` : sentence;
}

/** The one line a task row or a card says about its gates when a person has something to do or it failed; none otherwise. */
export function gateLine(view: GateView | null | undefined): string | null {
	const status = view?.status;
	switch (status?.state) {
		case 'stuck':
			return `${quoted(status.gate)} is stuck: ${whyWords(status.why)}`;
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
