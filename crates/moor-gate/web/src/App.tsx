import { useEffect, useState } from 'preact/hooks';

import { type Account, inboxList, load, signOut, SignedOut } from './api';
import { Checklist } from './Checklist';
import { dropDrafts, signInHref, useRenewWhileActive } from './session';
import { SignedOutDialog } from './SignedOutDialog';
import { pathOf } from './hashQuery';
import { Home, useBoxes, usePoll } from './Portal';
import { BoxPage, tabOf } from './TaskPage';
import { sectionAt, Settings } from './Settings';
import { Peek } from './Peek';
import { TopBar } from './TopBar';
import { Workflows, workflowsAt } from './Workflows';
import { WriteField } from './WriteField';

type State = { at: 'loading' } | { at: 'ready'; account: Account } | { at: 'out' } | { at: 'failed'; why: string };

/** The Box the address names, `#/box/<box>[/<tab>]`; null for any other page. */
function boxOf(hash: string): { box: string; tab: string | undefined } | null {
	const rest = hash.replace(/^#\/?/, '');
	if (!rest.startsWith('box/')) return null;
	const [box, tab] = rest.slice('box/'.length).split('/');
	return box ? { box: decodeURIComponent(box), tab } : null;
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
			(e) => {
				if (e instanceof SignedOut) location.replace(signInHref());
				else setState({ at: 'failed', why: String(e.message) });
			},
		);
	useEffect(() => void refresh(), []);

	if (state.at === 'out') {
		return (
			<main>
				<h1>moor</h1>
				<p class="muted">Your session ended.</p>
				<a class="button" href={signInHref()}>Sign in again</a>
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
function Ready({ account, hash: address, onSignedOut, onChanged }: { account: Account; hash: string; onSignedOut: () => void; onChanged: () => void }) {
	const hash = pathOf(address);
	const boxes = useBoxes();
	const inbox = usePoll(inboxList, 'inbox');
	const flows = workflowsAt(hash);
	const open = boxOf(hash);
	const composing = hash.replace(/^#\/?/, '') === 'new';
	const settings = sectionAt(hash) !== null;
	const home = !flows && !settings;
	const fresh = boxes.value?.length === 0;
	const unlinked = account.secrets.filter((s) => !s.linked).map((s) => s.name);
	const leave = () => signOut(account.csrf).then(dropDrafts).then(onSignedOut);
	useRenewWhileActive(account.csrf);
	return (
		<>
			<SignedOutDialog />
			<TopBar email={account.email} csrf={account.csrf} rows={boxes.value ?? []} inbox={inbox} onSignOut={() => void leave()} />
			<main class={flows || settings ? 'wide' : open ? 'app-box' : home ? 'app-home' : undefined}>
			{flows ? (
				<Workflows csrf={account.csrf} at={flows} />
			) : home ? (
				<>
					{unlinked.length > 0 && !(fresh && !composing && !open) && (
						<p class="notice warn">
							Link your {unlinked.join(' and ')} {unlinked.length === 1 ? 'token' : 'tokens'} in <a href="#/settings">Settings</a> before a task can start.
						</p>
					)}
					{open ? <BoxPage box={open.box} tab={tabOf(open.tab)} csrf={account.csrf} me={account.email.split('@')[0] ?? account.email} /> : (
						<>
							<WriteField account={account} rows={boxes.value ?? []} asked={composing} onStarted={boxes.reload} />
							{fresh && <Checklist account={account} />}
							<Home boxes={boxes} hash={address} />
							<Peek rows={boxes.value} now={boxes.now} hash={address} csrf={account.csrf} onSent={boxes.reload} />
						</>
					)}
				</>
			) : (
				<Settings account={account} hash={hash} onChanged={onChanged} />
			)}
			</main>
		</>
	);
}
