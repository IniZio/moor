import { afterAll, afterEach, beforeAll, expect, test } from 'bun:test';

import { follow } from './AskedTask';

/** A scrolling conversation: its content's height, its window's height, and where the reader stands. */
function conversation(content: number, view = 500) {
	const on: Record<string, () => void> = {};
	const child = {} as Element;
	const el = {
		children: [child],
		clientHeight: view,
		scrollHeight: content,
		scrollTop: 0,
		addEventListener: (name: string, fn: () => void) => (on[name] = fn),
		scrollTo({ top }: { top: number }) {
			el.scrollTop = Math.max(0, Math.min(top, el.scrollHeight - el.clientHeight));
			on.scroll?.();
		},
		/** The reader scrolls to `top`. */
		scroll(top: number) {
			el.scrollTop = top;
			on.scroll?.();
		},
	};
	return el;
}

const sizes: { observed: unknown[]; fire: () => void }[] = [];
class Sizes {
	observed: Element[] = [];
	constructor(readonly fire: () => void) {
		sizes.push(this);
	}
	observe(target: Element) {
		this.observed.push(target);
	}
	unobserve() {}
	disconnect() {}
}
class Mutations {
	observe() {}
	disconnect() {}
}
const real = { ResizeObserver: globalThis.ResizeObserver, MutationObserver: globalThis.MutationObserver };
beforeAll(() => Object.assign(globalThis, { ResizeObserver: Sizes, MutationObserver: Mutations }));
afterAll(() => Object.assign(globalThis, real));
afterEach(() => (sizes.length = 0));

/** The conversation's content grows to `height`, as when a picture in it loads. */
function grow(el: ReturnType<typeof conversation>, height: number) {
	el.scrollHeight = height;
	for (const s of sizes) if (s.observed.includes(el.children[0])) s.fire();
}

test('a conversation that opens at its end stays there when a picture loads', () => {
	const el = conversation(400);
	follow(el as unknown as HTMLElement);
	grow(el, 1400);
	expect(el.scrollTop).toBe(900);
});

test('a reader who scrolled up stays put when a picture loads', () => {
	const el = conversation(400);
	follow(el as unknown as HTMLElement);
	grow(el, 1400);
	el.scroll(200);
	grow(el, 1800);
	expect(el.scrollTop).toBe(200);
});
