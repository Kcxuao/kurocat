import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { fillIcon, loadShortcuts, type HotkeyStatus, type Shortcut, type ShortcutView } from './shortcutShared';
import './shortcutManager.css';
export async function setupShortcutManager(): Promise<void> {
  // The command is Windows-only and rejects Win7. Other platforms keep the existing menu.
  let view: ShortcutView;
  try { view = await loadShortcuts(); } catch { return; }
  const panel = document.createElement('section'); panel.className = 'shortcut-manager';
  const tab = document.getElementById('shortcut-tab')!;
  tab.hidden = false;
  tab.parentElement?.classList.add('has-shortcut-tab');
  const hotkeys = document.createElement('input'); hotkeys.type = 'checkbox';
  const hotkeyToggle = document.createElement('label'); hotkeyToggle.className = 'shortcut-hotkey-toggle';
  hotkeyToggle.append(hotkeys, ' 全局快捷键');
  hotkeys.onchange = () => { void operation(() => save(view.items, hotkeys.checked).then(() => {})); };
  const status = document.createElement('p'); status.className = 'shortcut-status'; status.setAttribute('role', 'status');
  const list = document.createElement('div');
  const addFolder = button('添加文件夹', () => add(true));
  const addFile = button('添加应用 / 快捷方式', () => add(false));
  const addBar = document.createElement('div'); addBar.className = 'shortcut-actions'; addBar.append(addFolder, addFile);
  const effectRow = document.createElement('label'); effectRow.className = 'shortcut-effect-row';
  effectRow.append('动画风格');
  const effectSelect = document.createElement('select'); effectSelect.setAttribute('aria-label', '快捷菜单动画风格');
  for (const [value, label] of [['energy', '能量轨道 · 全息'], ['magic', '魔法阵 · 空间裂缝'], ['comic', '漫画爆格']]) {
    const option = document.createElement('option'); option.value = value; option.textContent = label; effectSelect.append(option);
  }
  effectSelect.addEventListener('change', () => { void operation(async () => {
    await invoke('set_shortcut_effect', { effect: effectSelect.value }); await refresh(); message = '动画风格已保存';
  }); });
  effectRow.append(effectSelect);
  const preview = document.createElement('div'); preview.className = 'shortcut-effect-preview';
  const previewControls = document.createElement('div'); previewControls.className = 'shortcut-actions';
  const replay = button('预览动画', async () => {
    const { previewEffect } = await import('./shortcutPreview'); previewEffect(preview, effectSelect.value as ShortcutView['effect'], view);
  });
  const desktopReplay = button('在桌面播放', async () => {
    try { await invoke('preview_shortcut_effect'); } catch (error) { message = String(error); render(); }
  });
  desktopReplay.title = '关闭设置，在宠物周围播放 6 秒真实动画';
  previewControls.append(replay, desktopReplay);
  const animation = document.createElement('details'); animation.className = 'shortcut-animation';
  const animationTitle = document.createElement('summary'); animationTitle.textContent = '悬停动画';
  animation.append(animationTitle, effectRow, previewControls, preview);
  panel.append(addBar, hotkeyToggle, status, list, animation);
  document.getElementById('shortcut-tab-panel')?.append(panel);
  let busy = false;
  let message = '';
  let failedId: string | null = null;
  function button(text: string, action: () => void | Promise<void>): HTMLButtonElement {
    const el = document.createElement('button'); el.type = 'button'; el.textContent = text;
    el.addEventListener('click', event => { event.stopPropagation(); if (!busy) void action(); }); return el;
  }
  async function refresh(): Promise<void> { view = await loadShortcuts(); render(); }
  async function save(items: Shortcut[], hotkeysEnabled?: boolean): Promise<boolean> {
    try { await invoke('save_shortcuts', { items, hotkeysEnabled }); message = '已保存'; failedId = null; await refresh(); return true; }
    catch (error) { message = String(error); status.textContent = message; return false; }
  }
  async function operation(action: () => Promise<void>): Promise<void> {
    if (busy) return; busy = true; panel.setAttribute('aria-busy', 'true'); panel.querySelectorAll('button').forEach(b => b.disabled = true);
    try { await action(); } catch (error) { message = String(error); }
    finally { busy = false; panel.removeAttribute('aria-busy'); render(); }
  }
  async function choose(folder: boolean): Promise<string | null> { return invoke<string | null>('pick_shortcut_target', { folder }); }
  async function add(folder: boolean): Promise<void> {
    await operation(async () => {
      const path = await choose(folder); if (!path) return;
      const basename = path.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || path;
      const name = (folder ? basename : basename.replace(/\.(exe|lnk)$/i, '')).slice(0, 40);
      await save([...view.items, { id: crypto.randomUUID(), name, path }]);
    });
  }
  async function replace(item: Shortcut, folder: boolean): Promise<void> {
    await operation(async () => { const path = await choose(folder); if (path) await save(view.items.map(i => i.id === item.id ? { ...i, path } : i)); });
  }
  const modifiers = ['Alt', 'Ctrl', 'Ctrl+Alt', 'Alt+Shift', 'Ctrl+Shift', 'Ctrl+Alt+Shift'];
  const keys = [...'0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ', ...Array.from({ length: 11 }, (_, i) => `F${i + 1}`)];
  function options(values: string[], label: string): HTMLSelectElement {
    const select = document.createElement('select'); select.setAttribute('aria-label', label);
    for (const value of values) select.add(new Option(value.replaceAll('+', ' + '), value));
    return select;
  }
  function updateBadges(): void {
    const titles: Partial<Record<HotkeyStatus, string>> = {
      conflict: '已被其他应用或系统占用', duplicate: '与其他入口重复', paused: '已暂停',
    };
    for (const badge of list.querySelectorAll<HTMLElement>('[data-hotkey-id]')) {
      const current = view.hotkeyStatus?.[badge.dataset.hotkeyId!];
      const title = current && titles[current];
      badge.textContent = [badge.dataset.chord, title].filter(Boolean).join(' · ');
      badge.classList.toggle('shortcut-hotkey-conflict', current === 'conflict' || current === 'duplicate');
    }
  }
  const maxShortcuts = 24;
  function makeHotkeyEditor(item: Shortcut, index: number): HTMLElement {
    const defaultKey = index < 8 ? `Alt+${index + 1}` : undefined;
    const editor = document.createElement('details'); editor.className = 'shortcut-hotkey';
    const summary = document.createElement('summary');
    summary.dataset.hotkeyId = item.id;
    summary.dataset.chord = item.hotkey === '' ? '快捷键已关闭'
      : (item.hotkey ?? defaultKey ?? '设置快捷键').replaceAll('+', ' + ');
    const optionsRow = document.createElement('div'); optionsRow.className = 'shortcut-actions';
    const modifier = options(modifiers, `${item.name} 的修饰键`);
    const key = options(keys, `${item.name} 的主键`);
    const parts = (item.hotkey || defaultKey || 'Alt+K').split('+');
    key.value = parts.pop()!;
    modifier.value = modifiers.find(value => {
      const candidate = value.split('+');
      return candidate.length === parts.length && candidate.every(part => parts.includes(part));
    }) ?? 'Alt';
    const change = (hotkey?: string) => operation(async () => {
      await save(view.items.map(entry => entry.id === item.id ? { ...entry, hotkey } : entry));
    });
    optionsRow.append(modifier, key,
      button('保存按键', () => change(`${modifier.value}+${key.value}`)),
      button('恢复默认', () => change()),
      button('关闭此键', () => change('')));
    editor.append(summary, optionsRow);
    return editor;
  }
  function render(): void {
    list.replaceChildren();
    hotkeys.checked = view.hotkeysEnabled ?? true;
    hotkeys.disabled = busy || Boolean(view.error);
    effectSelect.value = view.effect; effectSelect.disabled = busy || Boolean(view.error);
    status.textContent = view.error || message || (view.items.length ? `${view.items.length}/${maxShortcuts} 个入口 · 外圈优先排满` : '还没有快捷入口，先添加文件夹或应用');
    view.items.forEach((item, index) => {
      const row = document.createElement('div'); row.className = 'shortcut-row';
      if (item.id === failedId) row.classList.add('shortcut-invalid');
      const icon = document.createElement('span'); icon.className = 'shortcut-icon'; fillIcon(icon, item, view.icons);
      const name = document.createElement('input'); name.value = item.name; name.maxLength = 40; name.setAttribute('aria-label', `${item.name} 的名称`);
      const path = document.createElement('div'); path.className = 'shortcut-path'; path.textContent = item.path; path.title = item.path;
      const controls = document.createElement('div'); controls.className = 'shortcut-actions';
      controls.append(button('改名', async () => { await operation(async () => { if (!await save(view.items.map(i => i.id === item.id ? { ...i, name: name.value } : i))) name.value = item.name; }); }));
      const folder = !/\.(exe|lnk)$/i.test(item.path);
      controls.append(button('重新选择', () => replace(item, folder)));
      const changeIcon = button('更换图标', () => operation(async () => {
        const icon = await invoke<number[] | null>('pick_shortcut_icon');
        if (icon) await save(view.items.map(i => i.id === item.id ? { ...i, icon } : i));
      }));
      changeIcon.title = '选择 PNG、JPG 或 ICO，最大 8 MB / 4096×4096';
      const resetIcon = button('恢复系统图标', () => operation(async () => {
        await save(view.items.map(i => i.id === item.id ? { ...i, icon: null } : i));
      }));
      resetIcon.disabled = !item.icon;
      controls.append(changeIcon, resetIcon);
      // Also allow repairing a shortcut to a target of the other supported kind.
      controls.append(button(folder ? '改选应用' : '改选文件夹', () => replace(item, !folder)));
      const up = button('↑', () => operation(async () => { const next = [...view.items]; [next[index - 1], next[index]] = [next[index], next[index - 1]]; await save(next); }));
      up.setAttribute('aria-label', `上移 ${item.name}`); up.disabled = index === 0;
      const down = button('↓', () => operation(async () => { const next = [...view.items]; [next[index + 1], next[index]] = [next[index], next[index + 1]]; await save(next); }));
      down.setAttribute('aria-label', `下移 ${item.name}`); down.disabled = index === view.items.length - 1;
      controls.append(up, down, button('删除', () => operation(async () => { await save(view.items.filter(i => i.id !== item.id)); })));
      row.append(icon, name, path, makeHotkeyEditor(item, index), controls); list.append(row);
    });
    updateBadges();
    addFolder.disabled = addFile.disabled = view.items.length >= maxShortcuts || Boolean(view.error) || busy;
    if (view.items.length >= maxShortcuts && !message && !view.error) status.textContent = `已达 ${maxShortcuts} 个入口上限`;
    if (view.error) panel.querySelectorAll('button').forEach(b => b.disabled = true);
  }
  await listen('kuro-shortcuts-changed', () => { if (!busy) void refresh().catch(error => { message = String(error); render(); }); });
  await listen<Record<string, HotkeyStatus>>('kuro-shortcut-hotkeys', event => {
    view.hotkeyStatus = event.payload; updateBadges();
  });
  await listen<{ id: string; message: string }>('kuro-shortcut-error', event => {
    message = event.payload.message.replace(/^INVALID_PATH:/, ''); failedId = event.payload.id;
    tab.click(); render();
    panel.querySelector('.shortcut-invalid')?.scrollIntoView({ block: 'nearest' });
  });
  await listen('kuro-menu-opened', () => { void refresh(); }); render();
}
