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
			{url ? (
				<ul class="cards">
					<Command label="Claude Code" line={`claude mcp add --transport http moor ${url}`} />
				</ul>
			) : (
				<p class="muted">This moor has no MCP URL set, so there is no command to copy.</p>
			)}
		</section>
	);
}
