import { expect, test } from 'bun:test';

import { StatusIcon } from './WorkflowBits';
import { textOf } from './testkit';

/** The attributes of the icon's one span. */
function look(status: Parameters<typeof StatusIcon>[0]['status'], verdict?: Parameters<typeof StatusIcon>[0]['verdict']) {
	const el = StatusIcon({ status, verdict });
	return { glyph: textOf(el), class: el.props.class, title: el.props.title };
}

test('a delivered run with no named outcome keeps the check mark and the word delivered', () => {
	expect(look('success')).toEqual({ glyph: '✓', class: 'status success', title: 'delivered' });
	expect(look('failure')).toEqual({ glyph: '✗', class: 'status failure', title: 'failed' });
});

test('a named outcome picks the icon by its tone and the tooltip names it', () => {
	expect(look('success', { name: 'check passed', tone: 'good' })).toEqual({ glyph: '✓', class: 'status success', title: 'delivered: check passed' });
	expect(look('success', { name: 'check failed', tone: 'bad' })).toEqual({ glyph: '!', class: 'status bad', title: 'delivered: check failed' });
	expect(look('success', { name: 'nothing to do', tone: 'plain' })).toEqual({ glyph: '✓', class: 'status plain', title: 'delivered: nothing to do' });
});
