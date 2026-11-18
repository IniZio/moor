import { useEffect, useState } from 'preact/hooks';

import { type Account, load, signOut, SignedOut } from './api';
import { Connect } from './Connect';
import { Machines } from './Machines';
import { Repos } from './Repos';
import { Secrets } from './Secrets';
import { Home, tabOf, TaskPage } from './Tasks';
import { Workflows, workflowsAt } from './Workflows';

type State = { at: 'loading' } | { at: 'ready'; account: Account } | { at: 'out' } | { at: 'failed'; why: string };

/** The repo the address names, `#/repos/owner/name`; `''` for the repos page with none; null for the account. */
function repoOf(hash: string): string | null {
	const rest = hash.replace(/^#\/?/, '');
	if (rest === 'repos') return '';
	return rest.startsWith('repos/') ? decodeURI(rest.slice('repos/'.length)) : null;
}

/** The task the address names, `#/task/<box>[/<tab>]`; null for any other page. */
function taskOf(hash: string): { box: string; tab: string | undefined } | null {
	const rest = hash.replace(/^#\/?/, '');
	if (!rest.startsWith('task/')) return null;
	const [box, tab] = rest.slice('task/'.length).split('/');
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

	const { account } = state;
	const repo = repoOf(hash);
	const flows = workflowsAt(hash);
	const task = taskOf(hash);
	const settings = settingsAt(hash);
	const tasks = repo === null && !flows && !settings;
	const unlinked = account.secrets.filter((s) => !s.linked).map((s) => s.name);
	const leave = () => signOut(account.csrf).then(() => setState({ at: 'out' }));
	return (
		<main class={flows || tasks ? 'wide' : undefined}>
			<header>
				<h1>moor</h1>
				<nav>
					<a href="#/" aria-current={tasks ? 'page' : undefined}>Tasks</a>
					<a href="#/workflows" aria-current={flows ? 'page' : undefined}>Workflows</a>
					<a href="#/repos" aria-current={repo === null ? undefined : 'page'}>Repos</a>
					<a href="#/settings" aria-current={settings ? 'page' : undefined}>Settings</a>
				</nav>
				<span class="muted">{account.email}</span>
				<button class="quiet" onClick={leave}>Sign out</button>
			</header>
			{flows ? (
				<Workflows csrf={account.csrf} at={flows} />
			) : tasks ? (
				<>
					{unlinked.length > 0 && (
						<p class="notice warn">
							Link your {unlinked.join(' and ')} {unlinked.length === 1 ? 'token' : 'tokens'} in <a href="#/settings">Settings</a> before a task can start.
						</p>
					)}
					{task ? <TaskPage box={task.box} tab={tabOf(task.tab)} /> : <Home />}
				</>
			) : repo === null ? (
				<>
					<Secrets account={account} onLinked={refresh} />
					<Machines
						csrf={account.csrf}
						machines={account.machines}
						serverUrl={account.connect.server_url}
						onChanged={refresh}
					/>
					<Connect url={account.connect.mcp_url} />
				</>
			) : (
				<Repos csrf={account.csrf} repo={repo === '' ? null : repo} />
			)}
		</main>
	);
}
