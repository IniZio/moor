import { intentsGet } from './api';
import { useLoad } from './useLoad';
import { Failed } from './WorkflowBits';
import './rules.css';

/** Whether the address is the intent table page. */
export function intentsAt(hash: string): boolean {
	return /^#?\/?intents$/.test(hash);
}

/** The intent table as moor ships it, raw and whole: each row's name, its words, the `may` lines a clause naming it adds, and the requests its examples prove. */
export function IntentsPage() {
	const { value, error, reload } = useLoad(intentsGet, 'intents');
	if (error) return <Failed error={error} retry={reload} />;
	if (!value) return <p class="muted" aria-busy="true">Loading the intent table…</p>;
	return (
		<section class="intents-page">
			<h1>Intent table</h1>
			<p class="rules-rev">
				{value.version == null ? 'This moor did not load its table.' : `Version ${value.version}. moor writes an automation's may lines from these rows, never from the reader's own words.`}
			</p>
			<div class="rules-file">
				<div class="rules-head">intents.yaml</div>
				<pre class="intents-raw">{value.text}</pre>
			</div>
		</section>
	);
}
