import { expect, test } from 'bun:test';

import { CONTRACT_HREF, ContractVersion, ContractView } from './Contract';
import { sectionAt } from './Settings';
import { all, textOf } from './testkit';

test('a run names the contract version its agent read and links to the contract page', () => {
	const link = ContractVersion({ version: 1 });
	expect(textOf(link)).toBe('contract v1');
	expect(link.props.href).toBe(CONTRACT_HREF);
	expect(sectionAt(CONTRACT_HREF)).toBe('contract');
});

test('the contract page shows the file raw and whole with its version', () => {
	const text = '# moor agent contract\n\nVersion 1\n\n## every\n\n- [ask] A question stops your work.\n';
	const page = ContractView({ contract: { version: 1, sha256: 'ab'.repeat(32), text } });
	expect(textOf(all(page, 'pre')[0])).toBe(text);
	expect(textOf(page)).toContain('Version 1');
});
