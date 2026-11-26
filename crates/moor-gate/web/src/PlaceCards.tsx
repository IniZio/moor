import { type ComponentChildren, render } from 'preact';

import type { BoxDetail, Evidence, Output, TaskBox } from './api';
import { Markdown } from './Markdown';
import { when } from './WorkflowBits';
import './place.css';

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
	link: (
		<>
			<path d="M6.5 9.5l3-3" />
			<path d="M7 4.5l1.2-1.2a2.5 2.5 0 0 1 3.5 3.5L10.5 8" />
			<path d="M9 11.5l-1.2 1.2a2.5 2.5 0 0 1-3.5-3.5L5.5 8" />
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

function Card({ glyph, name, where, note, href, tail, children }: { glyph: keyof typeof GLYPH; name: string; where?: string; note?: ComponentChildren; href: string; tail?: ComponentChildren; children?: ComponentChildren }) {
	return (
		<>
			{children}
			<div class="t-pl-row">
				<CardGlyph kind={glyph} />
				<span class="t-pl-name">{name}</span>
				{where && <span class="t-pl-where">in {where}</span>}
				<span class="t-pl-sum">{note}</span>
				{tail}
				<a class="btn sm" href={href} target="_blank" rel="noopener">
					Open
				</a>
			</div>
		</>
	);
}

/** The kinds of file whose words a card shows. */
const WORDS = /^text\/|json|yaml|markdown/;

/** Fills a file card's frame with the file's words, once, when the file turns out to be text; the frame stays hidden otherwise. */
function showWords(el: HTMLDivElement | null) {
	const href = el?.dataset.href;
	if (!el || !href) return;
	fetch(href)
		.then(async (res) => (res.ok && WORDS.test(res.headers.get('content-type') ?? '') ? render(<Markdown text={(await res.text()).slice(0, 4000)} />, el) : undefined))
		.catch(() => undefined);
}

const cardId = (name: string, version: number) => `t-art-${encodeURIComponent(name)}-${version}`;

/** Its versions up to its own as chips, the newest one filled; an older version is dimmed and links to the newest. */
function Versions({ a, newest }: { a: Evidence; newest: number }) {
	return (
		<>
			{a.version < newest && (
				<button type="button" class="t-pl-newer" onClick={() => document.getElementById(cardId(a.name, newest))?.scrollIntoView({ block: 'center' })}>
					newer: v{newest}
				</button>
			)}
			<span class="t-pl-chips">
				{Array.from({ length: a.version }, (_, i) => (
					<span class={i + 1 === newest ? 't-pl-chip t-on' : 't-pl-chip'} key={i}>
						v{i + 1}
					</span>
				))}
			</span>
		</>
	);
}

/** A file a Box left: the picture, or the start of its words, inline above its name, its versions and a link that opens it. `newest` is the latest version of the same name. */
export function ArtifactCard({ a, newest = a.version }: { a: Evidence; newest?: number }) {
	return (
		<article class={a.version < newest ? 't-pl-art t-pl-sup' : 't-pl-art'} id={cardId(a.name, a.version)}>
			<Card glyph={a.image ? 'image' : 'doc'} name={a.name} note={a.image ? undefined : kb(a.size)} href={a.href} tail={<Versions a={a} newest={newest} />}>
				{a.image ? (
					<div class="t-pl-pic">
						<img src={a.href} alt={a.name} loading="lazy" />
					</div>
				) : (
					WORDS.test(a.content_type) && <div class="t-pl-pic t-pl-doc" data-href={a.href} ref={showWords} />
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

/** Where an output lives, as its card names it: GitHub, or none for another site. */
const onGitHub = (url: string) => /^https:\/\/github\.com\//.test(url);

/** Something the agent made outside moor, as a card: its title, where it lives and a link that opens it. */
export function OutputCard({ o }: { o: Output }) {
	const github = onGitHub(o.url);
	return (
		<article class="t-pl-out">
			<Card glyph={github ? 'pr' : 'link'} name={o.title} where={github ? 'GitHub' : undefined} href={o.url} />
		</article>
	);
}

/** What the agent made outside moor, one quiet card each: its title opens it, then its kind and when it was first reported. Nothing when there is none. */
export function OutputList({ outputs }: { outputs: Output[] }) {
	if (!outputs.length) return null;
	return (
		<section class="t-pl-outs" aria-label="Outputs">
			<p class="muted">Outputs</p>
			{outputs.map((o) => (
				<article class="t-pl-out" key={o.url}>
					<div class="t-pl-row">
						<CardGlyph kind="link" />
						<a class="t-pl-name" href={o.url} target="_blank" rel="noopener">
							{o.title}
						</a>
						<span class="t-pl-sum">
							{o.kind} · {when(o.first_at)}
						</span>
					</div>
				</article>
			))}
		</section>
	);
}
