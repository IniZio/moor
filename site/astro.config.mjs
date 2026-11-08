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
				{ label: 'Get started', items: [{ label: 'What is moor', link: '/' }, 'use', 'run'] },
				{ label: 'Use', items: ['boxes', 'add-machine', 'repo', 'workflows', 'notify'] },
				{
					label: 'Self-host',
					items: ['install', 'providers', 'deploy', 'kubernetes', 'oidc', 'slack', 'upgrade'],
				},
				{ label: 'Reference', items: ['cli', 'config', 'helm', 'mcp', 'events', 'fronts', 'machines'] },
				{ label: 'Concepts', items: ['nodes', 'proxy', 'scheduling'] },
			],
		}),
	],
});
