import { useEffect, useState } from 'preact/hooks';

import { type Account, load, signOut, SignedOut } from './api';
import { Connect } from './Connect';
import { Machines } from './Machines';
import { Repos } from './Repos';
import { Secrets } from './Secrets';
import { BoxPage, Home, NewBox, tabOf, useBoxes } from './Portal';
import { TopBar } from './TopBar';
import { Workflows, workflowsAt } from './Workflows';

type State = { at: 'loading' } | { at: 'ready'; account: Account } | { at: 'out' } | { at: 'failed'; why: string };

/** The repo the address names, `#/repos/owner/name`; `''` for the repos page with none; null for the account. */
function repoOf(hash: string): string | null {
	const rest = hash.replace(/^#\/?/, '');
	if (rest === 'repos') return '';
	return rest.startsWith('repos/') ? decodeURI(rest.slice('repos/'.length)) : null;
}

/** The Box the address names, `#/box/<box>[/<tab>]`; null for any other page. */
function boxOf(hash: string): { box: string; tab: string | undefined } | null {
	const rest = hash.replace(/^#\/?/, '');
	if (!rest.startsWith('box/')) return null;
	const [box, tab] = rest.slice('box/'.length).split('/');
	return box ? { box: decodeURIComponent(box), tab } : null;
}

/** Whether the address is the account page, `#/settings`. */
function settingsAt(hash: string): boolean {
	return hash.replace(/^#\/?/, '') === 'settings';
}

export function App() {
	const [hash, setHash] = useState(location.hash);
	useEffect(() => {
		const onChange = () => setHash(location.hash);
		addEventListener('hashchange', onChange);
		return () => removeEventListener('hashchange', onChange);
	}, []);
	const [state, setState] = useState<State>({ at: 'loading' });
	const refresh = () =>
		load().then(
			(account) => setState({ at: 'ready', account }),
			(e) => setState(e instanceof SignedOut ? { at: 'out' } : { at: 'failed', why: String(e.message) }),
		);
	useEffect(() => void refresh(), []);

	if (state.at === 'out') {
		return (
			<main>
				<h1>moor</h1>
				<p class="muted">Your session ended.</p>
				<a class="button" href="/auth/login?return=/">Sign in again</a>
			</main>
		);
	}
	if (state.at === 'failed') {
		return (
			<main>
				<h1>moor</h1>
				<p class="notice danger" role="alert">{state.why}</p>
				<button onClick={refresh}>Try again</button>
			</main>
		);
	}
	if (state.at === 'loading') return <main aria-busy="true"><h1>moor</h1></main>;

	return <Ready account={state.account} hash={hash} onSignedOut={() => setState({ at: 'out' })} onChanged={refresh} />;
}

/** The signed-in page: the top bar, then the page the address names. */
function Ready({ account, hash, onSignedOut, onChanged }: { account: Account; hash: string; onSignedOut: () => void; onChanged: () => void }) {
	const boxes = useBoxes();
	const repo = repoOf(hash);
	const flows = workflowsAt(hash);
	const open = boxOf(hash);
	const composing = hash.replace(/^#\/?/, '') === 'new';
	const settings = settingsAt(hash);
	const home = repo === null && !flows && !settings;
	const unlinked = account.secrets.filter((s) => !s.linked).map((s) => s.name);
	const leave = () => signOut(account.csrf).then(onSignedOut);
	return (
		<>
			<TopBar email={account.email} rows={boxes.value ?? []} onSignOut={() => void leave()} />
			<main class={flows ? 'wide' : composing ? 'app-new' : open ? 'app-box' : home ? 'app-home' : undefined}>
			{flows ? (
				<Workflows csrf={account.csrf} at={flows} />
			) : home ? (
				<>
					{unlinked.length > 0 && (
						<p class="notice warn">
							Link your {unlinked.join(' and ')} {unlinked.length === 1 ? 'token' : 'tokens'} in <a href="#/settings">Settings</a> before a task can start.
						</p>
					)}
					{composing ? <NewBox account={account} /> : open ? <BoxPage box={open.box} tab={tabOf(open.tab)} csrf={account.csrf} /> : <Home boxes={boxes} />}
				</>
			) : repo === null ? (
				<>
					<Secrets account={account} onLinked={onChanged} />
					<Machines
						csrf={account.csrf}
						machines={account.machines}
						serverUrl={account.connect.server_url}
						onChanged={onChanged}
					/>
					<Connect url={account.connect.mcp_url} />
				</>
			) : (
				<Repos csrf={account.csrf} repo={repo === '' ? null : repo} />
			)}
			</main>
		</>
	);
}
