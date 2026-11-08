import type { Account, Node } from './api';
import { Command } from './Connect';

function state(n: Node): string {
	if (n.online) return 'Online';
	if (!n.offline_since) return 'Offline';
	const mins = Math.max(0, Math.round((Date.now() / 1000 - n.offline_since) / 60));
	return mins < 60 ? `Offline ${mins}m` : mins < 1440 ? `Offline ${Math.round(mins / 60)}h` : `Offline ${Math.round(mins / 1440)}d`;
}

function slots(n: Node): string {
	if (n.max_boxes === undefined) return `${n.used} running`;
	return `${Math.max(0, n.max_boxes - n.used)} of ${n.max_boxes} free`;
}

export function Machines({ machines, serverUrl }: { machines: Account['machines']; serverUrl: string }) {
	return (
		<section>
			<h2>Machines</h2>
			{machines.nodes.length === 0 ? (
				<>
					<p class="muted">No machine of yours yet. Adding one is optional. Run this on it:</p>
					<ul class="cards">
						<Command label="Add this machine" line={`moor-machine-agent up --server ${serverUrl}`} />
					</ul>
				</>
			) : (
				<ul class="cards">
					{machines.nodes.map((n) => (
						<li key={n.name}>
							<div class="row-head">
								<strong>{n.name}</strong>
								<span class={n.online ? 'pill ok' : 'pill warn'}>{state(n)}</span>
							</div>
							<p class="muted">
								{n.scope === 'personal' ? 'Yours' : 'Shared'} · {n.runtime} · {slots(n)}
								{n.mem_free_mib !== undefined && ` · ${Math.round(n.mem_free_mib / 1024)} GiB memory free`}
							</p>
						</li>
					))}
				</ul>
			)}
		</section>
	);
}
