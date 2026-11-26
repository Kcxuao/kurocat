import './shortcuts.css';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { fillIcon, loadShortcuts, type ShortcutView } from './shortcutShared';
import { ShortcutEffects, type EffectLayout, type EffectPoint } from './shortcutEffects';

let view: ShortcutView = { items: [], icons: {}, error: null, effect: 'energy' };
type MenuLayout = EffectLayout & { rotates: boolean; first: number; page: number; pages: number; pager: EffectPoint | null };
let layout: MenuLayout | null = null;
let busy = false, wantedOpen = false, loaded = false;
let revealFrame = 0;
let revealDelay = 0;
let layoutSignature = '';
const root = document.getElementById('shortcuts')!;
root.classList.add('shortcut-stage');
const effects = new ShortcutEffects(root, () => {
  if (wantedOpen && layout?.rotates) void invoke('shortcut_orbit_ready').catch(console.error);
});
const buttons = document.createElement('div'); buttons.className = 'shortcut-buttons'; root.append(buttons);
const pager = document.createElement('button'); pager.type = 'button'; pager.className = 'shortcut-pager';
pager.hidden = true;
pager.onclick = () => { if (wantedOpen && !busy) void invoke('shortcut_next_page').catch(console.error); };
root.append(pager);

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
    const local = index - layout!.first;
    const point = layout!.buttons[local]; if (!point) return [];
    const button = document.createElement('button'); button.type = 'button'; button.className = 'radial-shortcut';
    button.setAttribute('aria-label', item.name); button.title = item.name;
    button.style.left = `${(point.x - layout!.x) / layout!.sf - 24}px`;
    button.style.top = `${(point.y - layout!.y) / layout!.sf - 24}px`;
    fillIcon(button, item, view.icons);
    button.addEventListener('click', async () => {
      if (busy || !wantedOpen) return; busy = true; root.classList.add('launching');
      try {
        await effects.click(local);
        if (wantedOpen && button.isConnected) await invoke('launch_shortcut', { id: item.id });
      } catch (error) { console.error('Shortcut launch failed:', error); }
      finally { busy = false; root.classList.remove('launching'); }
    }); buttons.append(button); return [button];
  });
  effects.configure(layout, elements, view.effect);
  pager.hidden = !layout.pager || !wantedOpen;
  if (layout.pager) {
    pager.style.left = `${(layout.pager.x - layout.x) / layout.sf - 24}px`;
    pager.style.top = `${(layout.pager.y - layout.y) / layout.sf - 24}px`;
    pager.textContent = `${layout.page + 1}/${layout.pages} ›`;
    pager.setAttribute('aria-label', `下一页，当前第 ${layout.page + 1} 页，共 ${layout.pages} 页`);
  }
  reveal();
}
async function refresh(): Promise<void> { view = await loadShortcuts(); loaded = true; render(); }
async function start(): Promise<void> {
  await listen<MenuLayout>('kuro-shortcuts-layout', event => {
    const l = event.payload;
    // Native and JS emitters can serialize object keys in different orders.
    const signature = JSON.stringify([l.x, l.y, l.w, l.h, l.sf, l.anchor.x, l.anchor.y, l.first, l.page, l.pages, l.rotates, l.pager?.x, l.pager?.y, ...l.buttons.flatMap(p => [p.x, p.y])]);
    if (signature === layoutSignature) return;
    layoutSignature = signature; layout = event.payload; render();
  });
  await listen<boolean>('kuro-shortcuts-visibility', event => {
    const opening = event.payload && !wantedOpen;
    wantedOpen = event.payload;
    pager.hidden = !wantedOpen || !layout?.pager;
    if (wantedOpen) {
      if (opening && layout) effects.moveButtons(layout.buttons, layout);
      reveal();
    } else {
      cancelAnimationFrame(revealFrame); clearTimeout(revealDelay); effects.hide();
    }
  });
  await listen<EffectPoint[]>('kuro-shortcuts-orbit', ({ payload }) => {
    if (wantedOpen && layout?.rotates) effects.moveButtons(payload, layout);
  });
  await listen('kuro-shortcuts-changed', () => { void refresh(); });
  await refresh(); await invoke('shortcut_layout');
}
document.addEventListener('contextmenu', event => event.preventDefault());
void start().catch(console.error);
