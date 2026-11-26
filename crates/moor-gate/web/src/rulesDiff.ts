/** One line of a rules diff: kept, added by the change, or taken out by it. */
export type DiffLine = { text: string; kind: 'same' | 'add' | 'del' };

/** The lines of `text`; a closing newline starts no line of its own. */
export function textLines(text: string): string[] {
	return (text.endsWith('\n') ? text.slice(0, -1) : text).split('\n');
}

/** `next` against `prev` along a longest common run: each line kept, added or taken out, in reading order. */
export function diffLines(prev: string[], next: string[]): DiffLine[] {
	const n = prev.length;
	const m = next.length;
	const best: number[][] = Array.from({ length: n + 1 }, () => new Array<number>(m + 1).fill(0));
	for (let i = n - 1; i >= 0; i--) {
		for (let j = m - 1; j >= 0; j--) {
			best[i][j] = prev[i] === next[j] ? best[i + 1][j + 1] + 1 : Math.max(best[i + 1][j], best[i][j + 1]);
		}
	}
	const out: DiffLine[] = [];
	let i = 0;
	let j = 0;
	while (i < n && j < m) {
		if (prev[i] === next[j]) {
			out.push({ text: next[j], kind: 'same' });
			i++;
			j++;
		} else if (best[i + 1][j] >= best[i][j + 1]) out.push({ text: prev[i++], kind: 'del' });
		else out.push({ text: next[j++], kind: 'add' });
	}
	while (i < n) out.push({ text: prev[i++], kind: 'del' });
	while (j < m) out.push({ text: next[j++], kind: 'add' });
	return out;
}

/** The withheld requests an added rule line lets through: `METHOD /path` entries, `{repo}` for the repo and `*` for one path segment. */
export function litRequests(added: string[], requests: string[], repo: string): Set<string> {
	const out = new Set<string>();
	for (const line of added) {
		const entry = line.replace(/^\s*-\s*/, '').replace(/^(["'])(.*)\1$/, '$2').trim();
		if (!/^[A-Z]+ \//.test(entry)) continue;
		const source = entry
			.split(/(\{repo\}|\*)/)
			.map((part) => (part === '{repo}' ? repo.replace(/[.*+?^${}()|[\]\\]/g, '\\$&') : part === '*' ? '[^/]+' : part.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')))
			.join('');
		const rule = new RegExp(`^${source}$`);
		for (const r of requests) if (rule.test(r)) out.add(r);
	}
	return out;
}

/** What moor writes in place of a credential-shaped part when it keeps an agent's words. */
const REDACTED = '[redacted]';

/** The agent's edit of the rules, and what it said around it. */
export type Proposal = { note: string; text: string };

/** The rules in the agent's message `said`: its last ```yaml block, and the words outside the block; none without a block. */
export function proposalOf(said: string): Proposal | null {
	const blocks = [...said.matchAll(/```(?:ya?ml)?[ \t]*\n([\s\S]*?)```/g)];
	const last = blocks.at(-1);
	if (!last || last.index === undefined) return null;
	const note = (said.slice(0, last.index) + said.slice(last.index + last[0].length)).replace(/\s+/g, ' ').trim();
	return { note, text: last[1] };
}

/**
 * `proposal` with each line moor's scrub cut back to the line of `base` it was: the agent's words
 * are kept scrubbed, so a kept line naming a secret comes back with `[redacted]` in it. A line
 * still holding `[redacted]` after this is one the agent wrote anew, which the page refuses to save.
 */
export function unredact(base: string, proposal: string): string {
	const pool = textLines(base);
	const used = new Set<number>();
	const escape = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
	const lines = textLines(proposal).map((line) => {
		if (!line.includes(REDACTED)) return line;
		const kept = new RegExp(`^${line.split(REDACTED).map(escape).join('.+?')}$`);
		const at = pool.findIndex((b, k) => !used.has(k) && kept.test(b));
		if (at < 0) return line;
		used.add(at);
		return pool[at];
	});
	return `${lines.join('\n')}\n`;
}

/** Whether `text` still holds a part moor's scrub took out, so saving it would store the stand-in. */
export function scrubbed(text: string): boolean {
	return text.includes(REDACTED);
}
