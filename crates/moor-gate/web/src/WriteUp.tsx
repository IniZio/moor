import { useEffect, useState } from 'preact/hooks';

import type { Line } from './api';
import { Markdown } from './Markdown';

type Evidence = Line['evidence'][number];

/** The evidence a person reads rather than downloads: a markdown file the agent wrote, with a link to fetch it. */
export function writeUpsOf(evidence: Evidence[]): { name: string; href: string }[] {
	return evidence.flatMap((e) => (e.href && /\.md$/i.test(e.name) ? [{ name: e.name, href: e.href }] : []));
}

/** One write-up, read as the signed-in person and shown inline as markdown. */
export function WriteUp({ name, href }: { name: string; href: string }) {
	const [text, setText] = useState<string | null>(null);
	const [problem, setProblem] = useState('');
	useEffect(() => {
		fetch(href).then(
			async (res) => (res.ok ? setText(await res.text()) : setProblem(`${name} could not be read (${res.status}).`)),
			() => setProblem(`${name} could not be read.`),
		);
	}, [href]);
	return (
		<section class="writeup" aria-label={name}>
			<p class="writeup-name">{name}</p>
			{problem ? <p class="muted">{problem}</p> : text == null ? <p class="muted" aria-busy="true">Reading {name}…</p> : <Markdown text={text} />}
		</section>
	);
}
