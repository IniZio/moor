import { useEffect, useState } from 'preact/hooks';

import type { Line, Run } from './api';
import { Markdown } from './Markdown';

type Evidence = Line['evidence'][number];

/** The evidence a person reads rather than downloads: a markdown file the agent wrote, with a link to fetch it. */
export function writeUpsOf(evidence: Evidence[]): { name: string; href: string }[] {
	return evidence.flatMap((e) => (e.href && /\.md$/i.test(e.name) ? [{ name: e.name, href: e.href }] : []));
}

/** Every write-up a run's steps and lines handed in, once each by link, in the order they came. */
export function runWriteUps(run: Pick<Run, 'steps' | 'lines'>): { name: string; href: string }[] {
	const lines = [...run.steps.flatMap((s) => s.history), ...run.lines];
	const seen = new Map<string, { name: string; href: string }>();
	for (const w of lines.flatMap((l) => writeUpsOf(l.evidence))) seen.set(w.href, w);
	return [...seen.values()];
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
