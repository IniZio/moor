import preact from '@preact/preset-vite';
import { defineConfig } from 'vite';

export default defineConfig({
	base: '/account/',
	plugins: [preact()],
	build: {
		outDir: 'dist',
		emptyOutDir: true,
		cssCodeSplit: false,
		rollupOptions: {
			output: {
				entryFileNames: 'assets/app.js',
				assetFileNames: 'assets/app[extname]',
			},
		},
	},
	server: { proxy: { '/account/api': 'http://127.0.0.1:8080', '/auth': 'http://127.0.0.1:8080' } },
});
