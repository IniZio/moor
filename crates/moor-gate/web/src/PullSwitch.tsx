import { useEffect, useState } from 'preact/hooks';

import { githubApp } from './settingsApi';

/** The pull request switch. With no GitHub App it is off and disabled with the reason, unless it is already on, so it can still be turned off. */
export function PullSwitch({ on, onChange }: { on: boolean; onChange: (on: boolean) => void }) {
	const [hasApp, setHasApp] = useState(true);
	useEffect(() => {
		githubApp().then(
			(app) => setHasApp(app != null),
			() => setHasApp(true),
		);
	}, []);
	const locked = !hasApp && !on;
	return (
		<div class="wf-field">
			<label class="wf-switch">
				<input type="checkbox" role="switch" checked={on} disabled={locked} aria-describedby="wf-pull-help" onChange={(e) => onChange(e.currentTarget.checked)} />
				On pull request
			</label>
			<span class="muted" id="wf-pull-help">
				Runs when a pull request is opened, marked ready, or pushed to, in this workflow's first repo. Needs the GitHub App.
				{locked && ' This controller has no GitHub App configured, so the switch is off.'}
			</span>
		</div>
	);
}
