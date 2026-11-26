import type { FileLine, Hover } from './promptFile';

/** A head line with its trailing `# comment` set apart, the way an editor greys it. */
function Text({ line }: { line: FileLine }) {
	const m = line.kind === 'head' ? /^(.*?)(\s+# .*)$/.exec(line.text) : null;
	if (!m) return <>{line.text || ' '}</>;
	return (
		<>
			{m[1]}
			<span class="fc">{m[2]}</span>
		</>
	);
}

/** The automation's file, read-only, numbered, with the lines a change touched lit. Given `onHover`, a line says which one the pointer is on and the lines in `lit` are drawn lit. */
export function AutomationFile({ name, lines, dim, lit, onHover }: { name: string; lines: FileLine[]; dim: boolean; lit?: ReadonlySet<number>; onHover?: (on: Hover) => void }) {
	return (
		<section class={dim ? 'file dim' : 'file'} aria-label={name}>
			<div class="fname">{name}</div>
			<div class="fbody">
				{lines.map((l, i) => (
					<div key={i} class={`fl ${l.kind}${l.lit ? ' fadd' : ''}${lit?.has(i) ? ' flit' : ''}`} onMouseEnter={onHover && (() => onHover({ line: i }))} onMouseLeave={onHover && (() => onHover(null))}>
						<Text line={l} />
						{l.lit && <span class="pnew">new</span>}
					</div>
				))}
			</div>
		</section>
	);
}
