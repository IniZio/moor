import { useEffect, useState } from 'preact/hooks';
import * as v from 'valibot';

import { failure } from './api';
import { type Installs, type ProfileCheck, profileCheck, profileGet, profileRemove, profileSet, SourceInput } from './settingsApi';

const STEP_WORDS = {
	resolve: 'Finding the commit',
	manifest: 'Reading moor-profile.yaml',
	fetch: 'Fetching it into the Box',
	install: 'Installing a plugin',
	setup: 'Running setup.sh',
} as const;

function when(ms: number): string {
	return new Date(ms).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
}

/** One agent's part of what the profile installs; nothing when it sets nothing. */
function Agent({ name, installs }: { name: string; installs: Installs }) {
	if (!installs.model && !installs.marketplaces.length && !installs.plugins.length) return null;
	return (
		<>
			<dt>{name}</dt>
			<dd>
				{installs.model && (
					<div>
						Model <code>{installs.model}</code>
					</div>
				)}
				{installs.plugins.length > 0 && (
					<div>
						Plugins {installs.plugins.flatMap((p, i) => (i ? [', ', <code key={p}>{p}</code>] : [<code key={p}>{p}</code>]))}
					</div>
				)}
				{installs.marketplaces.length > 0 && (
					<div class="muted">
						From {installs.marketplaces.map((m) => `${m.name} (${m.repo})`).join(', ')}
					</div>
				)}
			</dd>
		</>
	);
}

/** What a check found: the commit and what it installs, or each failed step. */
export function CheckResult({ check }: { check: ProfileCheck }) {
	const r = check.resolved;
	return (
		<>
			{check.failures.map((f) => (
				<p class="notice danger" role="alert" key={f.step + f.detail}>
					{STEP_WORDS[f.step]} failed: {f.detail}
				</p>
			))}
			{r && (
				<dl class="facts">
					<dt>Commit</dt>
					<dd>
						<code>{r.sha.slice(0, 12)}</code>
					</dd>
					<Agent name="Claude" installs={r.claude} />
					<Agent name="Codex" installs={r.codex} />
					<dt>setup.sh</dt>
					<dd>{r.setup ? 'Runs once in each new Box' : 'None'}</dd>
					{r.unresolved.map((u) => (
						<>
							<dt>Unchecked</dt>
							<dd class="muted">{u}</dd>
						</>
					))}
				</dl>
			)}
		</>
	);
}

/** The agent profile section: the repo now, what it installs, its last failure, and a form to change it. */
export function Profile({ csrf, githubLinked, onChanged }: { csrf: string; githubLinked: boolean; onChanged: () => void }) {
	const [current, setCurrent] = useState<ProfileCheck | null>(null);
	const [loadError, setLoadError] = useState('');
	const [text, setText] = useState('');
	const [preview, setPreview] = useState<ProfileCheck | null>(null);
	const [busy, setBusy] = useState<'' | 'check' | 'save' | 'remove'>('');
	const [problem, setProblem] = useState('');
	const [saved, setSaved] = useState(false);

	const reload = () =>
		profileGet().then(
			(c) => {
				setCurrent(c);
				setText((typed) => typed || (c.source ?? ''));
			},
			(e: Error) => setLoadError(failure(e)),
		);
	useEffect(() => void reload(), []);

	const parsed = v.safeParse(SourceInput, text);
	const typo = text.trim() && !parsed.success ? parsed.issues[0].message : '';
	const unchanged = parsed.success && parsed.output === (current?.source ?? '');

	const run = async (what: 'check' | 'save' | 'remove') => {
		setBusy(what);
		setProblem('');
		setSaved(false);
		try {
			if (what === 'check' && parsed.success) setPreview(await profileCheck(csrf, parsed.output));
			if (what === 'save' && parsed.success) {
				const c = await profileSet(csrf, parsed.output);
				setCurrent({ ...c, last_failed: current?.last_failed });
				setPreview(null);
				setSaved(true);
				onChanged();
			}
			if (what === 'remove') {
				await profileRemove(csrf);
				setPreview(null);
				setText('');
				await reload();
				onChanged();
			}
		} catch (err) {
			setProblem(failure(err as Error));
		}
		setBusy('');
	};

	return (
		<section>
			<h2>Agent profile</h2>
			<p class="muted">
				A GitHub repo every new Box of yours applies: <code>moor-profile.yaml</code> for the model, plugin marketplaces and plugins, plus <code>AGENTS.md</code>,{' '}
				<code>skills/</code>, <code>claude/settings.json</code> and a <code>setup.sh</code>. Optional.
			</p>
			{!githubLinked && (
				<p class="notice warn">
					moor reads the profile with your GitHub token. <a href="#/settings/secrets">Link GitHub</a> first.
				</p>
			)}
			{loadError && (
				<p class="notice danger" role="alert">
					{loadError}
				</p>
			)}
			{!current && !loadError && <p aria-busy="true">Checking your profile…</p>}
			{current && (
				<ul class="cards">
					<li>
						<div class="row-head">
							<strong>{current.source ? <code>{current.source}</code> : 'No profile'}</strong>
							{current.source && (
								<span class={current.failures.length ? 'pill warn' : 'pill ok'}>{current.failures.length ? 'Does not apply' : 'Applies'}</span>
							)}
						</div>
						{current.source ? <CheckResult check={current} /> : <p class="muted">Boxes start with the agent as moor ships it.</p>}
						{current.last_failed && (
							<p class="notice warn">
								Last failure, in Box <code>{current.last_failed.box_name}</code> on {when(current.last_failed.at_ms)}: {STEP_WORDS[current.last_failed.step]} failed.{' '}
								{current.last_failed.detail}
							</p>
						)}
					</li>
				</ul>
			)}
			<form
				class="settings profile-form"
				onSubmit={(e) => {
					e.preventDefault();
					void run('save');
				}}
			>
				<label>
					Profile repo
					<input
						placeholder="https://github.com/you/agent-profile#main"
						autocomplete="off"
						spellcheck={false}
						aria-invalid={typo ? 'true' : undefined}
						value={text}
						onInput={(e) => {
							setText(e.currentTarget.value);
							setPreview(null);
						}}
					/>
				</label>
				<p class="muted">{typo || 'Add #ref to pin a branch, tag or commit. Without it, each Box takes the default branch.'}</p>
				{preview && <CheckResult check={preview} />}
				{preview && !preview.failures.length && <p class="notice ok">This profile applies. Save it to use it in your next Box.</p>}
				{problem && (
					<p class="notice danger" role="alert">
						{problem}
					</p>
				)}
				{saved && (
					<p class="notice ok" role="status">
						Saved. Your next Box applies it.
					</p>
				)}
				<div class="actions">
					<button disabled={!!busy || !parsed.success || unchanged || !githubLinked}>{busy === 'save' ? 'Checking and saving…' : 'Save'}</button>
					<button type="button" class="quiet" disabled={!!busy || !parsed.success || !githubLinked} onClick={() => void run('check')}>
						{busy === 'check' ? 'Checking…' : 'Check without saving'}
					</button>
					{current?.source && (
						<button type="button" class="quiet wf-danger" disabled={!!busy} onClick={() => void run('remove')}>
							Remove profile
						</button>
					)}
				</div>
			</form>
		</section>
	);
}
