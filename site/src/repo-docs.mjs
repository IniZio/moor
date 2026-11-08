import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { glob } from 'astro/loaders';

export const repoRoot = fileURLToPath(new URL('../../', import.meta.url));
export const repoBlob = 'https://github.com/IniZio/moor/blob/develop/';
const pattern = ['README.md', 'docs/*.md', '!docs/development.md'];

export function slugFor(repoPath) {
	if (repoPath === 'README.md') return 'index';
	const match = /^docs\/([^/]+)\.md$/.exec(repoPath);
	return match && repoPath !== 'docs/development.md' ? match[1] : null;
}

function firstHeading(filePath) {
	const match = /^# (.+)$/m.exec(readFileSync(filePath, 'utf8'));
	return match?.[1].replaceAll('`', '');
}

export function repoDocsLoader() {
	const inner = glob({
		base: repoRoot,
		pattern,
		generateId: ({ entry }) => slugFor(entry),
	});
	return {
		...inner,
		load: (context) =>
			inner.load({
				...context,
				parseData: (entry) =>
					context.parseData({
						...entry,
						data: { title: firstHeading(entry.filePath), ...entry.data },
					}),
			}),
	};
}

function headingText(node) {
	return (node.children ?? []).map((c) => c.value ?? headingText(c)).join('');
}

function rewrite(url, fromRepoPath, base) {
	if (/^([a-z]+:|#|\/)/i.test(url)) return url;
	const [target, hash] = url.split('#');
	const repoPath = path.posix.normalize(path.posix.join(path.posix.dirname(fromRepoPath), target));
	const slug = slugFor(repoPath);
	const anchor = hash ? `#${hash}` : '';
	if (slug === 'index') return `${base}${anchor}`;
	if (slug) return `${base}${slug}/${anchor}`;
	return `${repoBlob}${repoPath}${anchor}`;
}

export function remarkRepoDocs({ base }) {
	return (tree, file) => {
		const repoPath = path.relative(repoRoot, file.path).split(path.sep).join('/');
		if (slugFor(repoPath) === null) return;
		const first = tree.children.findIndex((n) => n.type !== 'yaml');
		const h1 = tree.children[first];
		if (h1?.type === 'heading' && h1.depth === 1 && headingText(h1)) tree.children.splice(first, 1);
		const visit = (node) => {
			if ((node.type === 'link' || node.type === 'definition') && node.url) {
				node.url = rewrite(node.url, repoPath, base);
			}
			node.children?.forEach(visit);
		};
		visit(tree);
	};
}
