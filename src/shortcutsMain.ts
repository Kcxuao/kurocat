import './shortcuts.css';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { fillIcon, loadShortcuts, type ShortcutView } from './shortcutShared';
import { ShortcutEffects, type EffectLayout } from './shortcutEffects';

let view: ShortcutView = { items: [], icons: {}, error: null, effect: 'energy' };
let layout: EffectLayout | null = null;
let busy = false, wantedOpen = false, loaded = false;
let revealFrame = 0;
let revealDelay = 0;
let layoutSignature = '';
const root = document.getElementById('shortcuts')!;
root.classList.add('shortcut-stage');
const effects = new ShortcutEffects(root);
const buttons = document.createElement('div'); buttons.className = 'shortcut-buttons'; root.append(buttons);

function reveal(): void {
  cancelAnimationFrame(revealFrame); clearTimeout(revealDelay);
  if (!wantedOpen || !loaded || !layout) return;
  // Explicit Web Animations also work when data arrives after visibility events.
  revealFrame = requestAnimationFrame(() => {
    revealFrame = requestAnimationFrame(() => {
      // Let the native transparent window reach the compositor before starting.
      revealDelay = window.setTimeout(() => { if (wantedOpen) effects.show(); }, 140);
    });
  });
}
function render(): void {
  effects.dispose(); buttons.replaceChildren(); if (!layout) return;
  const elements = view.items.flatMap((item, index) => {
    const point = layout!.buttons[index]; if (!point) return [];
    const button = document.createElement('button'); button.type = 'button'; button.className = 'radial-shortcut';
    button.setAttribute('aria-label', item.name); button.title = item.name;
    button.style.left = `${(point.x - layout!.x) / layout!.sf - 24}px`;
    button.style.top = `${(point.y - layout!.y) / layout!.sf - 24}px`;
    fillIcon(button, item, view.icons);
    button.addEventListener('click', async () => {
      if (busy || !wantedOpen) return; busy = true; root.classList.add('launching');
      try {
        await effects.click(index);
        if (wantedOpen && button.isConnected) await invoke('launch_shortcut', { id: item.id });
      } catch (error) { console.error('Shortcut launch failed:', error); }
      finally { busy = false; root.classList.remove('launching'); }
    }); buttons.append(button); return [button];
  });
  effects.configure(layout, elements, view.effect); reveal();
}
async function refresh(): Promise<void> { view = await loadShortcuts(); loaded = true; render(); }
async function start(): Promise<void> {
  await listen<EffectLayout>('kuro-shortcuts-layout', event => {
    const l = event.payload;
    // Native and JS emitters can serialize object keys in different orders.
    const signature = JSON.stringify([l.x, l.y, l.w, l.h, l.sf, l.anchor.x, l.anchor.y, ...l.buttons.flatMap(p => [p.x, p.y])]);
    if (signature === layoutSignature) return;
    layoutSignature = signature; layout = event.payload; render();
  });
  await listen<boolean>('kuro-shortcuts-visibility', event => {
    wantedOpen = event.payload;
    if (wantedOpen) reveal(); else { cancelAnimationFrame(revealFrame); clearTimeout(revealDelay); effects.hide(); }
  });
  await listen('kuro-shortcuts-changed', () => { void refresh(); });
  await refresh(); await invoke('shortcut_layout');
}
document.addEventListener('contextmenu', event => event.preventDefault());
void start().catch(console.error);
