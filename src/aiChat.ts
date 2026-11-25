import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import './aiChat.css';

type Settings = { endpoint: string; model: string; proactive: boolean; frequency: string };
type ChatState = {
  settings: Settings;
  history: { role: string; content: string }[];
  choices: string[];
  hasKey: boolean;
  muted: boolean;
  error?: string;
};

const byId = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const history = byId('history');
const replies = byId('replies');
const input = byId<HTMLInputElement>('message');
const endpoint = byId<HTMLInputElement>('endpoint');
const model = byId<HTMLInputElement>('model');
const key = byId<HTMLInputElement>('key');
const forgetKey = byId<HTMLInputElement>('forget-key');
const proactive = byId<HTMLInputElement>('proactive');
const frequency = byId<HTMLSelectElement>('frequency');
const status = byId('status');
let muted = false;
let busy = false;

function showConfig(open: boolean) {
  byId('chat').hidden = open;
  byId('config').hidden = !open;
  byId('show-chat').setAttribute('aria-pressed', String(!open));
  byId('show-config').setAttribute('aria-pressed', String(open));
}

async function refresh(fillSettings = false) {
  const state = await invoke<ChatState>('get_ai_chat');
  if (fillSettings) {
    endpoint.value = state.settings.endpoint;
    model.value = state.settings.model;
    proactive.checked = state.settings.proactive;
    frequency.value = state.settings.frequency;
    key.placeholder = state.hasKey ? '已保存，留空保留原密钥' : '不需要密钥时留空';
    byId('forget-row').hidden = !state.hasKey;
    key.value = '';
    forgetKey.checked = false;
  }
  muted = state.muted;
  byId('mute').textContent = muted ? '恢复搭话' : '今天别搭话';
  history.replaceChildren();
  if (!state.history.length) {
    const empty = document.createElement('p');
    empty.className = 'empty';
    empty.textContent = state.settings.endpoint ? '和桌宠打个招呼吧。' : '先在「模型与搭话」里配置模型。';
    history.append(empty);
  }
  for (const message of state.history) {
    const line = document.createElement('p');
    line.className = message.role === 'user' ? 'user' : 'pet';
    line.textContent = message.content;
    history.append(line);
  }
  history.scrollTop = history.scrollHeight;
  replies.replaceChildren();
  for (const choice of state.choices) {
    const button = document.createElement('button');
    button.type = 'button';
    button.textContent = choice;
    button.onclick = () => void send(choice);
    replies.append(button);
  }
  if (state.error) status.textContent = state.error;
  return state;
}

async function run(pending: string, action: () => Promise<string>) {
  if (busy) return;
  busy = true;
  status.textContent = pending;
  const controls = () => document.querySelectorAll<HTMLInputElement | HTMLButtonElement | HTMLSelectElement>('form input, form button, form select, #replies button');
  for (const element of controls()) element.disabled = true;
  try { status.textContent = await action(); }
  catch (error) { status.textContent = String(error); }
  finally {
    busy = false;
    for (const element of controls()) element.disabled = false;
  }
}

function send(text: string) {
  if (!text.trim()) return;
  return run('桌宠正在想怎么回你…', async () => {
    await invoke('send_ai_message', { text, test: false });
    input.value = '';
    await refresh();
    return '';
  });
}

byId('show-chat').onclick = () => showConfig(false);
byId('show-config').onclick = () => showConfig(true);
byId<HTMLFormElement>('send').onsubmit = event => { event.preventDefault(); void send(input.value); };
byId<HTMLFormElement>('settings').onsubmit = event => {
  event.preventDefault();
  void run('正在保存…', async () => {
    const settings: Settings = { endpoint: endpoint.value, model: model.value, proactive: proactive.checked, frequency: frequency.value };
    await invoke('save_ai_settings', { settings, apiKey: forgetKey.checked ? '' : key.value.trim() || null });
    await refresh(true);
    return '设置已保存。';
  });
};
byId('test').onclick = () => void run('正在测试已保存的模型…', async () => {
  await invoke('send_ai_message', { text: '', test: true });
  return '连接成功。';
});
byId('try-opener').onclick = () => void run('桌宠正在想开场白…', async () => {
  await invoke('preview_ai_opener');
  return '';
});
byId('mute').onclick = () => void run('正在保存…', async () => {
  await invoke('mute_ai_today', { mute: !muted });
  await refresh();
  return muted ? '桌宠今天会保持安静。' : '已取消今天的静音。';
});

void (async () => {
  await listen('kuro-ai-open', () => { if (!busy) void refresh().catch(error => { status.textContent = String(error); }); });
  const state = await refresh(true);
  showConfig(!state.settings.endpoint);
})().catch(error => { status.textContent = String(error); });
