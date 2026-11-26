import type { ReadingView } from './api';
import type { Hover } from './promptFile';

type Phrase = ReadingView['phrases'][number];

const MARK: Record<Phrase['mark'], string> = { held: 'k-enf', asked: 'k-ask', unmapped: 'k-ask k-unm' };

/** Why moor holds none of an intent's lines. */
const CANNOT = {
	no_rule_for_host: "the repo's rules have no rule for its host",
	no_app: 'this moor has no GitHub App to send it as',
} as const;

/** A table row's name, linked to the table page. */
const Row = ({ name }: { name: string }) => (
	<a href="#/intents" onClick={(e) => e.stopPropagation()}>
		<code>{name}</code>
	</a>
);

/** What a phrase holds, raw: each `may` line its intents gave, the table they came from, the lines the rules already allow, and the requests moor has no rule for. Only for a phrase that names an intent. */
export function PhraseNote({ reading, phrase }: { reading: ReadingView; phrase: Phrase }) {
	const lines = reading.effects.filter((e) => e.kind === 'allow' && e.from === 'table' && phrase.effects.includes(e.id));
	const rows = phrase.intents.filter((i) => i.found === 'lines');
	return (
		<span class="pnote" role="tooltip">
			{lines.map((e) => (
				<code key={e.id} class="pline">
					{e.line}
				</code>
			))}
			{rows.length > 0 && (
				<span class="pfrom">
					from{' '}
					{rows.map((i, k) => (
						<span key={i.intent}>
							{k > 0 && ', '}
							<Row name={i.intent} />
						</span>
					))}
					{reading.table != null && `, table ${reading.table}`}
				</span>
			)}
			{rows.flatMap((i) =>
				i.already.map((l) => (
					<span key={`${i.intent} ${l}`} class="pline already">
						<code>{l}</code> already allowed
					</span>
				)),
			)}
			{phrase.intents
				.filter((i) => i.found !== 'lines')
				.map((i) => (
					<span key={i.intent} class="pfrom">
						{i.found === 'no_row' ? 'moor has no rule for this' : `moor cannot hold this: ${i.unholdable ? CANNOT[i.unholdable] : 'a line does not check'}`} (<code>{i.intent}</code>)
					</span>
				))}
		</span>
	);
}

/** The saved prompt drawn from a reading's runs, so any script draws as written: solid under what moor holds the agent to, dashed under what is only asked or what moor has no rule for. A phrase lights while the pointer or focus is on it, or on a line of the file that holds it; a lit phrase that names an intent shows what it holds, raw. */
export function PromptMarks({ reading, lit, onHover, onEdit }: { reading: ReadingView; lit: ReadonlySet<string>; onHover: (on: Hover) => void; onEdit?: (e: { currentTarget: HTMLElement }) => void }) {
	const phrases = new Map(reading.phrases.map((p) => [p.id, p]));
	return (
		<div class="pmarks" role="group" aria-label="Prompt" onClick={onEdit}>
			{reading.runs.map((run, i) => {
				const phrase = run.phrase ? phrases.get(run.phrase) : undefined;
				if (!phrase) return run.text;
				const on = { phrase: phrase.id };
				const shown = lit.has(phrase.id);
				return (
					<span
						key={i}
						class={`psp ${MARK[phrase.mark]}${shown ? ' nlit' : ''}`}
						data-id={phrase.id}
						tabIndex={0}
						onMouseEnter={() => onHover(on)}
						onMouseLeave={() => onHover(null)}
						onFocus={() => onHover(on)}
						onBlur={() => onHover(null)}
					>
						{run.text}
						{shown && phrase.intents.length > 0 && <PhraseNote reading={reading} phrase={phrase} />}
					</span>
				);
			})}
		</div>
	);
}
