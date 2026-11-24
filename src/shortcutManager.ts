import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { fillIcon, loadShortcuts, type Shortcut, type ShortcutView } from './shortcutShared';
import './shortcutManager.css';
export async function setupShortcutManager(): Promise<void> {
  // The command is Windows-only and rejects Win7. Other platforms keep the existing menu.
  let view: ShortcutView;
  try { view = await loadShortcuts(); } catch { return; }
  const panel = document.createElement('section'); panel.className = 'shortcut-manager';
  const title = document.createElement('h3'); title.textContent = '悬停快捷菜单';
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
  panel.append(title, effectRow, previewControls, preview, status, list, addBar);
  document.getElementById('btn-desktop-shortcut')?.before(panel);
  let busy = false;
  let message = '';
  let failedId: string | null = null;
  function button(text: string, action: () => void | Promise<void>): HTMLButtonElement {
    const el = document.createElement('button'); el.type = 'button'; el.textContent = text;
    el.addEventListener('click', event => { event.stopPropagation(); if (!busy) void action(); }); return el;
  }
  async function refresh(): Promise<void> { view = await loadShortcuts(); render(); }
  async function save(items: Shortcut[]): Promise<boolean> {
    try { await invoke('save_shortcuts', { items }); message = '已保存'; failedId = null; await refresh(); return true; }
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
  function render(): void {
    list.replaceChildren();
    effectSelect.value = view.effect; effectSelect.disabled = busy || Boolean(view.error);
    status.textContent = view.error || message || (view.items.length ? `${view.items.length}/8 个入口 · 停留在宠物上展开` : '还没有快捷入口，先添加文件夹或应用');
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
      row.append(icon, name, path, controls); list.append(row);
    });
    addFolder.disabled = addFile.disabled = view.items.length >= 8 || Boolean(view.error) || busy;
    if (view.items.length >= 8 && !message && !view.error) status.textContent = '已达 8 个入口上限';
    if (view.error) panel.querySelectorAll('button').forEach(b => b.disabled = true);
  }
  await listen('kuro-shortcuts-changed', () => { if (!busy) void refresh().catch(error => { message = String(error); render(); }); });
  await listen<{ id: string; message: string }>('kuro-shortcut-error', event => {
    message = event.payload.message.replace(/^INVALID_PATH:/, ''); failedId = event.payload.id; render(); panel.scrollIntoView({ block: 'start' });
  });
  await listen('kuro-menu-opened', () => { void refresh(); }); render();
}
