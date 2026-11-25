import * as v from 'valibot';

const text = v.nullish(v.string());
const StepDocSchema = v.strictObject({
	id: v.string(),
	title: v.string(),
	kind: v.string(),
	max_tries: v.nullish(v.number()),
	instructions: text,
	done_when: text,
	evidence: v.nullish(v.array(v.string())),
	command: text,
	question: text,
	ok: text,
	fail: text,
});
const PlanDocSchema = v.strictObject({
	steps: v.array(StepDocSchema),
	made_from: text,
	edited: v.nullish(v.boolean()),
});

export type StepDoc = {
	id: string;
	title: string;
	kind: string;
	max_tries?: number;
	instructions?: string;
	done_when?: string;
	evidence?: string[];
	command?: string;
	question?: string;
	ok?: string;
	fail?: string;
};
export type PlanDoc = { steps: StepDoc[]; made_from?: string; edited?: boolean };

function dropNulls<T extends object>(o: T): T {
	return Object.fromEntries(Object.entries(o).filter(([, x]) => x != null)) as T;
}

/** The plan in `text`, or why it is not one. */
export function readPlan(text: string): { doc: PlanDoc } | { error: string } {
	let raw: unknown;
	try {
		raw = JSON.parse(text);
	} catch (err) {
		return { error: `the plan is not valid: ${err instanceof Error ? err.message : 'not JSON'}` };
	}
	if (!v.is(v.looseObject({}), raw) || Array.isArray(raw)) {
		return { error: 'the plan is not valid: a plan is an object with a `steps` list, like {"steps": [...]}' };
	}
	const parsed = v.safeParse(PlanDocSchema, raw);
	if (!parsed.success) {
		const issue = parsed.issues[0];
		const keyed = issue.type === 'strict_object';
		const at = (keyed ? issue.path?.slice(0, -1) : issue.path)?.map((p) => String(p.key)).join('.');
		const why = !keyed ? issue.message : issue.received === 'undefined' ? `missing field ${issue.expected}` : `unknown field ${issue.received}`;
		return { error: `the plan is not valid: ${at ? `${at}: ` : ''}${why}` };
	}
	const doc = dropNulls(parsed.output) as PlanDoc;
	return { doc: { ...doc, steps: parsed.output.steps.map((s) => dropNulls(s) as StepDoc) } };
}

/** What a step asks, short: its instructions, its command or its question. */
export function gist(step: StepDoc): string {
	const said = step.kind === 'check' ? step.command : step.kind === 'approve' ? step.question : step.instructions;
	const line = (said ?? '').replace(/\s+/g, ' ').trim();
	return line.length > 140 ? `${line.slice(0, 139)}…` : line;
}

/** Where a step leads, in words, such as "then 3. Live regression · on fail 5. Report". */
export function leads(doc: PlanDoc, step: StepDoc): string {
	if (step.kind === 'deliver') return 'ends the run';
	const name = (id: string | undefined) => {
		const i = doc.steps.findIndex((s) => s.id === id);
		return i < 0 ? (id ? `missing step ${id}` : 'nowhere yet') : `${i + 1}. ${doc.steps[i].title}`;
	};
	const approve = step.kind === 'approve';
	const ok = `${approve ? 'if yes' : 'then'} ${name(step.ok)}`;
	const fail = step.fail !== undefined ? name(step.fail) : approve ? 'the run ends declined' : 'the run fails';
	return `${ok} · ${approve ? 'if no' : 'on fail'} ${fail}`;
}
