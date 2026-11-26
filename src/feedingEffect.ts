import { listen } from '@tauri-apps/api/event';
import './feedingEffect.css';

type Kind = 'paper' | 'picture' | 'archive' | 'other';
if (navigator.userAgent.includes('Windows') && '__TAURI_INTERNALS__' in window) {
  const effect = document.createElement('div');
  effect.className = 'feeding-effect';
  effect.setAttribute('aria-hidden', 'true');
  document.body.append(effect);
  const target = document.createElement('div');
  target.className = 'feeding-drop-target';
  target.textContent = '松开移入回收站';
  target.hidden = true;
  document.body.append(target);
  let noticeTimer: number | undefined;
  void listen<boolean>('kuro-feeding-drop-hover', ({ payload }) => {
    window.clearTimeout(noticeTimer);
    target.textContent = '松开移入回收站';
    target.hidden = !payload;
  }).catch(() => {});
  void listen<string>('kuro-feeding-drop-error', ({ payload }) => {
    window.clearTimeout(noticeTimer);
    target.textContent = payload;
    target.hidden = false;
    noticeTimer = window.setTimeout(() => { target.hidden = true; }, 4500);
  }).catch(() => {});
  const particles: Record<Kind, string[]> = {
    paper: ['▱', '◩', '▱'], picture: ['🟨', '🟪', '🟩'], archive: ['◇', '〰', '◇'], other: ['✦', '·', '✧'],
  };
  void listen<{phase: 'walk' | 'eat' | 'stop'; kind: Kind}>('kuro-feeding-effect', ({ payload }) => {
    document.body.dataset.feeding = payload.phase;
    effect.replaceChildren();
    if (payload.phase !== 'eat') return;
    for (const [index, symbol] of particles[payload.kind].entries()) {
      const crumb = document.createElement('span');
      crumb.textContent = symbol;
      crumb.style.setProperty('--dx', `${(index - 1) * 30}px`);
      crumb.style.animationDelay = `${index * 100}ms`;
      effect.append(crumb);
    }
  }).catch(() => {});
}
