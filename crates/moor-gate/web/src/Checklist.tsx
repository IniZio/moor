import type { Account } from './api';

type Item = { id: string; title: string; done: boolean; optional?: boolean; text: string; href: string; action: string };

/** The steps a new person takes before their first Box, read from what the account already holds. */
export function items(account: Account): Item[] {
	const linked = (name: string) => account.secrets.some((s) => s.name === name && s.linked);
	const nodes = account.machines.nodes;
	const mine = nodes.find((n) => n.scope === 'personal');
	const shared = nodes.find((n) => n.scope === 'shared' && n.online);
	const machine = mine
		? `${mine.name} is yours${mine.online ? ' and online' : ', but offline'}.`
		: shared
			? `A shared machine, ${shared.name}, is ready. Add your own to keep your Boxes on your hardware.`
			: 'No machine is ready for you yet. Add a computer of your own.';
	const ready = linked('claude') && linked('github') && Boolean(mine?.online || shared);
	return [
		{ id: 'claude', title: 'Link Claude', done: linked('claude'), text: 'Boxes run Claude Code with your token.', href: '#/settings/secrets', action: 'Link' },
		{ id: 'github', title: 'Link GitHub', done: linked('github'), text: 'Boxes clone your repos and push with it.', href: '#/settings/secrets', action: 'Link' },
		{ id: 'machine', title: 'Choose a machine', done: Boolean(mine || shared), text: machine, href: '#/settings/machines', action: mine ? 'See it' : 'Add a machine' },
		{
			id: 'profile',
			title: 'Set an agent profile',
			optional: true,
			done: Boolean(account.profile),
			text: account.profile ?? 'Bring your own plugins, skills and setup to every Box.',
			href: '#/settings/profile',
			action: account.profile ? 'Change' : 'Set one',
		},
		{
			id: 'start',
			title: 'Start your first task',
			done: false,
			text: ready ? 'Say what to do and on which repo.' : 'Finish the steps above first.',
			href: '#/new',
			action: 'New task',
		},
	];
}

/** The first-run checklist on Home, shown until the person's first Box. */
export function Checklist({ account }: { account: Account }) {
	const list = items(account);
	const ready = list.every((i) => i.done || i.optional || i.id === 'start');
	return (
		<section class="checklist" aria-labelledby="checklist-h">
			<h2 id="checklist-h">Get set up</h2>
			<ol>
				{list.map((i) => (
					<li key={i.id} class={i.done ? 'done' : undefined}>
						<span class="check" role="img" aria-label={i.done ? 'done' : 'to do'}>
							{i.done ? '✓' : ''}
						</span>
						<span class="grow">
							<strong>{i.title}</strong>
							{i.optional && <span class="muted"> · optional</span>}
							<br />
							<span class="muted">{i.text}</span>
						</span>
						{i.id === 'start' ? (
							<a class={ready ? 'btn primary' : 'btn'} href={i.href} aria-disabled={!ready}>
								{i.action}
							</a>
						) : (
							<a class={i.done ? 'btn ghost' : 'btn'} href={i.href}>
								{i.action}
							</a>
						)}
					</li>
				))}
			</ol>
		</section>
	);
}
