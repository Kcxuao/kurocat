import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import './mood.css';

interface Mood {
  score: number;
  tier: 'happy' | 'calm' | 'low';
  bored: boolean;
  quietUntil: number;
  pranksEnabled: boolean;
  error: string | null;
}

const tab = document.querySelector<HTMLButtonElement>('#mood-tab')!;
const status = document.querySelector('#mood-status')!;
const score = document.querySelector<HTMLMeterElement>('#mood-score')!;
const scoreLabel = document.querySelector('#mood-score-label')!;
const quiet = document.querySelector<HTMLButtonElement>('#mood-quiet')!;
const quietLabel = document.querySelector('#mood-quiet-label')!;
const previews = document.querySelectorAll<HTMLButtonElement>('[data-mood-preview]');
const pranks = document.querySelector<HTMLButtonElement>('#mood-pranks')!;
const pranksCheck = document.querySelector('#mood-pranks-check')!;
const error = document.querySelector('#mood-menu-error')!;
const labels = { happy: '开心', calm: '平静', low: '心情差' };
let current: Mood | undefined;
let saving = false;
let recentError = "";

function render(mood: Mood) {
  current = mood;
  tab.hidden = false;
  tab.parentElement?.classList.add('has-mood-tab');
  status.textContent = `${labels[mood.tier]}${mood.bored ? ' · 无聊' : ''}`;
  score.value = mood.score;
  scoreLabel.textContent = `${mood.score} / 100`;
  quietLabel.textContent = mood.quietUntil > Date.now() / 1000 ? '恢复捣乱' : '暂时安静 1 小时';
  quiet.disabled = saving || Boolean(mood.error);
  for (const preview of previews) preview.disabled = saving || !mood.pranksEnabled;
  pranks.setAttribute('aria-checked', String(mood.pranksEnabled));
  pranksCheck.textContent = mood.pranksEnabled ? '✓' : '';
  pranks.disabled = saving || Boolean(mood.error);
  error.textContent = mood.error ?? recentError;
}

async function refresh() { render(await invoke<Mood>('get_mood')); }
async function update(command: string, args?: Record<string, unknown>) {
  if (saving || !current) return;
  saving = true;
  recentError = "";
  render(current);
  try {
    await invoke(command, args);
    await refresh();
  } catch (reason) {
    recentError = String(reason);
  } finally {
    saving = false;
    render(current);
  }
}

quiet.onclick = () => { void update('set_mood_quiet', { quiet: (current?.quietUntil ?? 0) <= Date.now() / 1000 }); };
for (const preview of previews) {
  preview.onclick = () => { void update('preview_mood_prank', { kind: preview.dataset.moodPreview }); };
}
pranks.onclick = () => { void update('set_mood_pranks', { enabled: !current?.pranksEnabled }); };

if (navigator.userAgent.includes('Windows')) {
  void (async () => {
    await listen<Mood>('kuro-mood-changed', ({ payload }) => render(payload));
    await listen('kuro-menu-opened', () => { void refresh().catch(() => {}); });
    await refresh();
  })().catch(() => {});
}
