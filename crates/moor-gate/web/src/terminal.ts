import '@xterm/xterm/css/xterm.css';

import { ClipboardAddon, type IClipboardProvider } from '@xterm/addon-clipboard';
import { FitAddon } from '@xterm/addon-fit';
import { Unicode11Addon } from '@xterm/addon-unicode11';
import { WebLinksAddon } from '@xterm/addon-web-links';
import { WebglAddon } from '@xterm/addon-webgl';
import { Terminal } from '@xterm/xterm';
import * as v from 'valibot';

/** How a terminal stands: opening, showing the agent, or over with the reason. */
export type Status = { kind: 'opening' } | { kind: 'live' } | { kind: 'ended'; why: string };

/** The palette colour `name`. The terminal stays dark in both themes, since Claude draws for a dark one. */
function token(name: string): string {
	return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

/** The gate's last text message: why the terminal ended. */
const EndedSchema = v.pipe(v.string(), v.parseJson(), v.object({ ended: v.string() }));

/** OSC 52 may write the clipboard but never read it, so the Box cannot see what the person copied. */
const clipboard: IClipboardProvider = {
	readText: () => Promise.resolve(''),
	writeText: (_, text) => navigator.clipboard.writeText(text),
};

/** Opens a web link the terminal shows in a new tab, cut off from this page. */
function open(_: MouseEvent, uri: string) {
	if (/^https?:\/\//i.test(uri)) window.open(uri, '_blank', 'noopener,noreferrer');
}

/** What a native terminal sends for the chords the browser terminal would drop or keep to itself. */
function chord(e: KeyboardEvent): string | 'copy' | 'paste' | null {
	const only = (ctrl: boolean, shift: boolean) => e.ctrlKey === ctrl && e.shiftKey === shift && !e.altKey && !e.metaKey;
	if (only(true, true) && e.code === 'KeyC') return 'copy';
	if (only(true, true) && e.code === 'KeyV') return 'paste';
	if (only(false, true) && e.key === 'Enter') return '\x1b\r';
	if (only(true, false) && e.key === '/') return '\x1f';
	return null;
}

/** Draws with WebGL, since the page's style policy blanks the DOM renderer; a lost context gets a new one. */
function draw(term: Terminal) {
	const gl = new WebglAddon();
	gl.onContextLoss(() => {
		gl.dispose();
		draw(term);
	});
	term.loadAddon(gl);
}

/** Shows the agent's terminal of `box` in `host` and types into it; returns what closes it. */
export async function attach(host: HTMLElement, box: string, csrf: string, onStatus: (s: Status) => void): Promise<() => void> {
	await document.fonts.load("13px 'JetBrains Mono Variable'");
	const term = new Terminal({
		fontFamily: "'JetBrains Mono Variable', ui-monospace, monospace",
		fontSize: 13,
		cursorBlink: true,
		scrollback: 10000,
		allowProposedApi: true,
		macOptionClickForcesSelection: true,
		linkHandler: { activate: open },
		theme: {
			background: token('--moor-peat-950'),
			foreground: token('--moor-peat-100'),
			cursor: token('--moor-heather-300'),
			selectionBackground: token('--moor-heather-900'),
		},
	});
	const fit = new FitAddon();
	term.loadAddon(fit);
	term.loadAddon(new Unicode11Addon());
	term.unicode.activeVersion = '11';
	term.loadAddon(new ClipboardAddon(undefined, clipboard));
	term.loadAddon(new WebLinksAddon(open));
	term.open(host);
	draw(term);
	fit.fit();
	const query = new URLSearchParams({ box, cols: String(term.cols), rows: String(term.rows), csrf });
	const scheme = location.protocol === 'https:' ? 'wss' : 'ws';
	const ws = new WebSocket(`${scheme}://${location.host}/account/api/box/terminal?${query}`);
	ws.binaryType = 'arraybuffer';
	const keys = new TextEncoder();
	const raw = (bytes: Uint8Array<ArrayBuffer>) => ws.readyState === WebSocket.OPEN && ws.send(bytes);
	const send = (data: string) => raw(keys.encode(data));
	let why: string | null = null;
	let opened = false;
	ws.onopen = () => {
		opened = true;
		onStatus({ kind: 'live' });
		term.focus();
	};
	ws.onmessage = (e: MessageEvent<unknown>) => {
		if (e.data instanceof ArrayBuffer) return term.write(new Uint8Array(e.data));
		const said = v.safeParse(EndedSchema, e.data);
		if (said.success) why = said.output.ended;
	};
	ws.onclose = () => {
		const fallback = opened ? 'The connection dropped. The task keeps running.' : 'The terminal could not open.';
		onStatus({ kind: 'ended', why: why ?? fallback });
	};
	term.attachCustomKeyEventHandler((e) => {
		const sent = e.type === 'keydown' ? chord(e) : null;
		if (sent === null) return true;
		if (sent === 'copy') {
			e.preventDefault();
			if (term.hasSelection()) void navigator.clipboard.writeText(term.getSelection());
		} else if (sent !== 'paste') {
			e.preventDefault();
			send(sent);
		}
		return false;
	});
	const copied = () => term.hasSelection() && void navigator.clipboard.writeText(term.getSelection());
	host.addEventListener('mouseup', copied);
	const typed = term.onData(send);
	const binary = term.onBinary((data) => raw(Uint8Array.from(data, (c) => c.charCodeAt(0))));
	const resized = term.onResize(({ cols, rows }) => {
		if (ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ cols, rows }));
	});
	const watch = new ResizeObserver(() => fit.fit());
	watch.observe(host);
	return () => {
		ws.onclose = null;
		ws.close();
		watch.disconnect();
		host.removeEventListener('mouseup', copied);
		typed.dispose();
		binary.dispose();
		resized.dispose();
		term.dispose();
	};
}
