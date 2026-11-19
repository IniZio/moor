import type { ComponentChildren } from 'preact';

import { type Inline, type MdBlock, parse } from './markdownParse';

function Marks({ marks }: { marks: Inline[] }) {
	return (
		<>
			{marks.map((m, n): ComponentChildren => {
				switch (m.t) {
					case 's':
						return m.s;
					case 'code':
						return <code key={n}>{m.s}</code>;
					case 'b':
						return <strong key={n}><Marks marks={m.c} /></strong>;
					case 'i':
						return <em key={n}><Marks marks={m.c} /></em>;
					case 'a':
						return <a key={n} href={m.href} target="_blank" rel="noopener noreferrer"><Marks marks={m.c} /></a>;
				}
			})}
		</>
	);
}

function Lines({ lines }: { lines: Inline[][] }) {
	return (
		<>
			{lines.map((l, n) => (
				<>
					{n > 0 && <br />}
					<Marks marks={l} />
				</>
			))}
		</>
	);
}

function Block({ b }: { b: MdBlock }) {
	switch (b.t) {
		case 'code':
			return <pre class="md-code"><code>{b.s}</code></pre>;
		case 'ul':
		case 'ol': {
			const List = b.t;
			return <List>{b.items.map((it, n) => <li key={n}><Marks marks={it} /></li>)}</List>;
		}
		case 'q':
			return <blockquote><Lines lines={b.lines} /></blockquote>;
		case 'h':
			return <p class="md-h"><Marks marks={b.c} /></p>;
		case 'p':
			return <p><Lines lines={b.lines} /></p>;
	}
}

/** An agent's message as markdown: text nodes and a few elements, so no raw HTML reaches the page. */
export function Markdown({ text }: { text: string }) {
	return <div class="md">{parse(text).map((b, n) => <Block b={b} key={n} />)}</div>;
}
