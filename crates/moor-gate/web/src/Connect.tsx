import { useState } from 'preact/hooks';

export function Command({ label, line }: { label: string; line: string }) {
	const [copied, setCopied] = useState(false);
	const copy = () =>
		navigator.clipboard.writeText(line).then(() => {
			setCopied(true);
			setTimeout(() => setCopied(false), 1500);
		});
	return (
		<li>
			<div class="row-head">
				<strong>{label}</strong>
				<button class="quiet" onClick={copy}>{copied ? 'Copied' : 'Copy'}</button>
			</div>
			<pre><code>{line}</code></pre>
		</li>
	);
}

export function Connect({ url }: { url: string | null }) {
	return (
		<section>
			<h2>Connect</h2>
			<ul class="cards">
				{url && <Command label="Claude Code" line={`claude mcp add --transport http moor ${url}`} />}
				<li>
					<div class="row-head"><strong>Slack</strong></div>
					<p class="muted">Mention <code>@moor</code> in a channel to start a Box. Nothing to install.</p>
				</li>
			</ul>
		</section>
	);
}
