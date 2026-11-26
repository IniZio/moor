import { expect, test } from 'bun:test';
import * as v from 'valibot';

import { ReadingViewSchema } from './api';

const COMMENT = 'POST /repos/{repo}/issues/*/comments';

/** A reading as the controller's ReadingView serializes it, with kinds a later controller may add. */
const sent = {
	id: '0193a3e1-7c2b-7d4e-8f00-0123456789ab',
	state: 'read',
	stale: ['prompt', 'secrets'],
	source: 'Post one comment. Never merge. Ping me.',
	reader: { harness: 'claude-code', model: null, instructions: 2 },
	runs: [{ text: 'Post one comment', phrase: 'p1' }, { text: '. ' }, { text: 'Never merge', phrase: 'p2' }, { text: '. ' }, { text: 'Ping me', phrase: 'p3' }, { text: '.' }],
	phrases: [
		{ id: 'p1', mark: 'held', effects: ['e1'], why: 'the Proxy lets one comment through' },
		{ id: 'p2', mark: 'asked', effects: [] },
		{ id: 'p3', mark: 'someday', effects: ['e2'] },
	],
	effects: [
		{ id: 'e1', kind: 'allow', line: COMMENT, key: `allow:github-api:${COMMENT}`, phrases: ['p1'] },
		{ id: 'e2', kind: 'notify', line: 'me', key: 'notify:me', phrases: ['p3'] },
	],
	gates: null,
	may: { 'github-api': [COMMENT] },
};

test('a reading the controller sends parses, and kinds this page does not know draw nothing', () => {
	const r = v.parse(ReadingViewSchema, sent);
	expect(r.id).toBe(sent.id);
	expect(r.reader?.instructions).toBe(2);
	expect(r.phrases.map((p) => p.id)).toEqual(['p1', 'p2']);
	expect(r.phrases[0].why).toBe('the Proxy lets one comment through');
	expect(r.effects.map((e) => e.key)).toEqual([`allow:github-api:${COMMENT}`]);
	expect(r.stale).toEqual(['prompt']);
	expect(r.runs.map((x) => x.text).join('')).toBe(sent.source);
});

test('a reading built from stored gates has no id and no reader', () => {
	const r = v.parse(ReadingViewSchema, { ...sent, id: null, reader: null, stale: [] });
	expect(r.id).toBeNull();
	expect(r.reader).toBeNull();
});
