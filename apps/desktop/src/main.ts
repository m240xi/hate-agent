import { mount } from 'svelte';
import './app.css';
import App from './App.svelte';
import DevComponents from './lib/components/DevComponents.svelte';

// `/dev/components` renders the component gallery (both themes, fixture
// data) — the "storybook-lite" that keeps the UI reviewable without a
// designer.
const target = document.getElementById('app');
if (!target) throw new Error('#app element missing');

const isDevRoute = window.location.pathname === '/dev/components';

mount(isDevRoute ? DevComponents : App, { target });
