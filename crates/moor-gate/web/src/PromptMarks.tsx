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

/** What found an intent's lines or withheld its requests. */
const HOLDS: ReadonlySet<string> = new Set(['lines', 'refused']);

/** What a phrase holds, raw: each `may` line its intents gave, each request its forbidding words withhold, the table they came from, the lines the rules already allow, the request the rules let through despite forbidding words, and the requests moor has no rule for. Only for a phrase that names an intent. */
export function PhraseNote({ reading, phrase, rules }: { reading: ReadingView; phrase: Phrase; rules?: string }) {
	const mine = reading.effects.filter((e) => e.from === 'table' && phrase.effects.includes(e.id));
	const lines = mine.filter((e) => e.kind === 'allow');
	const withheld = mine.filter((e) => e.kind === 'withheld');
	const rows = phrase.intents.filter((i) => HOLDS.has(i.found));
	const claims = mine.length > 0 || rows.some((i) => i.already.length > 0);
	return (
		<span class="pnote" role="tooltip">
			{lines.map((e) => (
				<code key={e.id} class="pline">
					{e.line}
				</code>
			))}
			{withheld.map((e) => (
				<span key={e.id} class="pline">
					<code>{e.line}</code> withheld
				</span>
			))}
			{claims && rows.length > 0 && (
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
			{phrase.allowed && (
				<span class="pfrom">
					the repo's {rules ? <a href={rules} onClick={(e) => e.stopPropagation()}>rules</a> : 'rules'} let <code>{phrase.allowed.request}</code> through under <code>{phrase.allowed.rule}</code>
				</span>
			)}
			{phrase.intents
				.filter((i) => !HOLDS.has(i.found) && i.found !== 'let_through')
				.map((i) => (
					<span key={i.intent} class="pfrom">
						{i.found === 'no_row' ? 'moor has no rule for this' : `moor cannot hold this: ${i.unholdable ? CANNOT[i.unholdable] : 'a line does not check'}`} (<code>{i.intent}</code>)
					</span>
				))}
		</span>
	);
}

/** The saved prompt drawn from a reading's runs, so any script draws as written: solid under what moor holds the agent to, dashed under what is only asked, what the rules let through despite forbidding words, or what moor has no rule for. A phrase lights while the pointer or focus is on it or its note, or on a line of the file that holds it, until Escape; a lit phrase that names an intent shows what it holds, raw. */
export function PromptMarks({ reading, lit, onHover, onEdit, rules }: { reading: ReadingView; lit: ReadonlySet<string>; onHover: (on: Hover) => void; onEdit?: (e: { currentTarget: HTMLElement }) => void; rules?: string }) {
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
						onFocusOut={(e) => {
							if (!e.currentTarget.contains(e.relatedTarget as Node | null)) onHover(null);
						}}
						onKeyDown={(e) => {
							if (e.key !== 'Escape') return;
							e.currentTarget.focus();
							onHover(null);
						}}
					>
						{run.text}
						{shown && phrase.intents.length > 0 && <PhraseNote reading={reading} phrase={phrase} rules={rules} />}
					</span>
				);
			})}
		</div>
	);
}
