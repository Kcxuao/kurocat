import { invoke } from '@tauri-apps/api/core';
import type { ShortcutEffect } from './shortcutEffects';
export interface Shortcut { id: string; name: string; path: string; icon?: number[] | null; hotkey?: string }
export type HotkeyStatus = 'active' | 'conflict' | 'duplicate' | 'paused';
export interface ShortcutView { hotkeysEnabled?: boolean; hotkeyStatus?: Record<string, HotkeyStatus>; effect: ShortcutEffect; items: Shortcut[]; icons: Record<string, number[]>; error: string | null }
export const loadShortcuts = () => invoke<ShortcutView>('get_shortcuts');
export function iconUrl(bytes?: number[]): string | null {
  if (!bytes || bytes.length !== 4096) return null;
  const canvas = document.createElement('canvas'); canvas.width = canvas.height = 32;
  canvas.getContext('2d')!.putImageData(new ImageData(new Uint8ClampedArray(bytes), 32, 32), 0, 0);
  return canvas.toDataURL('image/png');
}
export function fillIcon(el: HTMLElement, item: Shortcut, icons: Record<string, number[]>): void {
  const url = iconUrl(icons[item.id]);
  if (url) { const img = document.createElement('img'); img.src = url; img.alt = ''; img.width = img.height = 32; el.append(img); }
  else el.textContent = /\.lnk$/i.test(item.path) ? '↗' : /\.exe$/i.test(item.path) ? '▣' : '📁';
}
