import type { ComponentChildren } from 'preact';
import { useEffect, useState } from 'preact/hooks';

import { type Account, Conflict, type Node, nodeSet, failure } from './api';
import { RemoveMachine } from './RemoveMachine';

function state(n: Node): string {
	if (n.online) return 'Online';
	if (!n.offline_since) return 'Offline';
	const mins = Math.max(0, Math.round((Date.now() / 1000 - n.offline_since) / 60));
	return mins < 60 ? `Offline ${mins}m` : mins < 1440 ? `Offline ${Math.round(mins / 60)}h` : `Offline ${Math.round(mins / 1440)}d`;
}

function gib(mib: number): string {
	return `${(mib / 1024).toFixed(1)} GiB`;
}

/** What the node carries now and how hard its host is pressed, as the last memory report says. */
function load(n: Node): string[] {
	const parts = [n.max_boxes === undefined ? `${n.used} running, no cap` : `${n.used} of ${n.max_boxes} running`];
	if (n.waiting > 0) parts.push(`${n.waiting} waiting`);
	if (n.mem_free_mib !== undefined) parts.push(`${gib(n.mem_free_mib)} memory free`);
	if (n.mem_psi_centi !== undefined) parts.push(`memory pressure ${(n.mem_psi_centi / 100).toFixed(2)}`);
	if (n.box_memory_mib !== undefined) parts.push(`Boxes hold ${gib(n.box_memory_mib)}`);
	if (n.mem_report_age_secs !== undefined) parts.push(`reported ${n.mem_report_age_secs}s ago`);
	return parts;
}

function words(text: string): string[] {
	return text
		.split(/[\s,]+/)
		.map((w) => w.trim())
		.filter(Boolean);
}

function Settings({ csrf, node, onChanged }: { csrf: string; node: Node; onChanged: () => void }) {
	const [cap, setCap] = useState(node.max_boxes === undefined ? '' : String(node.max_boxes));
	const [labels, setLabels] = useState(node.labels.join(', '));
	const [busy, setBusy] = useState(false);
	const [problem, setProblem] = useState<{ text: string; stale: boolean } | null>(null);
	const [saved, setSaved] = useState(false);

	useEffect(() => {
		setCap(node.max_boxes === undefined ? '' : String(node.max_boxes));
		setLabels(node.labels.join(', '));
	}, [node.revision]);

	const capped = cap.trim() !== '';
	const max = capped ? Number(cap.trim()) : null;
	const valid = max === null || (Number.isInteger(max) && max >= 1);
	const dirty = (max ?? undefined) !== node.max_boxes || words(labels).join(',') !== node.labels.join(',');

	const save = async (e: Event) => {
		e.preventDefault();
		setBusy(true);
		setProblem(null);
		setSaved(false);
		try {
			await nodeSet(csrf, node.name, max, words(labels), node.revision);
			setSaved(true);
			onChanged();
		} catch (err) {
			setProblem({ text: failure(err), stale: err instanceof Conflict });
		}
		setBusy(false);
	};

	return (
		<form onSubmit={save} class="settings">
			<label>
				Most Boxes
				<input
					type="number"
					min={1}
					step={1}
					placeholder="no cap"
					value={cap}
					onInput={(e) => setCap(e.currentTarget.value)}
				/>
			</label>
			<label>
				Labels
				<input
					placeholder="gpu, big"
					autocomplete="off"
					spellcheck={false}
					value={labels}
					onInput={(e) => setLabels(e.currentTarget.value)}
				/>
			</label>
			<p class="muted">Leave Most Boxes empty for no cap. Free memory and pressure then decide. A change applies to the next start.</p>
			{problem?.text && <p class="notice danger" role="alert">{problem.text}</p>}
			{saved && <p class="notice ok" role="status">Saved.</p>}
			<div class="actions">
				<button disabled={busy || !dirty || !valid}>Save</button>
				{problem?.stale && (
					<button type="button" class="quiet" onClick={onChanged}>Discard mine and load theirs</button>
				)}
			</div>
		</form>
	);
}

export function Machines({
	csrf,
	machines,
	onChanged,
	action,
}: {
	csrf: string;
	machines: Account['machines'];
	onChanged: () => void;
	action?: ComponentChildren;
}) {
	return (
		<section>
			<div class="row-head section-head">
				<h2>Machines</h2>
				{action}
			</div>
			<p class="muted">Where your Boxes run. Add a computer of your own, or use a shared one your team runs.</p>
			{machines.nodes.length === 0 ? (
				<p class="muted">No machine runs Boxes for you yet. Add one of your own, or ask your team for a shared one.</p>
			) : (
				<ul class="cards">
					{machines.nodes.map((n) => (
						<li key={n.name}>
							<div class="row-head">
								<strong>{n.name}</strong>
								<span class="grow" />
								<span class={n.online ? 'pill ok' : 'pill warn'}>{state(n)}</span>
								{n.scope === 'personal' && n.editable && <RemoveMachine csrf={csrf} node={n} onRemoved={onChanged} />}
							</div>
							<p class="muted">
								{n.scope === 'personal' ? 'Yours' : 'Shared'} · {n.provides ?? n.runtime}
								{n.labels.length > 0 && ` · ${n.labels.join(', ')}`}
							</p>
							<p>{load(n).join(' · ')}</p>
							{n.pressure_since !== undefined && (
								<p class="notice warn" role="status">
									Under memory pressure since {new Date(n.pressure_since * 1000).toLocaleTimeString()}.
								</p>
							)}
							{n.editable && <Settings csrf={csrf} node={n} onChanged={onChanged} />}
						</li>
					))}
				</ul>
			)}
		</section>
	);
}
