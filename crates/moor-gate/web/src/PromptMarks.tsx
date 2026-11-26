import type { ReadingView } from './api';
import type { Hover } from './promptFile';

/** The saved prompt drawn from a reading's runs, so any script draws as written: solid under what moor holds the agent to, dashed under what is only asked. A phrase lights while the pointer or focus is on it, or on a line of the file that holds it. */
export function PromptMarks({ reading, lit, onHover, onEdit }: { reading: ReadingView; lit: ReadonlySet<string>; onHover: (on: Hover) => void; onEdit?: (e: { currentTarget: HTMLElement }) => void }) {
	const phrases = new Map(reading.phrases.map((p) => [p.id, p]));
	return (
		<div class="pmarks" role="group" aria-label="Prompt" onClick={onEdit}>
			{reading.runs.map((run, i) => {
				const phrase = run.phrase ? phrases.get(run.phrase) : undefined;
				if (!phrase) return run.text;
				const on = { phrase: phrase.id };
				return (
					<span
						key={i}
						class={`psp ${phrase.mark === 'held' ? 'k-enf' : 'k-ask'}${lit.has(phrase.id) ? ' nlit' : ''}`}
						data-id={phrase.id}
						tabIndex={0}
						onMouseEnter={() => onHover(on)}
						onMouseLeave={() => onHover(null)}
						onFocus={() => onHover(on)}
						onBlur={() => onHover(null)}
					>
						{run.text}
					</span>
				);
			})}
		</div>
	);
}
