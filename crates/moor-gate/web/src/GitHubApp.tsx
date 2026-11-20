import { useEffect, useState } from 'preact/hooks';

import { failure, toError } from './api';
import { githubApp } from './settingsApi';

type App = { install_url: string; repos: string[] };

/** The operator's GitHub App: an install link and the repos it is on. Nothing when moor has no App. */
export function GitHubApp() {
	const [app, setApp] = useState<App | null>(null);
	const [problem, setProblem] = useState('');
	useEffect(() => {
		githubApp().then(setApp, (err) => setProblem(failure(toError(err))));
	}, []);
	if (problem) return <p class="notice danger" role="alert">GitHub App: {problem}</p>;
	if (!app) return null;
	return (
		<section>
			<h2>GitHub App</h2>
			<p>
				<a href={app.install_url} target="_blank" rel="noopener noreferrer">
					Install the App on a repo
				</a>
			</p>
			{app.repos.length === 0 ? (
				<p class="muted">The App is not installed on any repo yet.</p>
			) : (
				<ul>
					{app.repos.map((r) => (
						<li key={r}>
							<code>{r}</code>
						</li>
					))}
				</ul>
			)}
		</section>
	);
}
