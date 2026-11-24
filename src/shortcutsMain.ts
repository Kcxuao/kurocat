import './shortcuts.css';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { fillIcon, loadShortcuts, type ShortcutView } from './shortcutShared';
interface Layout { buttons: { x: number; y: number }[]; x: number; y: number; sf: number }
let view: ShortcutView = { items: [], icons: {}, error: null };
let layout: Layout | null = null;
let busy = false;
const root = document.getElementById('shortcuts')!;
function render(): void {
  root.replaceChildren();
  if (!layout) return;
  view.items.forEach((item, index) => {
    const point = layout!.buttons[index]; if (!point) return;
    const button = document.createElement('button'); button.type = 'button'; button.className = 'radial-shortcut';
    button.setAttribute('aria-label', item.name); button.title = item.name;
    button.style.left = `${(point.x - layout!.x) / layout!.sf - 24}px`;
    button.style.top = `${(point.y - layout!.y) / layout!.sf - 24}px`;
    fillIcon(button, item, view.icons);
    button.addEventListener('click', async () => {
      if (busy) return; busy = true; button.disabled = true;
      try { await invoke('launch_shortcut', { id: item.id }); }
      catch (error) { console.error('Shortcut launch failed:', error); }
      finally { busy = false; button.disabled = false; }
    }); root.append(button);
  });
}
async function refresh(): Promise<void> { view = await loadShortcuts(); render(); }
async function start(): Promise<void> {
  await listen<Layout>('kuro-shortcuts-layout', (event) => { layout = event.payload; render(); });
  await listen('kuro-shortcuts-changed', () => { void refresh(); });
  await refresh(); await invoke('shortcut_layout');
}
document.addEventListener('contextmenu', event => event.preventDefault());
void start().catch(console.error);
