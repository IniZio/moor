import type { ComponentChildren } from 'preact';
import { useState } from 'preact/hooks';

import {
	addAfter,
	gist,
	isKind,
	type Kind,
	KINDS,
	leads,
	MAX_STEPS,
	move,
	type PlanDoc,
	planText,
	type Problem,
	readPlan,
	remove,
	starter,
	type StepDoc,
	textProblems,
	withKind,
} from './plan';

type Edit = { doc: PlanDoc; set: (doc: PlanDoc) => void };

function Errors({ list }: { list: Problem[] }) {
	if (list.length === 0) return null;
	return (
		<ul class="plan-errors" role="alert">
			{list.map((p) => (
				<li key={p.text}>{p.text}</li>
			))}
		</ul>
	);
}

function Field({ label, children }: { label: string; children: ComponentChildren }) {
	return (
		<label class="plan-field">
			<span>{label}</span>
			{children}
		</label>
	);
}

function Target({ label, value, none, doc, onPick }: { label: string; value?: string; none?: string; doc: PlanDoc; onPick: (id?: string) => void }) {
	const known = value === undefined || doc.steps.some((s) => s.id === value);
	return (
		<Field label={label}>
			<select value={value ?? ''} onChange={(e) => onPick(e.currentTarget.value || undefined)}>
				{(none !== undefined || value === undefined) && <option value="">{none ?? 'Pick a step'}</option>}
				{!known && <option value={value}>missing step {value}</option>}
				{doc.steps.map((s, i) => (
					<option key={s.id} value={s.id}>
						{i + 1}. {s.title}
					</option>
				))}
			</select>
		</Field>
	);
}

function StepFields({ step, at, edit }: { step: StepDoc; at: number; edit: Edit }) {
	const [typed, setEvidence] = useState((step.evidence ?? []).join(', '));
	const names = (text: string) => text.split(/[\s,]+/).filter(Boolean);
	const evidence = names(typed).join() === (step.evidence ?? []).join() ? typed : (step.evidence ?? []).join(', ');
	const put = (next: StepDoc) => edit.set({ ...edit.doc, steps: edit.doc.steps.map((s, i) => (i === at ? next : s)) });
	const patch = (fields: Partial<StepDoc>) => put({ ...step, ...fields });
	const text = (field: 'instructions' | 'done_when' | 'command' | 'question', label: string, rows = 3) => (
		<Field label={label}>
			<textarea rows={rows} spellcheck={false} value={step[field] ?? ''} onInput={(e) => patch({ [field]: e.currentTarget.value })} />
		</Field>
	);
	const approve = step.kind === 'approve';
	return (
		<div class="plan-fields">
			<Field label="Title">
				<input value={step.title} onInput={(e) => patch({ title: e.currentTarget.value })} />
			</Field>
			<Field label="Kind">
				<select value={step.kind} onChange={(e) => isKind(e.currentTarget.value) && put(withKind(step, e.currentTarget.value, edit.doc))}>
					{KINDS.map((k) => (
						<option key={k} value={k}>
							{k}
						</option>
					))}
				</select>
			</Field>
			{(step.kind === 'work' || step.kind === 'deliver') && (
				<>
					{text('instructions', 'Instructions', 4)}
					{text('done_when', 'Done when', 2)}
				</>
			)}
			{step.kind === 'work' && (
				<Field label="Evidence files, comma separated">
					<input
						class="t-mono"
						value={evidence}
						placeholder="report.md, before-*"
						onInput={(e) => {
							setEvidence(e.currentTarget.value);
							patch({ evidence: names(e.currentTarget.value) });
						}}
					/>
				</Field>
			)}
			{step.kind === 'check' && text('command', 'Command, exit 0 passes', 2)}
			{approve && text('question', 'Question for the owner', 2)}
			<Field label="Tries, 1 to 5">
				<input
					type="number"
					min={1}
					max={5}
					value={step.max_tries ?? ''}
					placeholder="1"
					onInput={(e) => patch({ max_tries: Number.isNaN(e.currentTarget.valueAsNumber) ? undefined : e.currentTarget.valueAsNumber })}
				/>
			</Field>
			{step.kind !== 'deliver' && (
				<>
					<Target label={approve ? 'If yes, go to' : 'Then go to'} value={step.ok} doc={edit.doc} onPick={(ok) => patch({ ok })} />
					<Target
						label={approve ? 'If no, go to' : 'On fail, go to'}
						value={step.fail}
						none={approve ? 'End the run, declined' : 'End the run, failed'}
						doc={edit.doc}
						onPick={(fail) => patch({ fail })}
					/>
				</>
			)}
			<p class="muted">
				Step id <code>{step.id}</code>
			</p>
		</div>
	);
}

function StepCard({ step, at, edit, problems, open, onOpen }: { step: StepDoc; at: number; edit: Edit; problems: Problem[]; open: boolean; onOpen: (id: string | null) => void }) {
	const { doc, set } = edit;
	const n = doc.steps.length;
	const add = (kind: string) => {
		if (!isKind(kind)) return;
		const next = addAfter(doc, at, kind);
		set(next);
		onOpen(next.steps[at + 1].id);
	};
	return (
		<li class={problems.length > 0 ? 'plan-step bad' : 'plan-step'}>
			<div class="plan-head">
				<span class="plan-n">{at + 1}.</span>
				<strong class="grow">{step.title || 'Untitled step'}</strong>
				<span class={`kind ${step.kind}`}>{step.kind}</span>
			</div>
			{gist(step) && <p class="plan-gist">{step.kind === 'check' ? <code>{gist(step)}</code> : gist(step)}</p>}
			<p class="muted">{leads(doc, step)}</p>
			<Errors list={problems} />
			<div class="plan-tools">
				<button type="button" class="quiet" aria-expanded={open} aria-label={`${open ? 'Done editing' : 'Edit'} step ${at + 1}`} onClick={() => onOpen(open ? null : step.id)}>
					{open ? 'Done' : 'Edit'}
				</button>
				<button type="button" class="quiet" aria-label={`Move step ${at + 1} up`} disabled={at === 0} onClick={() => set(move(doc, at, -1))}>
					↑
				</button>
				<button type="button" class="quiet" aria-label={`Move step ${at + 1} down`} disabled={at === n - 1} onClick={() => set(move(doc, at, 1))}>
					↓
				</button>
				<button type="button" class="quiet" aria-label={`Remove step ${at + 1}`} disabled={n === 1} onClick={() => set(remove(doc, at))}>
					Remove
				</button>
				<select aria-label={`Add a step after step ${at + 1}`} value="" disabled={n >= MAX_STEPS} onChange={(e) => add(e.currentTarget.value)}>
					<option value="">Add a step after…</option>
					{KINDS.map((k: Kind) => (
						<option key={k} value={k}>
							{k}
						</option>
					))}
				</select>
			</div>
			{open && <StepFields key={step.id} step={step} at={at} edit={edit} />}
		</li>
	);
}

/** A workflow's plan as numbered steps, edited in place, with its JSON behind a toggle. Its value is the plan's JSON text, blank for none. */
export function PlanEditor({ text, onText, prompt }: { text: string; onText: (text: string) => void; prompt: string }) {
	const [asJson, setAsJson] = useState(false);
	const [open, setOpen] = useState<string | null>(null);
	const [stuck, setStuck] = useState('');
	const all = textProblems(text);
	const toSteps = () => {
		const read = text.trim() === '' ? null : readPlan(text);
		if (read && 'error' in read) return setStuck(`Fix the JSON to see it as steps: ${read.error}`);
		setStuck('');
		setAsJson(false);
	};
	const head = (
		<div class="plan-bar">
			<strong class="grow">Plan</strong>
			<button type="button" class="quiet" aria-pressed={asJson} onClick={() => (asJson ? toSteps() : setAsJson(true))}>
				{asJson ? 'Edit as steps' : 'Edit as JSON'}
			</button>
		</div>
	);
	if (asJson) {
		return (
			<div class="plan">
				{head}
				<textarea
					aria-label="Plan JSON"
					rows={18}
					spellcheck={false}
					class="t-mono"
					placeholder="No plan: each run delivers the prompt in one step."
					value={text}
					onInput={(e) => onText(e.currentTarget.value)}
				/>
				{stuck && <p class="notice danger">{stuck}</p>}
				<Errors list={all} />
			</div>
		);
	}
	const read = text.trim() === '' ? null : readPlan(text);
	if (!read) {
		return (
			<div class="plan">
				{head}
				<p class="muted">Runs the prompt in one step.</p>
				<button
					type="button"
					onClick={() => {
						onText(planText(starter(prompt)));
						setOpen('deliver');
					}}
				>
					Add a plan
				</button>
			</div>
		);
	}
	if ('error' in read) return <div class="plan">{head}<Errors list={all} /></div>;
	const edit: Edit = { doc: read.doc, set: (doc) => onText(planText(doc)) };
	return (
		<div class="plan">
			{head}
			<Errors list={all.filter((p) => p.step === null)} />
			<ol class="plan-steps">
				{read.doc.steps.map((s, i) => (
					<StepCard
						key={`${i}:${s.id}`}
						step={s}
						at={i}
						edit={edit}
						problems={all.filter((p) => p.step === i)}
						open={open === s.id}
						onOpen={setOpen}
					/>
				))}
			</ol>
		</div>
	);
}
