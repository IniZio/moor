/** A small markdown reader for agent messages: it parses to data, never to HTML. */

export type Inline =
	| { t: 's'; s: string }
	| { t: 'code'; s: string }
	| { t: 'b' | 'i'; c: Inline[] }
	| { t: 'a'; href: string; c: Inline[] };

export type MdBlock =
	| { t: 'code'; s: string; lang: string }
	| { t: 'ul' | 'ol'; items: Inline[][] }
	| { t: 'p' | 'q'; lines: Inline[][] }
	| { t: 'h'; c: Inline[] };

/** A link only ever goes to http or https; anything else stays plain text. */
export function safeHref(raw: string): string | null {
	try {
		const u = new URL(raw);
		return u.protocol === 'https:' || u.protocol === 'http:' ? u.href : null;
	} catch {
		return null;
	}
}

const INLINE = /`([^`\n]+)`|\*\*([^*\n]+?)\*\*|\*([^*\s][^*\n]*?)\*|\[([^\]\n]+)\]\(([^)\s]+)\)|(https?:\/\/[^\s<>]+)/g;

/** The inline marks of one line: code, bold, italic, links and bare links. */
export function inline(text: string): Inline[] {
	const out: Inline[] = [];
	let at = 0;
	for (const m of text.matchAll(INLINE)) {
		const start = m.index ?? 0;
		if (start > at) out.push({ t: 's', s: text.slice(at, start) });
		at = start + m[0].length;
		if (m[1] !== undefined) out.push({ t: 'code', s: m[1] });
		else if (m[2] !== undefined) out.push({ t: 'b', c: inline(m[2]) });
		else if (m[3] !== undefined) out.push({ t: 'i', c: inline(m[3]) });
		else if (m[4] !== undefined && m[5] !== undefined) {
			const href = safeHref(m[5]);
			out.push(href ? { t: 'a', href, c: inline(m[4]) } : { t: 's', s: m[0] });
		} else if (m[6] !== undefined) {
			const tail = /[.,;:!?)\]]+$/.exec(m[6])?.[0] ?? '';
			const url = m[6].slice(0, m[6].length - tail.length);
			const href = safeHref(url);
			out.push(href ? { t: 'a', href, c: [{ t: 's', s: url }] } : { t: 's', s: url });
			at -= tail.length;
		}
	}
	if (at < text.length) out.push({ t: 's', s: text.slice(at) });
	return out;
}

const FENCE = /^\s{0,3}```\s*([\w+-]*)\s*$/;
const ITEM = /^\s*([-*+]|\d+[.)])\s+(.*)$/;
const HEAD = /^\s{0,3}#{1,6}\s+(.*)$/;
const QUOTE = /^\s{0,3}>\s?(.*)$/;

/** The blocks of a message. An unclosed fence runs to the end as code. */
export function parse(source: string): MdBlock[] {
	const lines = source.replace(/\r\n?/g, '\n').split('\n');
	const out: MdBlock[] = [];
	let i = 0;
	let broke = false;
	const para = (kind: 'p' | 'q', line: string) => {
		const last = out[out.length - 1];
		if (!broke && last && last.t === kind) last.lines.push(inline(line));
		else out.push({ t: kind, lines: [inline(line)] });
	};
	while (i < lines.length) {
		const line = lines[i] ?? '';
		const fence = FENCE.exec(line);
		if (fence) {
			const body: string[] = [];
			i += 1;
			while (i < lines.length && !FENCE.test(lines[i] ?? '')) body.push(lines[i++] ?? '');
			i += 1;
			out.push({ t: 'code', s: body.join('\n'), lang: fence[1] ?? '' });
			broke = false;
			continue;
		}
		i += 1;
		if (!line.trim()) {
			broke = true;
			continue;
		}
		const item = ITEM.exec(line);
		const head = HEAD.exec(line);
		const quote = QUOTE.exec(line);
		const wasBroken = broke;
		broke = false;
		if (item) {
			const kind = /^\s*\d/.test(line) ? 'ol' : 'ul';
			const last = out[out.length - 1];
			if (last && last.t === kind) last.items.push(inline(item[2] ?? ''));
			else out.push({ t: kind, items: [inline(item[2] ?? '')] });
		} else if (head) {
			out.push({ t: 'h', c: inline(head[1] ?? '') });
		} else if (quote) {
			broke = wasBroken;
			para('q', quote[1] ?? '');
			broke = false;
		} else {
			broke = wasBroken;
			para('p', line.trim());
			broke = false;
		}
	}
	return out;
}
