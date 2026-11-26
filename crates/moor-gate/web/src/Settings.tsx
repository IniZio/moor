import type { Account } from './api';
import { AddMachine } from './AddMachine';
import { Connect } from './Connect';
import { ContractPage } from './Contract';
import { Machines } from './Machines';
import { Profile } from './Profile';
import { Repos } from './Repos';
import { rulesAt } from './Rules';
import { Secrets } from './Secrets';
import './settingsPage.css';

const SECTIONS = {
	machines: { label: 'Machines', href: '#/settings/machines' },
	profile: { label: 'Agent profile', href: '#/settings/profile' },
	secrets: { label: 'Secrets', href: '#/settings/secrets' },
	repos: { label: 'Repos', href: '#/repos' },
	connect: { label: 'Connect', href: '#/settings/connect' },
	contract: { label: 'Agent contract', href: '#/settings/contract' },
} as const;

export type Section = keyof typeof SECTIONS;

function isSection(name: string): name is Section {
	return Object.hasOwn(SECTIONS, name);
}

/** The settings section the address names: `#/settings[/<section>]`, or `#/repos[/owner/name]` for Repos; null for any other page. */
export function sectionAt(hash: string): Section | null {
	const rest = hash.replace(/^#\/?/, '');
	if (rest === 'repos' || (rest.startsWith('repos/') && !rulesAt(hash))) return 'repos';
	if (rest === 'settings') return 'machines';
	const name = rest.startsWith('settings/') ? rest.slice('settings/'.length) : null;
	return name !== null && isSection(name) ? name : null;
}

/** The repo a `#/repos/owner/name` address names, or null for the list. */
function repoAt(hash: string): string | null {
	const rest = hash.replace(/^#\/?/, '');
	return rest.startsWith('repos/') ? decodeURI(rest.slice('repos/'.length)) || null : null;
}

/** Settings: a side list of sections, then the one the address names. */
export function Settings({ account, hash, onChanged }: { account: Account; hash: string; onChanged: () => void }) {
	const section = sectionAt(hash) ?? 'machines';
	const github = account.secrets.some((s) => s.name === 'github' && s.linked);
	return (
		<div class="settings-page">
			<nav class="settings-nav" aria-label="Settings">
				<h1>Settings</h1>
				{Object.entries(SECTIONS).map(([id, s]) => (
					<a href={s.href} aria-current={id === section ? 'page' : undefined} key={id}>
						{s.label}
					</a>
				))}
			</nav>
			<div class="settings-body">
				{section === 'machines' && (
					<Machines
						csrf={account.csrf}
						machines={account.machines}
						onChanged={onChanged}
						action={<AddMachine csrf={account.csrf} server={account.connect.server_url} nodes={account.machines.nodes} reload={onChanged} />}
					/>
				)}
				{section === 'profile' && <Profile csrf={account.csrf} githubLinked={github} onChanged={onChanged} />}
				{section === 'secrets' && <Secrets account={account} onLinked={onChanged} />}
				{section === 'repos' && <Repos csrf={account.csrf} repo={repoAt(hash)} />}
				{section === 'connect' && <Connect url={account.connect.mcp_url} />}
				{section === 'contract' && <ContractPage />}
			</div>
		</div>
	);
}
