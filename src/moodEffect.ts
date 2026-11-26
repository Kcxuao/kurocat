import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import './mood.css';

interface Effect { id: number; kind: 'peek' | 'knock' }
const card = document.querySelector<HTMLElement>('#mood-card')!;
const mascot = document.querySelector('#mood-mascot')!;
let shown: number | undefined;

async function refresh() {
  const effect = await invoke<Effect | null>('get_mood_effect');
  if (!effect || effect.id === shown) return;
  shown = effect.id;
  card.hidden = true;
  card.dataset.kind = effect.kind;
  mascot.textContent = effect.kind === 'peek' ? '🐱' : '🐾';
  card.hidden = false;
  await new Promise<void>(resolve => window.requestAnimationFrame(() => resolve()));
  if (shown === effect.id) await invoke('mood_effect_ready', { id: effect.id });
}

void (async () => {
  await listen('kuro-mood-effect', () => { void refresh().catch(() => {}); });
  await refresh();
})().catch(() => {});
