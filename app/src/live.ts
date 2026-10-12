import './app.css';
import { mount } from 'svelte';
import Live from './Live.svelte';

export default mount(Live, { target: document.getElementById('app')! });
