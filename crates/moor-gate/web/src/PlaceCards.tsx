import type { ComponentChildren } from 'preact';

import type { BoxDetail, Evidence, TaskBox } from './api';

/** The states of a work Box that still has a terminal to open. */
const OPEN = ['starting', 'running', 'asleep'];

const GLYPH = {
	image: (
		<>
			<rect x="2" y="3" width="12" height="10" rx="1.5" />
			<circle cx="6" cy="7" r="1.2" />
			<path d="M2.5 12l3.5-3 3 2.5 2-1.5 2.5 2" />
		</>
	),
	doc: (
		<>
			<path d="M4 1.5h5l3 3V14H4z" />
			<path d="M9 1.5v3h3" />
		</>
	),
	pr: (
		<>
			<circle cx="4.5" cy="3.5" r="1.5" />
			<circle cx="4.5" cy="12.5" r="1.5" />
			<circle cx="11.5" cy="12.5" r="1.5" />
			<path d="M4.5 5v6M11.5 11V7.5a2 2 0 0 0-2-2H8" />
		</>
	),
};

function CardGlyph({ kind }: { kind: keyof typeof GLYPH }) {
	return (
		<span class="t-pl-glyph">
			<svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
				{GLYPH[kind]}
			</svg>
		</span>
	);
}

/** How long a Box ran, as the line says it: whole minutes, or seconds under one. */
function ran(from: string, to: string): string {
	const seconds = Math.max(0, Math.round((Date.parse(to) - Date.parse(from)) / 1000));
	return seconds < 60 ? `${seconds}s` : `${Math.round(seconds / 60)} min`;
}

/** What the line of a work Box says: the repo, the ready time once there is one, and where the Box stands. */
export function boxWords(b: TaskBox): string {
	const ready = b.ready_ms == null ? [] : [`ready in ${Math.round(b.ready_ms / 1000)}s`];
	const stand = b.state === 'closed' && b.closed_at ? `closed after ${ran(b.started_at, b.closed_at)}` : b.state;
	return ['Box', b.repo, ...ready, stand].join(' · ');
}

/** A work Box as one quiet line under the agent's words; while the Box is open the line opens its terminal. */
export function BoxLine({ box }: { box: TaskBox }) {
	const text = (
		<>
			<span class="t-chev">▸</span> {boxWords(box)}
		</>
	);
	if (!OPEN.includes(box.state)) return <p class="t-pl-box">{text}</p>;
	return (
		<a class="t-pl-box" href={`#/box/${encodeURIComponent(box.name)}/terminal`}>
			{text}
		</a>
	);
}

const kb = (bytes: number) => `${Math.max(1, Math.round(bytes / 1024))} KB`;

function Card({ glyph, name, where, note, href, children }: { glyph: keyof typeof GLYPH; name: string; where?: string; note?: ComponentChildren; href: string; children?: ComponentChildren }) {
	return (
		<>
			{children}
			<div class="t-pl-row">
				<CardGlyph kind={glyph} />
				<span class="t-pl-name">{name}</span>
				{where && <span class="t-pl-where">in {where}</span>}
				<span class="t-pl-sum">{note}</span>
				<a class="btn sm" href={href} target="_blank" rel="noopener">
					Open
				</a>
			</div>
		</>
	);
}

/** A file a work Box left: the picture inline when it is one, then its name and a link that opens it. */
export function ArtifactCard({ a }: { a: Evidence }) {
	return (
		<article class="t-pl-art">
			<Card glyph={a.image ? 'image' : 'doc'} name={a.name} note={a.image ? undefined : kb(a.size)} href={a.href}>
				{a.image && (
					<div class="t-pl-pic">
						<img src={a.href} alt={a.name} loading="lazy" />
					</div>
				)}
			</Card>
		</article>
	);
}

/** The pull request the agent named: its number and a link, and where it stands once the page has read that. */
export function PrCard({ pr, state }: { pr: NonNullable<BoxDetail['pr']>; state?: ComponentChildren }) {
	return (
		<article class="t-pl-pr">
			<Card glyph="pr" name={`Pull request #${pr.number}`} where="GitHub" note={state} href={pr.url} />
		</article>
	);
}
