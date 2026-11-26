import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { iconUrl } from './shortcutShared';
import './feeding.css';

interface Snack { name: string; kind: 'paper' | 'picture' | 'archive' | 'other'; icon: number[] }
interface Tray { snacks: Snack[]; busy: boolean; waiting: boolean; hungry: boolean; error: string | null; glutton: boolean }
const list = document.getElementById('snacks')!;
const status = document.getElementById('status')!;
const makeHungry = document.getElementById('make-hungry') as HTMLButtonElement;
const tryFeed = document.getElementById('try-feed') as HTMLButtonElement;
const glutton = document.getElementById('glutton') as HTMLInputElement;
const warning = document.getElementById('glutton-warning') as HTMLDialogElement;
let changingGlutton = false;
const fallback = { paper: '📄', picture: '🖼️', archive: '📦', other: '📎' };

async function refresh() {
  const state = await invoke<Tray>('get_feeding');
  list.replaceChildren();
  for (const snack of state.snacks) {
    const row = document.createElement('li');
    const url = iconUrl(snack.icon);
    const icon = document.createElement(url ? 'img' : 'span');
    icon.setAttribute('aria-hidden', 'true');
    if (icon instanceof HTMLImageElement) { icon.src = url!; icon.alt = ''; }
    else icon.textContent = fallback[snack.kind];
    const name = document.createElement('span'); name.textContent = snack.name; name.title = snack.name;
    row.append(icon, name); list.append(row);
  }
  tryFeed.disabled = state.busy || state.waiting || !state.snacks.length || Boolean(state.error);
  glutton.checked = state.glutton;
  glutton.disabled = changingGlutton;
  makeHungry.disabled = state.busy || (Boolean(state.error) && !state.glutton);
  status.textContent = state.error ?? (state.busy ? '桌宠正在享用零食…'
    : state.waiting ? '桌宠准备出发…' : state.hungry ? (state.glutton ? '桌宠饿了，正在找桌面文件' : '桌宠饿了，等待零食') : `${state.snacks.length}/12 份零食`);
}
function showError(error: unknown) { status.textContent = String(error); }
async function setGlutton(enabled: boolean) {
  glutton.disabled = changingGlutton = true;
  try {
    await invoke('set_glutton_mode', { enabled });
    glutton.checked = enabled;
  } catch (error) {
    showError(error);
  } finally {
    glutton.disabled = changingGlutton = false;
  }
}
glutton.onchange = () => {
  if (glutton.checked) {
    glutton.checked = false;
    warning.returnValue = '';
    warning.showModal();
  } else {
    glutton.checked = true;
    void setGlutton(false);
  }
};
warning.onclose = () => {
  if (warning.returnValue === 'enable') void setGlutton(true);
};
makeHungry.onclick = () => {
  void invoke('make_pet_hungry').then(refresh).catch(showError);
};
tryFeed.onclick = () => {
  tryFeed.disabled = true;
  void invoke('try_feeding').then(refresh).catch(error => { tryFeed.disabled = false; showError(error); });
};
void (async () => {
  await listen('kuro-feeding-changed', () => { void refresh().catch(showError); });
  await listen<string>('kuro-feeding-notice', ({ payload }) => { status.textContent = payload; });
  await refresh();
})().catch(showError);
