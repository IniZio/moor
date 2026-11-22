import { type ComponentChild, isValidElement, type VNode } from 'preact';
import * as v from 'valibot';

import type { Unanswered } from './gate';

/** The props a test reads off an element. */
type Props = { children?: ComponentChild; href?: string; value?: string; onClick?: () => void };

/** A rendered element. */
export type El = VNode<Props>;

const Text = v.union([v.string(), v.number()]);
const Component = v.function();

/** What an element shows: a component without hooks is called, so the tree is what a person would see. */
function childrenOf(el: El): ComponentChild {
	return v.is(Component, el.type) ? (el.type as (props: Props) => ComponentChild)(el.props) : el.props.children;
}

/** Visits every element of a rendered tree. */
export function walk(node: ComponentChild, visit: (el: El) => void): void {
	if (Array.isArray(node)) {
		for (const child of node) walk(child, visit);
	} else if (isValidElement(node)) {
		const el: El = node;
		visit(el);
		walk(childrenOf(el), visit);
	}
}

/** The text of a rendered tree, as one line. */
export function textOf(node: ComponentChild): string {
	if (Array.isArray(node)) return node.map(textOf).join('');
	if (v.is(Text, node)) return String(node);
	return isValidElement(node) ? textOf(childrenOf(node)) : '';
}

/** The elements of tag `tag` in a rendered tree. */
export function all(node: ComponentChild, tag: string): El[] {
	const found: El[] = [];
	walk(node, (el) => {
		if (el.type === tag) found.push(el);
	});
	return found;
}

/** The two causes the core gives for a stage moor got no answer from, as its tests write them. */
export const NOT_RUN = { check: { gate: { check: 0 } }, command: 'make', reason: { kind: 'not_run', reason: 'Box 3 is busy with another command' } } satisfies Unanswered;
export const LOST = { check: { outcome: { outcome: 'merged', check: 0 } }, command: 'gh pr view', says: 'The pull request is merged.', reason: { kind: 'lost' } } satisfies Unanswered;

/** The button of a rendered tree whose label is `label`. */
export function button(node: ComponentChild, label: string): El | undefined {
	return all(node, 'button').find((b) => textOf(b).trim() === label);
}
