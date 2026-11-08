import { useEffect, useState } from 'preact/hooks';

import { type Account, load, signOut, SignedOut } from './api';
import { Connect } from './Connect';
import { Machines } from './Machines';
import { Secrets } from './Secrets';

type State = { at: 'loading' } | { at: 'ready'; account: Account } | { at: 'out' } | { at: 'failed'; why: string };

export function App() {
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
	const leave = () => signOut(account.csrf).then(() => setState({ at: 'out' }));
	return (
		<main>
			<header>
				<h1>moor</h1>
				<span class="muted">{account.email}</span>
				<button class="quiet" onClick={leave}>Sign out</button>
			</header>
			<Secrets account={account} onLinked={refresh} />
			<Machines machines={account.machines} serverUrl={account.connect.server_url} />
			<Connect url={account.connect.mcp_url} />
		</main>
	);
}
