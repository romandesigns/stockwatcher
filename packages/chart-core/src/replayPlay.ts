// The replay bar's Play/Pause button: what it shows and what a press sends (the Practice-page
// freeze, 2026-11-26).
//
// The server's `replay.clock` is the truth: `playing` there is what the replay does. The button
// used to show only that, and a press sent the opposite of it. Two things broke that:
//
// - a clock frame that never came (the page's connection had fallen behind the broadcast and
//   skipped it, or was reconnecting) left the button showing the old state, so a press sent the
//   same command again ("pause" to a paused replay) and nothing happened, however often it was
//   pressed;
// - a press while the socket was down went nowhere, with no sign of it.
//
// Now a press is an intent: the button shows it at once, marked pending, until the server's clock
// agrees or `PLAY_INTENT_MS` passes, and then shows the server again. Pressing toggles what the
// button shows, so a second press always undoes the first. The feed keeps a command the socket
// could not take and sends it on the reconnect, for as long as the intent stands
// (`CONTROL_QUEUE_MS`). Pure, so every client shares it and it is tested without timers.

/** How long a press is shown before the server must have confirmed it. */
export const PLAY_INTENT_MS = 8_000;
/** How long a command made while disconnected is kept for the reconnect (the same window). */
export const CONTROL_QUEUE_MS = PLAY_INTENT_MS;

export interface PlayIntent {
  /** What the press asked for. */
  playing: boolean;
  /** When (ms, any monotonic wall clock the caller uses consistently). */
  at: number;
}

export interface PlayShown {
  /** What the button shows. */
  playing: boolean;
  /** A press the server has not confirmed yet. */
  pending: boolean;
}

/** The intent while it stands: not yet confirmed by the server and not expired. */
export function standingIntent(server: boolean | null, intent: PlayIntent | null, now: number): PlayIntent | null {
  if (!intent) return null;
  if (server === intent.playing) return null;
  if (now - intent.at >= PLAY_INTENT_MS) return null;
  return intent;
}

/** What the button shows: a standing press, else the server's clock (paused before any clock). */
export function shownPlay(server: boolean | null, intent: PlayIntent | null, now: number): PlayShown {
  const standing = standingIntent(server, intent, now);
  return standing ? { playing: standing.playing, pending: true } : { playing: server ?? false, pending: false };
}

/** A press: the new intent and the command to send. It toggles what the button shows. */
export function pressPlay(server: boolean | null, intent: PlayIntent | null, now: number): { intent: PlayIntent; action: "play" | "pause" } {
  const playing = !shownPlay(server, intent, now).playing;
  return { intent: { playing, at: now }, action: playing ? "play" : "pause" };
}

/** When the button must look again by itself (the standing intent's expiry), or null. */
export function playIntentDeadline(server: boolean | null, intent: PlayIntent | null, now: number): number | null {
  const standing = standingIntent(server, intent, now);
  return standing ? standing.at + PLAY_INTENT_MS : null;
}

/**
 * The replay commands a disconnected feed holds for the reconnect: the newest of each kind
 * (play/pause, speed, position), oldest kind first, each for `CONTROL_QUEUE_MS`. A seek and a day
 * switch are one kind (the later one wins); so are play and pause.
 */
export interface QueuedControl<C> {
  cmd: C;
  at: number;
}

export function controlKind(action: string): "playback" | "speed" | "position" | "other" {
  if (action === "play" || action === "pause") return "playback";
  if (action === "speed") return "speed";
  if (action === "seek" || action === "day" || action === "step" || action === "stepInterval") return "position";
  return "other";
}

export function queueControl<C extends { action: string }>(queue: readonly QueuedControl<C>[], cmd: C, now: number): QueuedControl<C>[] {
  const kind = controlKind(cmd.action);
  return [...queue.filter((q) => kind === "other" || controlKind(q.cmd.action) !== kind), { cmd, at: now }];
}

/** What to send on the reconnect: the queue's commands still inside their window, in order. */
export function dueControls<C>(queue: readonly QueuedControl<C>[], now: number): C[] {
  return queue.filter((q) => now - q.at < CONTROL_QUEUE_MS).map((q) => q.cmd);
}
