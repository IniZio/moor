import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import starlightLlmsTxt from 'starlight-llms-txt';
import { remarkRepoDocs } from './src/repo-docs.mjs';

const base = '/moor/';

export default defineConfig({
	site: 'https://inizio.github.io',
	base,
	trailingSlash: 'always',
	markdown: { remarkPlugins: [[remarkRepoDocs, { base }]] },
	integrations: [
		starlight({
			title: 'moor',
			description: 'Run coding agents in Boxes, one piece of work per Box.',
			social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/IniZio/moor' }],
			editLink: { baseUrl: 'https://github.com/IniZio/moor/edit/develop/site/' },
			customCss: [
				'@fontsource-variable/figtree',
				'@fontsource-variable/bricolage-grotesque',
				'@fontsource-variable/jetbrains-mono',
				'../ui/tokens.css',
				'./src/styles/theme.css',
			],
			plugins: [starlightLlmsTxt()],
			sidebar: [
				{ label: 'Get started', items: [{ label: 'What is moor', link: '/' }, 'use'] },
				{ label: 'Use', items: ['mcp', 'herdr', 'machines', 'workflows', 'notify', 'listen'] },
				{ label: 'Self-host', items: ['run', 'deploy', 'kubernetes', 'slack', 'web'] },
				{ label: 'Reference', items: ['cli', 'config'] },
				{ label: 'Concepts', items: ['nodes', 'providers'] },
			],
		}),
	],
});
