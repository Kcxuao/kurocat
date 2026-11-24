import './shortcutEffects.css';
import { ShortcutEffects, type ShortcutEffect } from './shortcutEffects';
import { fillIcon, type ShortcutView } from './shortcutShared';
const previews = new WeakMap<HTMLElement, ShortcutEffects>();
export function previewEffect(root: HTMLElement, effect: ShortcutEffect, view: ShortcutView): void {
  previews.get(root)?.dispose(); root.replaceChildren(); root.classList.add('shortcut-stage');
  const effects = new ShortcutEffects(root); previews.set(root, effects);
  const width = root.clientWidth || 210, height = 200;
  const anchor = { x: width / 2, y: height / 2 };
  const points = Array.from({ length: 4 }, (_, i) => {
    const a = -Math.PI / 2 + i * Math.PI / 2; return { x: anchor.x + 65 * Math.cos(a), y: anchor.y + 65 * Math.sin(a) };
  });
  const pet = document.createElement('span'); pet.className = 'shortcut-preview-pet'; pet.textContent = '🐈‍⬛';
  pet.style.left = `${anchor.x}px`; pet.style.top = `${anchor.y}px`; root.append(pet);
  const buttons = points.map((point, index) => {
    const button = document.createElement('button'); button.className = 'radial-shortcut'; button.type = 'button';
    button.style.left = `${point.x - 24}px`; button.style.top = `${point.y - 24}px`;
    const item = view.items[index]; if (item) fillIcon(button, item, view.icons); else button.textContent = ['📁', '✦', '▣', '↗'][index];
    button.title = '点击体验反馈，不会打开文件'; button.onclick = () => { void effects.click(index); }; root.append(button); return button;
  });
  effects.configure({ buttons: points, anchor, x: 0, y: 0, w: width, h: height, sf: 1 }, buttons, effect);
  requestAnimationFrame(() => effects.show());
}
