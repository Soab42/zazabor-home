/**
 * The ringtone: a short two-tone phone ring made with Web Audio (nothing to ship or load), played
 * as "ring ring, pause" until stopped. `volume` is 0 to 100.
 */
let context: AudioContext | null = null;
let timer = 0;
let level = 0.7;

function burst(ctx: AudioContext, at: number, gain: number) {
  const master = ctx.createGain();
  master.connect(ctx.destination);
  // Two rings of 0.4 s, each a 440 + 480 Hz pair with a soft edge.
  for (const start of [0, 0.5]) {
    const t = at + start;
    master.gain.setValueAtTime(0, t);
    master.gain.linearRampToValueAtTime(gain, t + 0.03);
    master.gain.setValueAtTime(gain, t + 0.37);
    master.gain.linearRampToValueAtTime(0, t + 0.4);
  }
  for (const hz of [440, 480]) {
    const osc = ctx.createOscillator();
    osc.type = "sine";
    osc.frequency.value = hz;
    osc.connect(master);
    osc.start(at);
    osc.stop(at + 1);
  }
}

export function startRing(volume: number) {
  level = Math.max(0, Math.min(100, volume)) / 100;
  if (timer) return;
  try {
    context ??= new AudioContext();
    void context.resume();
  } catch (err) {
    console.warn("Alerts: no sound:", err);
    return;
  }
  console.warn(`Alerts: ringtone starts (audio ${context.state}, volume ${Math.round(level * 100)})`);
  const play = () => context && burst(context, context.currentTime + 0.02, 0.25 * level * level);
  play();
  timer = window.setInterval(play, 2500);
}

export function stopRing() {
  if (!timer) return;
  clearInterval(timer);
  timer = 0;
  console.info("Alerts: ringtone stops");
}
