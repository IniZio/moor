import { FitAddon, Ghostty, Terminal } from 'ghostty-web';
import wasm from 'ghostty-web/ghostty-vt.wasm?url';
import * as v from 'valibot';

/** How a terminal stands: opening, showing the agent, or over with the reason. */
export type Status = { kind: 'opening' } | { kind: 'live' } | { kind: 'ended'; why: string };

/** Loaded once per page from the bundle's own file, since the page may fetch only from itself. */
let ghostty: Promise<Ghostty> | null = null;

/** The palette colour `name`. The terminal stays dark in both themes, since Claude draws for a dark one. */
function token(name: string): string {
	return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

/** The gate's last text message: why the terminal ended. */
const EndedSchema = v.pipe(v.string(), v.parseJson(), v.object({ ended: v.string() }));

/** Shows the agent's terminal of `box` in `host` and types into it; returns what closes it. */
export async function attach(host: HTMLElement, box: string, csrf: string, onStatus: (s: Status) => void): Promise<() => void> {
	ghostty ??= Ghostty.load(wasm);
	const [loaded] = await Promise.all([ghostty, document.fonts.load("13px 'JetBrains Mono Variable'")]);
	const term = new Terminal({
		ghostty: loaded,
		fontFamily: "'JetBrains Mono Variable', ui-monospace, monospace",
		fontSize: 13,
		cursorBlink: true,
		theme: {
			background: token('--moor-peat-950'),
			foreground: token('--moor-peat-100'),
			cursor: token('--moor-heather-300'),
			selectionBackground: token('--moor-heather-900'),
		},
	});
	const fit = new FitAddon();
	term.loadAddon(fit);
	term.open(host);
	fit.fit();
	const query = new URLSearchParams({ box, cols: String(term.cols), rows: String(term.rows), csrf });
	const scheme = location.protocol === 'https:' ? 'wss' : 'ws';
	const ws = new WebSocket(`${scheme}://${location.host}/account/api/box/terminal?${query}`);
	ws.binaryType = 'arraybuffer';
	const keys = new TextEncoder();
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
	const typed = term.onData((data) => {
		if (ws.readyState === WebSocket.OPEN) ws.send(keys.encode(data));
	});
	const resized = term.onResize(({ cols, rows }) => {
		if (ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ cols, rows }));
	});
	fit.observeResize();
	return () => {
		ws.onclose = null;
		ws.close();
		typed.dispose();
		resized.dispose();
		fit.dispose();
		term.dispose();
	};
}
