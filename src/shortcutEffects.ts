export type ShortcutEffect = 'energy' | 'magic' | 'comic';
export interface EffectPoint { x: number; y: number }
export interface EffectLayout { buttons: EffectPoint[]; x: number; y: number; w: number; h: number; sf: number; anchor: EffectPoint }
type Phase = 'closed' | 'opening' | 'idle' | 'closing' | 'click';
interface Pose extends EffectPoint { scale: number; rotation: number; alpha: number; stretch: number; yaw?: number }
interface Flight { from: EffectPoint; control: EffectPoint; to: EffectPoint }
const clamp = (v: number, min = 0, max = 1) => Math.min(max, Math.max(min, v));
const ease = (t: number) => t < .5 ? 4 * t ** 3 : 1 - (-2 * t + 2) ** 3 / 2;
const mix = (a: number, b: number, t: number) => a + (b - a) * t;

export function flightPoint(flight: Flight, t: number): EffectPoint {
  const u = 1 - clamp(t); t = clamp(t);
  return { x: u * u * flight.from.x + 2 * u * t * flight.control.x + t * t * flight.to.x,
    y: u * u * flight.from.y + 2 * u * t * flight.control.y + t * t * flight.to.y };
}

/** One visible-frame clock drives positions, trails and particles together. */
export class ShortcutEffects {
  private canvas = document.createElement('canvas');
  private ctx: CanvasRenderingContext2D;
  private buttons: HTMLButtonElement[] = [];
  private points: EffectPoint[] = [];
  private flights: Flight[] = [];
  private poses: Pose[] = [];
  private departure: Pose[] = [];
  private anchor = { x: 0, y: 0 };
  private width = 0;
  private height = 0;
  private style: ShortcutEffect = 'energy';
  private phase: Phase = 'closed';
  private elapsed = 0;
  private frame = 0;
  private previousTime = 0;
  private hover = -1;
  private pressed = -1;
  private selected = -1;
  private hoverAmount: number[] = [];
  private done: (() => void) | null = null;

  constructor(private root: HTMLElement, private onOpened?: () => void) {
    this.canvas.className = 'shortcut-fx'; this.canvas.setAttribute('aria-hidden', 'true');
    root.append(this.canvas); this.ctx = this.canvas.getContext('2d')!;
    root.dataset.open = 'false'; root.dataset.phase = 'closed';
  }
  configure(layout: EffectLayout, buttons: HTMLButtonElement[], style: ShortcutEffect): void {
    this.dispose(); this.buttons = buttons; this.style = style;
    this.width = Math.ceil(layout.w / layout.sf); this.height = Math.ceil(layout.h / layout.sf);
    this.anchor = this.inBounds({ x: (layout.anchor.x - layout.x) / layout.sf, y: (layout.anchor.y - layout.y) / layout.sf });
    this.points = layout.buttons.map(p => ({ x: (p.x - layout.x) / layout.sf, y: (p.y - layout.y) / layout.sf }));
    this.flights = this.points.map((to, i) => this.makeFlight(this.anchor, to, i));
    this.hoverAmount = buttons.map(() => 0); this.hover = this.pressed = this.selected = -1;
    const dpr = window.devicePixelRatio || 1;
    this.canvas.width = Math.ceil(this.width * dpr); this.canvas.height = Math.ceil(this.height * dpr);
    this.canvas.style.width = `${this.width}px`; this.canvas.style.height = `${this.height}px`;
    this.ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    this.root.dataset.effect = style;
    buttons.forEach((button, i) => {
      button.onpointerenter = () => { this.hover = i; };
      button.onpointerleave = () => { if (this.hover === i) this.hover = -1; this.pressed = -1; };
      button.onpointerdown = () => { this.pressed = i; };
      button.onpointerup = button.onpointercancel = () => { this.pressed = -1; };
      button.style.opacity = '0';
      button.style.transform = `translate(${this.anchor.x - this.points[i].x}px,${this.anchor.y - this.points[i].y}px) scale(.05)`;
    });
    this.poses = this.points.map(() => ({ ...this.anchor, scale: .05, rotation: 0, alpha: 0, stretch: 1 }));
  }
  moveButtons(positions: EffectPoint[], layout: EffectLayout): void {
    if (positions.length !== this.buttons.length) return;
    this.points = positions.map(p => ({ x: (p.x - layout.x) / layout.sf, y: (p.y - layout.y) / layout.sf }));
    this.points.forEach((p, i) => {
      this.buttons[i].style.left = `${p.x - 24}px`;
      this.buttons[i].style.top = `${p.y - 24}px`;
    });
    this.flights = this.points.map((to, i) => this.makeFlight(this.anchor, to, i));
  }

  show(): void {
    if (!this.buttons.length || this.phase === 'opening' || this.phase === 'idle' || this.phase === 'click') return;
    this.flights = this.points.map((to, i) => this.makeFlight(this.phase === 'closing' ? this.poses[i] : this.anchor, to, i));
    // Choosing an animated launcher style explicitly enables its choreography.
    // Windows' general motion preference must not turn that choice into a static theme.
    this.begin('opening');
    this.root.dataset.open = 'true';
    this.draw(); this.schedule();
  }
  hide(): void {
    if (this.phase === 'closed' || this.phase === 'closing') return;
    this.departure = this.poses.map(p => ({ ...p }));
    this.begin('closing'); this.root.dataset.open = 'false';
    this.draw(); this.schedule();
  }
  click(index: number): Promise<void> {
    if (this.phase === 'closed' || this.phase === 'closing' || this.phase === 'click') return Promise.resolve();
    this.selected = index; this.pressed = -1;
    this.begin('click');
    return new Promise(resolve => {
      this.done = resolve;
      this.draw(); this.schedule();
    });
  }
  dispose(): void {
    cancelAnimationFrame(this.frame); this.phase = 'closed'; this.previousTime = 0;
    this.root.dataset.open = 'false'; this.root.dataset.phase = 'closed';
    this.done?.(); this.done = null;
    this.buttons.forEach(b => { b.style.opacity = '0'; b.classList.remove('activated'); });
    this.ctx.clearRect(0, 0, this.width, this.height);
  }
  private begin(phase: Phase): void {
    cancelAnimationFrame(this.frame); this.done?.(); this.done = null;
    this.phase = phase; this.elapsed = 0; this.previousTime = 0; this.root.dataset.phase = phase;
  }
  private schedule(): void {
    cancelAnimationFrame(this.frame);
    this.frame = requestAnimationFrame(now => {
      // Never skip the choreography after an occluded/native window stops drawing.
      this.elapsed += this.previousTime ? Math.min(50, now - this.previousTime) : 0;
      this.previousTime = now; this.draw();
      if (this.phase !== 'closed') this.schedule();
    });
  }
  private inBounds(p: EffectPoint): EffectPoint {
    return { x: clamp(p.x, Math.min(26, this.width / 2), Math.max(26, this.width - 26)),
      y: clamp(p.y, Math.min(26, this.height / 2), Math.max(26, this.height - 26)) };
  }
  private makeFlight(from: EffectPoint, to: EffectPoint, index: number): Flight {
    const dx = to.x - from.x, dy = to.y - from.y, length = Math.hypot(dx, dy) || 1;
    const bend = Math.min(110, length * .55) * (this.style === 'energy' ? 1 : -1);
    return { from: { x: from.x, y: from.y }, to, control: this.inBounds({
      x: (from.x + to.x) / 2 - dy / length * bend,
      y: (from.y + to.y) / 2 + dx / length * bend + (index % 2 ? 8 : -8),
    }) };
  }
  private flightProgress(index: number, time = this.elapsed): number {
    const prime = this.style === 'energy' ? 180 : 230;
    const duration = this.style === 'energy' ? 850 : 950;
    return clamp((time - prime - index * 65) / duration);
  }
  private openingDuration(): number {
    const stagger = Math.max(0, this.buttons.length - 1);
    if (this.style === 'comic') return 620 + stagger * 84;
    return (this.style === 'energy' ? 1030 : 1180) + stagger * 65;
  }
  private comicEntry(index: number): number {
    return (this.elapsed - 125 - index * 84) / 490;
  }
  private draw(): void {
    const phase = this.phase;
    this.ctx.clearRect(0, 0, this.width, this.height);
    if (phase === 'closed') { this.buttons.forEach(b => b.style.opacity = '0'); return; }
    const magic = this.style === 'magic';
    this.buttons.forEach((button, i) => {
      const target = this.points[i]; let pose: Pose;
      this.hoverAmount[i] += ((i === this.hover ? 1 : 0) - this.hoverAmount[i]) * .2;
      if (this.style === 'comic') {
        pose = this.comicPose(i, target);
      } else if (magic && phase === 'opening') {
        const age = this.riftAge(i);
        const emerge = clamp((age - .21) / .58);
        const turn = 1 - (1 - emerge) ** 2;
        const overshoot = Math.sin(turn * Math.PI) * 14;
        pose = {
          x: target.x + mix(-28, 0, turn) + overshoot,
          y: target.y + mix(13, 0, turn) - overshoot * .65,
          scale: mix(.55, 1, turn), alpha: clamp(emerge * 8), rotation: -18 * (1 - turn),
          yaw: 85 * (1 - turn), stretch: 1,
        };
      } else if (magic && phase === 'closing') {
        const start = this.departure[i];
        const t = clamp((this.elapsed - (this.buttons.length - 1 - i) * 26) / 420);
        const suck = clamp((t - .14) / .86) ** 2;
        pose = {
          x: mix(start.x, target.x - 24, suck), y: mix(start.y, target.y + 10, suck),
          scale: mix(start.scale, .3, suck), alpha: start.alpha * (1 - suck),
          rotation: mix(start.rotation, -16, suck), yaw: mix(start.yaw ?? 0, 88, suck), stretch: 1,
        };
      } else if (phase === 'opening') {
        const t = this.flightProgress(i), u = ease(t), point = flightPoint(this.flights[i], u);
        const snap = t > .78 ? Math.sin((t - .78) / .22 * Math.PI) : 0;
        pose = { ...point, scale: .12 + .88 * clamp(t / .65) + snap * .13,
          rotation: -45 * (1 - u), alpha: clamp(t * 6), stretch: 1 };
      } else if (phase === 'closing') {
        const t = clamp((this.elapsed - (this.buttons.length - 1 - i) * 26) / 420);
        const f = this.makeFlight(this.departure[i], this.anchor, i);
        pose = { ...flightPoint(f, t * t), scale: mix(this.departure[i].scale, .05, t * t),
          rotation: t * 45, alpha: this.departure[i].alpha * (1 - t * t), stretch: 1 };
      } else {
        const hover = this.hoverAmount[i];
        const floating = i === this.hover || phase === 'click' ? 0 : Math.sin(this.elapsed / 650 + i) * 2.5;
        pose = { x: target.x, y: target.y + floating, scale: this.pressed === i ? .84 : 1 + hover * .12,
          rotation: 0, alpha: phase === 'click' && i !== this.selected ? .55 : 1, stretch: 1 };
        if (phase === 'click' && i === this.selected) {
          const charge = clamp(this.elapsed / 150), burst = clamp((this.elapsed - 150) / 300);
          pose.scale = this.elapsed < 150 ? 1 - .22 * charge : 1 + .22 * Math.sin(burst * Math.PI);
          if (magic) pose.yaw = -25 * Math.sin(burst * Math.PI) + 18 * (1 - charge);
        } else if (magic) {
          pose.yaw = -18 * hover;
        }
      }
      this.poses[i] = pose;
      button.style.opacity = String(pose.alpha);
      button.style.transform = `translate(${pose.x - target.x}px,${pose.y - target.y}px) rotate(${pose.rotation}deg) ${pose.yaw ? `perspective(140px) rotateY(${pose.yaw}deg)` : ''} scale(${pose.scale * pose.stretch},${pose.scale / pose.stretch})`;
      button.classList.toggle('activated', phase === 'click' && i === this.selected);
    });
    this.paint();
    if (phase === 'opening' && this.elapsed >= this.openingDuration()) {
      this.begin('idle');
      this.onOpened?.();
    }
    else if (phase === 'closing' && this.elapsed >= 420 + this.buttons.length * 26) { this.begin('closed'); this.draw(); }
    else if (phase === 'click' && this.elapsed >= 450) { this.begin('idle'); this.selected = -1; }
  }
  private comicPose(index: number, target: EffectPoint): Pose {
    const side = target.x < this.anchor.x ? -1 : 1;
    const pose: Pose = { ...target, scale: 1, rotation: 0, alpha: 1, stretch: 1 };
    if (this.phase === 'opening') {
      const age = this.comicEntry(index);
      const pop = clamp((age - .19) / .81);
      const snap = Math.sin(pop * Math.PI) * (1 - pop);
      const direction = target.y < this.anchor.y ? -1 : 1;
      pose.x += side * (-28 * (1 - pop) + 30 * snap);
      pose.y += direction * (-34 * (1 - pop) + 45 * snap);
      pose.scale = .35 + .65 * pop + .9 * snap;
      pose.rotation = side * (-28 * (1 - pop) + 38 * snap);
      pose.alpha = clamp(pop * 7);
    } else if (this.phase === 'closing') {
      const start = this.departure[index];
      const t = clamp((this.elapsed - (this.buttons.length - 1 - index) * 26) / 420);
      const fold = t * t;
      pose.x = mix(start.x, this.anchor.x, fold);
      pose.y = mix(start.y, this.anchor.y, fold);
      pose.scale = mix(start.scale, .06, fold);
      pose.rotation = mix(start.rotation, 34 * side, fold);
      pose.yaw = mix(start.yaw ?? 0, 86 * side, t);
      pose.alpha = start.alpha * (1 - t);
    } else if (this.phase === 'click' && index === this.selected) {
      const burst = clamp((this.elapsed - 90) / 360);
      const impact = Math.sin(burst * Math.PI) * (1 - burst);
      pose.x += side * 15 * impact;
      pose.rotation = side * 14 * impact;
      pose.scale = .84 + .16 * burst + .72 * impact;
    } else {
      const hover = this.hoverAmount[index];
      pose.y -= 7 * hover;
      pose.rotation = side * -7 * hover;
      pose.scale = this.pressed === index ? .84 : 1 + hover * .1;
    }
    return pose;
  }
  private paintComic(): void {
    const c = this.ctx;
    for (const [i, target] of this.points.entries()) {
      const pose = this.poses[i];
      const age = this.comicEntry(i);
      const opening = this.phase === 'opening';
      const side = target.x < this.anchor.x ? -1 : 1;
      const flipped = opening ? clamp(age / .42) : 1;
      const click = this.phase === 'click' && this.selected === i;
      const burst = clamp((this.elapsed - 90) / 360);
      const flash = click ? 1 - burst : 0;
      const hover = this.hoverAmount[i];
      const alpha = opening ? clamp(age * 7) : pose.alpha;
      if (alpha <= 0) continue;
      c.save();
      c.globalAlpha = alpha;
      const center = opening ? {
        x: mix(this.anchor.x, target.x, flipped),
        y: mix(this.anchor.y, target.y, flipped),
      } : pose;
      c.translate(center.x, center.y);
      c.rotate(side * (-.65 * (1 - flipped) + .10 - hover * .22) + flash * .18);
      const panelSize = opening ? mix(.18, 1, flipped) : pose.scale;
      c.scale(panelSize, panelSize);
      c.fillStyle = '#121319';
      c.fillRect(-25, -23, 57, 56);
      c.beginPath();
      c.moveTo(-30, -25); c.lineTo(25, -30); c.lineTo(31, 25); c.lineTo(-24, 30); c.closePath();
      c.fillStyle = flash || hover > .35 ? '#ffe234' : '#fffaf0';
      c.strokeStyle = '#121319'; c.lineWidth = 3;
      c.fill(); c.stroke();
      c.save();
      c.clip();
      c.fillStyle = 'rgba(18,19,25,.25)';
      for (let y = 10; y < 32; y += 5) {
        for (let x = -30; x < 32; x += 5) c.fillRect(x, y, 2, 2);
      }
      c.restore();
      c.restore();
      const impact = click && this.elapsed >= 90 ? burst : opening && age > .21 && age < .65 ? (age - .21) / .44 : null;
      if (impact === null) continue;
      c.save();
      c.translate(pose.x, pose.y);
      c.globalAlpha = Math.min(1, (1 - impact) * 1.8);
      c.fillStyle = '#ffdb21'; c.strokeStyle = '#15141b'; c.lineWidth = 1.4;
      for (let ray = 0; ray < 12; ray++) {
        c.save(); c.rotate(ray * Math.PI / 6 + i);
        const radius = 26 + impact * 32;
        c.beginPath(); c.moveTo(radius, -2.8); c.lineTo(radius + 13 + ray % 3 * 5, 0);
        c.lineTo(radius, 2.8); c.closePath(); c.fill(); c.stroke(); c.restore();
      }
      c.restore();
    }
  }
  private color(alpha: number, white = false): string {
    return `rgba(${white ? '235,255,255' : this.style === 'energy' ? '64,224,255' : '210,142,255'},${clamp(alpha)})`;
  }
  private strokeArc(center: EffectPoint, radius: number, from: number, to: number, alpha: number, width = 1.5): void {
    const c = this.ctx; c.strokeStyle = this.color(alpha); c.lineWidth = width;
    c.beginPath(); c.arc(center.x, center.y, Math.max(.1, radius), from, to); c.stroke();
  }
  private star(p: EffectPoint, size: number, alpha: number): void {
    const c = this.ctx; c.fillStyle = this.color(alpha, true);
    c.beginPath(); c.moveTo(p.x, p.y - size); c.lineTo(p.x + size * .26, p.y - size * .26);
    c.lineTo(p.x + size, p.y); c.lineTo(p.x + size * .26, p.y + size * .26);
    c.lineTo(p.x, p.y + size); c.lineTo(p.x - size * .26, p.y + size * .26);
    c.lineTo(p.x - size, p.y); c.lineTo(p.x - size * .26, p.y - size * .26); c.closePath(); c.fill();
  }
  private path(flight: Flight, end: number, alpha: number, width: number): void {
    const c = this.ctx; c.strokeStyle = this.color(alpha); c.lineWidth = width;
    c.beginPath(); const start = flightPoint(flight, 0); c.moveTo(start.x, start.y);
    for (let t = .025; t <= end + .025; t += .025) { const p = flightPoint(flight, Math.min(end, t)); c.lineTo(p.x, p.y); }
    c.stroke();
  }
  private riftAge(index: number): number {
    return (this.elapsed - 190 - index * 65) / 850;
  }
  private paintRift(target: EffectPoint, width: number, alpha: number, burst = 0): void {
    if (alpha <= 0) return;
    const c = this.ctx;
    c.save(); c.translate(target.x, target.y); c.rotate(-.27);
    c.shadowBlur = 14; c.strokeStyle = this.color(alpha, true); c.lineWidth = 1.7;
    c.beginPath();
    c.moveTo(0, -37);
    c.lineTo(width * .68, -20); c.lineTo(width, -5);
    c.lineTo(width * .52, 15); c.lineTo(0, 37);
    c.lineTo(-width * .65, 23); c.lineTo(-width, 1); c.lineTo(-width * .45, -11);
    c.closePath();
    c.globalCompositeOperation = 'source-over';
    c.fillStyle = `rgba(14,4,34,${alpha * .9})`; c.fill(); c.stroke();
    c.globalCompositeOperation = 'lighter';
    for (let i = 0; i < 10; i++) {
      const side = i % 2 ? 1 : -1;
      const x = side * (width + 7 + (i % 3) * 5 + burst * (32 + i * 3));
      const y = (i - 4.5) * 6 + side * burst * 18;
      c.fillStyle = this.color(alpha * (.35 + (i % 3) * .24), i % 3 === 0);
      c.beginPath(); c.moveTo(x, y - 5); c.lineTo(x + side * 6, y + 1);
      c.lineTo(x - side * 2, y + 4); c.closePath(); c.fill();
    }
    c.restore();
  }
  private paint(): void {
    if (this.style === 'comic') {
      this.paintComic();
      return;
    }
    const c = this.ctx, time = this.elapsed / 1000, magic = this.style === 'magic';
    const open = this.phase === 'opening', close = this.phase === 'closing';
    const radius = Math.min(80, Math.max(35, ...this.points.map(p => Math.hypot(p.x - this.anchor.x, p.y - this.anchor.y) * .42)));
    const prime = open ? clamp(this.elapsed / 250) : 1;
    const fade = close ? 1 - clamp(this.elapsed / 650) : 1;
    c.save(); c.globalCompositeOperation = 'lighter'; c.shadowColor = this.color(1); c.shadowBlur = 12;
    // First beat: an expanding wave and a rapidly traced hologram / summoning seal.
    if (!magic && open && this.elapsed < 650) {
      const wave = clamp(this.elapsed / 650);
      this.strokeArc(this.anchor, 8 + wave * radius * 1.8, 0, Math.PI * 2, (1 - wave) * .9, 3 * (1 - wave) + .5);
      for (let i = 0; i < 16; i++) {
        const angle = i * Math.PI / 8 + (magic ? time : -time);
        this.star({ x: this.anchor.x + Math.cos(angle) * (10 + wave * radius), y: this.anchor.y + Math.sin(angle) * (10 + wave * radius) }, 1 + (1 - wave) * 3, (1 - wave) * .9);
      }
    }
    this.strokeArc(this.anchor, radius * prime, -Math.PI / 2, -Math.PI / 2 + Math.PI * 2 * prime, .4 * fade);
    c.save(); c.translate(this.anchor.x, this.anchor.y); c.rotate(time * (magic ? -.55 : .85));
    c.setLineDash(magic ? [3, 9] : [22, 7, 3, 7]);
    this.strokeArc({ x: 0, y: 0 }, radius * .82, 0, Math.PI * 2 * prime, .7 * fade); c.setLineDash([]);
    if (magic) {
      for (let i = 0; i < 12; i++) {
        c.save(); c.rotate(i * Math.PI / 6); c.translate(radius * .94, 0);
        c.strokeStyle = this.color(.8 * fade); c.lineWidth = 1.5;
        c.beginPath(); c.moveTo(-4, -6); c.lineTo(4, 0); c.lineTo(-4, 6); c.moveTo(-5, 0); c.lineTo(5, 0); c.stroke(); c.restore();
      }
      c.rotate(-time * .5); c.strokeStyle = this.color(.22 * fade); c.beginPath();
      for (let i = 0; i <= 6; i++) { const a = i * Math.PI * 2 / 3; const x = Math.cos(a) * radius * .65, y = Math.sin(a) * radius * .65; if (i) c.lineTo(x, y); else c.moveTo(x, y); } c.stroke();
    } else this.strokeArc({ x: 0, y: 0 }, radius + 7, -.5, .4, .85 * fade, 4);
    c.restore();
    this.points.forEach((target, i) => {
      if (magic) {
        const hover = this.hoverAmount[i];
        if (open) {
          const age = this.riftAge(i);
          const aperture = clamp(age / .27) * (1 - clamp((age - .65) / .35));
          this.paintRift(target, 1 + aperture * 25, aperture);
        } else if (close) {
          const t = clamp((this.elapsed - (this.buttons.length - 1 - i) * 26) / 420);
          const aperture = Math.sin(t * Math.PI);
          this.paintRift(target, 1 + aperture * 23, aperture);
        } else if (this.phase === 'click' && i === this.selected) {
          const burst = clamp((this.elapsed - 130) / 320);
          const flash = clamp(this.elapsed / 130) * (1 - burst);
          this.paintRift(target, 2 + flash * 36, flash, burst);
        } else if (hover > .02) {
          this.paintRift(target, 4 + hover * 11, hover * .8);
        }
        return;
      }
      const pose = this.poses[i], flight = this.flights[i], t = open ? this.flightProgress(i) : 1;
      const u = ease(t), hover = this.hoverAmount[i];
      if (open && t <= 0) return;
      this.path(flight, open ? u : 1, (.13 + hover * .5) * fade, 1);
      // The trail is sampled from exactly the same trajectory and time as its icon.
      if (open && t < 1 || close) {
        for (let j = 1; j <= 18; j++) {
          let p: EffectPoint;
          if (open) p = flightPoint(flight, ease(this.flightProgress(i, this.elapsed - j * 12)));
          else { const back = clamp((this.elapsed - (this.buttons.length - 1 - i) * 26 - j * 10) / 420); p = flightPoint(this.makeFlight(this.departure[i], this.anchor, i), back * back); }
          c.fillStyle = this.color((1 - j / 19) * .6 * pose.alpha); c.beginPath(); c.arc(p.x, p.y, 1 + (1 - j / 19) * 3.5, 0, Math.PI * 2); c.fill();
        }
      }
      if (open && t > .78 && t < 1) this.strokeArc(target, 22 + (t - .78) / .22 * 18, 0, Math.PI * 2, (1 - t) * 3, 2);
      if (!open && !close) {
        const orbit = time * 1.2 + i;
        this.strokeArc(target, 29, orbit, orbit + Math.PI * .65, .6, 1.5);
        const dot = flightPoint(flight, (time * .28 + i * .17) % 1);
        c.fillStyle = this.color(.9, true); c.beginPath(); c.arc(dot.x, dot.y, 2.5, 0, Math.PI * 2); c.fill();
      }
      if (hover > .02 && !close) {
        c.save(); c.translate(target.x, target.y); c.rotate((1 - hover) * -.5);
        c.strokeStyle = this.color(hover, true); c.lineWidth = 2;
        for (const [x, y] of [[-1, -1], [1, -1], [1, 1], [-1, 1]]) { c.beginPath(); c.moveTo(x * 22, y * 34); c.lineTo(x * 34, y * 34); c.lineTo(x * 34, y * 22); c.stroke(); }
        c.restore();
      }
    });
    if (!magic && this.phase === 'click' && this.selected >= 0) {
      const p = this.points[this.selected], charge = clamp(this.elapsed / 150), burst = clamp((this.elapsed - 150) / 300);
      if (this.elapsed < 150) {
        this.strokeArc(p, 42 - charge * 17, 0, Math.PI * 2, charge, 3);
        this.star(p, 4 + charge * 13, charge * .7);
      } else {
        for (let j = 0; j < 3; j++) this.strokeArc(p, 22 + burst * (62 + j * 16), 0, Math.PI * 2, (1 - burst) * (.9 - j * .2), 3 - j * .6);
        for (let i = 0; i < 24; i++) {
          const angle = i * Math.PI / 12, distance = 25 + burst * (35 + i % 3 * 12);
          const spark = { x: p.x + Math.cos(angle) * distance, y: p.y + Math.sin(angle) * distance };
          c.strokeStyle = this.color(1 - burst, true); c.lineWidth = 2; c.beginPath(); c.moveTo(spark.x, spark.y); c.lineTo(spark.x + Math.cos(angle) * 8, spark.y + Math.sin(angle) * 8); c.stroke();
        }
      }
    }
    c.restore();
  }
}
