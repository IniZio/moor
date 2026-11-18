import { render } from 'preact';

import { App } from './App';
import './app.css';
import './portal.css';
import { startTheme } from './theme';

startTheme();
const root = document.getElementById('app');
if (root) render(<App />, root);
