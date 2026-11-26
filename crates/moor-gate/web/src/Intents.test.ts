import { expect, test } from 'bun:test';

import { intentsAt } from './Intents';

test('the intent table page is at #/intents alone', () => {
	expect(intentsAt('#/intents')).toBe(true);
	expect(intentsAt('#intents')).toBe(true);
	expect(intentsAt('#/intents/comment')).toBe(false);
	expect(intentsAt('#/repos/acme/web/rules')).toBe(false);
});
