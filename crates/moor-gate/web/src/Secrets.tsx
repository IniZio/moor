import type { ComponentChildren } from 'preact';
import { useState } from 'preact/hooks';

import { type Account, link, SignedOut } from './api';

const HINTS: Record<string, ComponentChildren> = {
	claude: (
		<>
			Run <code>claude setup-token</code> on your own machine and paste what it prints.
		</>
	),
	github: (
		<>
			Paste what <code>gh auth token</code> prints, or a fine-grained token that can push to your repos.
		</>
	),
};

function Row({ account, name, linked, onLinked }: { account: Account; name: string; linked: boolean; onLinked: () => void }) {
	const [value, setValue] = useState('');
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState('');
	const submit = async (e: Event) => {
		e.preventDefault();
		setBusy(true);
		setError('');
		try {
			await link(account.csrf, name, value);
			setValue('');
			onLinked();
		} catch (err) {
			setError(err instanceof SignedOut ? 'Your session ended. Reload to sign in again.' : String((err as Error).message));
		}
		setBusy(false);
	};
	return (
		<li>
			<form onSubmit={submit}>
				<div class="row-head">
					<strong>{name}</strong>
					<span class={linked ? 'pill ok' : 'pill'}>{linked ? 'Linked' : 'Not linked'}</span>
				</div>
				<p class="muted">{HINTS[name] ?? 'Paste the token.'}</p>
				<div class="field">
					<input
						type="password"
						autocomplete="off"
						spellcheck={false}
						aria-label={`${name} token`}
						placeholder={linked ? 'Paste a new token to replace it' : 'Paste token'}
						value={value}
						onInput={(e) => setValue(e.currentTarget.value)}
					/>
					<button disabled={busy || value === ''}>{linked ? 'Replace' : 'Link'}</button>
				</div>
				{error && <p class="notice danger" role="alert">{error}</p>}
			</form>
		</li>
	);
}

export function Secrets({ account, onLinked }: { account: Account; onLinked: () => void }) {
	return (
		<section>
			<h2>Secrets</h2>
			<p class="muted">Your Boxes use these. Values are never shown again.</p>
			<ul class="cards">
				{account.secrets.map((s) => (
					<Row key={s.name} account={account} name={s.name} linked={s.linked} onLinked={onLinked} />
				))}
			</ul>
		</section>
	);
}
