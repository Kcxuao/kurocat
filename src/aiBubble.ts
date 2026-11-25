import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

if (navigator.userAgent.includes('Windows') && '__TAURI_INTERNALS__' in window) {
  const bubble = document.createElement('button');
  bubble.type = 'button';
  bubble.hidden = true;
  bubble.title = '和桌宠接着聊';
  bubble.style.cssText = 'position:fixed;top:4px;left:4px;width:calc(100% - 8px);height:56px;z-index:50;border:1px solid #badd73;border-radius:12px;background:#232a23f5;color:#f3ffdd;padding:5px 8px;cursor:pointer;white-space:normal;overflow:hidden;font:13px system-ui;text-align:center;';
  bubble.onpointerenter = () => { void invoke('ai_bubble', { hovered: true }).catch(() => {}); };
  bubble.onpointerleave = () => { void invoke('ai_bubble', { hovered: false }).catch(() => {}); };
  bubble.onclick = () => {
    void invoke('open_ai_menu').catch(error => {
      bubble.textContent = String(error);
    });
  };
  document.body.append(bubble);
  void listen<{text: string} | null>('kuro-ai-bubble', ({ payload }) => {
    bubble.hidden = !payload;
    bubble.textContent = payload?.text ?? '';
  }).catch(() => {});
}
