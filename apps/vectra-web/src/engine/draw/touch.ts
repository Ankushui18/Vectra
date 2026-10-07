/**
 * **The invisible UI** (Task 10.7 RULE 1).
 *
 * Procreate's workflow is not a toolbar, it is a vocabulary of gestures: two
 * fingers to take something back, three to put it back, three dragged down to
 * copy. Vectra already has the commands those gestures mean — `Undo`, `Redo`,
 * `DuplicateNode` — so this file is the *recogniser*, and it is written exactly
 * like the pointer router next to it: pure, DOM-free, headless, and driven from a
 * loop in the tests. No canvas, no React, no `setTimeout`.
 *
 * | gesture | means | why it cannot be confused with drawing |
 * | --- | --- | --- |
 * | 2-finger tap | **Undo** | two contacts is not one stroke; a stroke has one pointer id |
 * | 3-finger tap | **Redo** | as above, with the third finger |
 * | 3-finger swipe down | **Copy / Paste** | three fingers that travel: not a tap (travel is distance), not a stroke (three contacts) |
 * | Ctrl + click | **Undo** | the mouse has no second finger; the modifier is the same sentence |
 * | Ctrl + Shift + click | **Redo** | |
 *
 * ## The CRITICAL half: the browser must not answer first
 *
 * A two-finger tap on a touchscreen is the browser's pinch-zoom. A three-finger
 * drag is its scroll. A long press is its context menu. Every one of them would
 * fire *alongside* (or instead of) the engine's command, so the recogniser does
 * not only name the action — it also **owns the event**, and says so:
 * {@link TouchDecision.swallow}. From the moment a second contact lands, the
 * answer is `true`, and the caller's job is one line: `if (d.swallow)
 * event.preventDefault()`. The canvas additionally declares `touch-action: none`
 * and swallows `contextmenu`, so the page cannot scroll or open a menu before
 * preventDefault even runs — the three layers are listed in the report.
 *
 * The timing bound is a *tap* bound, not a timer: nothing here schedules work. A
 * contact that stays down past {@link TAP_MAX_MS} simply never becomes a tap,
 * which is why this file needs no clock the tests would have to fake.
 */

export interface Point {
  x: number;
  y: number;
}

/** A contact has to be this short to count as a tap, in milliseconds. */
export const TAP_MAX_MS = 320;

/** …and this still. Fingers roll a little; a hand that *moves* is a drag. */
export const TAP_SLOP_PX = 12;

/** How far down three fingers must travel before the drag is a copy/paste. */
export const SWIPE_MIN_PX = 40;

/** …and how far sideways they may drift while doing it. Past this it reads as a
 *  horizontal gesture (a layer flip in Procreate's own vocabulary), and a
 *  gesture the engine has no command for must not fire one by accident. */
export const SWIPE_CROSS_PX = 40;

/** What a recognised gesture means, in the engine's vocabulary. */
export type TouchAction = 'undo' | 'redo' | 'copyPaste' | 'none';

/** The routing decision for one pointer event. */
export interface TouchDecision {
  /** The command the gesture became. */
  action: TouchAction;
  /**
   * **The canvas owns this event.** True from the second contact on, and true
   * whenever an action fired. The caller must `preventDefault()` — this is the
   * difference between an undo and a page that scrolled while the designer was
   * trying to undo.
   */
  swallow: boolean;
  /** How many contacts the router is holding right now. */
  contacts: number;
}

/** A pointer event, in the two numbers the recogniser needs. */
export interface ContactInput {
  pointerId: number;
  clientX: number;
  clientY: number;
}

/** A modifier click, as the canvas sees it. */
export interface ClickInput {
  ctrlKey: boolean;
  shiftKey: boolean;
  /** The primary button only; a right-click is the context menu's. */
  button?: number;
  /** True when the pointer travelled past the slop before lifting. */
  dragged?: boolean;
}

interface Contact {
  id: number;
  start: Point;
  last: Point;
  startAt: number;
}

/**
 * **What a modifier click means** (RULE 1's mouse half).
 *
 * Literally the brief: `Ctrl` → Undo, `Ctrl+Shift` → Redo. It is a *separate*
 * entry point rather than a branch inside the router because a mouse never has
 * two contacts: the tap machine would have to special-case "one finger that
 * happens to hold Ctrl", which is exactly the kind of exception that grows teeth
 * later.
 *
 * `Cmd` is deliberately **not** accepted: on macOS `⌘`-click is the ordinary
 * secondary/selection modifier, and hijacking it would take Undo away from the
 * two-finger tap that *is* the native gesture there.
 *
 * A dragged click is refused: Ctrl-dragging is a legitimate gesture elsewhere in
 * the app (and in the engine, `alt`-drag), and a pointer that travelled is not a
 * click by any definition the rest of this codebase uses
 * (`DRAG_SLOP_PX` in `pointer.ts`).
 */
export function modifierClick(input: ClickInput): TouchAction {
  if (input.button !== undefined && input.button !== 0) return 'none';
  if (input.dragged) return 'none';
  if (!input.ctrlKey) return 'none';
  return input.shiftKey ? 'redo' : 'undo';
}

/**
 * The multi-touch recogniser.
 *
 * It is a *class* because a gesture has state across events (which contacts are
 * down, when the episode started, how many there ever were), and because the
 * state is the thing the laws test: feed a synthetic gesture in, read the
 * decisions out. Every method takes `time` as a number — the tests drive a
 * decade of gestures in a millisecond, and nothing here reads a clock.
 */
export class TouchRouter {
  private contacts = new Map<number, Contact>();
  /** The most contacts this episode ever held — a tap is decided on the *high
   *  water mark*, so a three-finger tap whose middle finger lands a few
   *  milliseconds late is still a Redo. */
  private maxContacts = 0;
  /** The episode's first contact, for the tap duration bound. */
  private startedAt = 0;
  /** Did any contact move past the slop during this episode? One moving finger
   *  disqualifies the whole tap: it is a drag with company. */
  private moved = false;
  /** Has this episode already produced an action? A gesture fires once. */
  private fired = false;

  /** How many contacts are down. */
  get live(): number {
    return this.contacts.size;
  }

  /** Did the current episode produce an action already? (The UI uses it to keep
   *  the preview quiet while a swipe is in flight.) */
  get firedThisEpisode(): boolean {
    return this.fired;
  }

  /** A contact landed. */
  down(input: ContactInput, time: number): TouchDecision {
    if (this.contacts.size === 0) {
      this.maxContacts = 0;
      this.moved = false;
      this.fired = false;
      this.startedAt = time;
    }
    this.contacts.set(input.pointerId, {
      id: input.pointerId,
      start: { x: input.clientX, y: input.clientY },
      last: { x: input.clientX, y: input.clientY },
      startAt: time,
    });
    this.maxContacts = Math.max(this.maxContacts, this.contacts.size);
    return this.decision('none');
  }

  /**
   * A contact moved.
   *
   * The timestamp is accepted and unused: every contact event in this file
   * carries one, so the vocabulary is uniform and a velocity threshold can
   * be added without changing a single call site. What it must *not* do is
   * silently help a tap linger — the duration bound is measured from the
   * episode's first contact, in `up`.
   */
  move(input: ContactInput, _time: number): TouchDecision {
    const contact = this.contacts.get(input.pointerId);
    if (!contact) return this.decision('none');
    contact.last = { x: input.clientX, y: input.clientY };
    if (
      Math.hypot(contact.last.x - contact.start.x, contact.last.y - contact.start.y) >= TAP_SLOP_PX
    ) {
      this.moved = true;
    }
    // The swipe: three fingers, travelling down, together. Fires the moment the
    // threshold is crossed — a swipe is not a tap and must not wait for release,
    // or the copy would arrive after the hand left the glass.
    if (!this.fired && this.contacts.size >= 3 && this.maxContacts >= 3) {
      const live = [...this.contacts.values()];
      const down = Math.max(...live.map((c) => c.last.y - c.start.y));
      const across = Math.max(...live.map((c) => Math.abs(c.last.x - c.start.x)));
      if (down >= SWIPE_MIN_PX && across <= SWIPE_CROSS_PX) {
        this.fired = true;
        return this.decision('copyPaste');
      }
    }
    return this.decision('none');
  }

  /**
   * A contact lifted — where a **tap** is decided.
   *
   * Everything the tap needs is already in the episode: how many contacts ever
   * landed ({@link maxContacts}), whether any of them travelled
   * ({@link moved}), and how long the hand was on the glass. The decision is
   * therefore made at the moment the *last* contact leaves, with no timer and no
   * grace period: a gesture that is still undecided is simply not a tap.
   */
  up(input: ContactInput, time: number): TouchDecision {
    const contact = this.contacts.get(input.pointerId);
    if (!contact) return this.decision('none');
    this.contacts.delete(input.pointerId);
    if (this.contacts.size > 0 || this.fired) return this.decision('none');
    const held = time - this.startedAt;
    const tap = this.maxContacts >= 2 && !this.moved && held <= TAP_MAX_MS;
    if (!tap) return this.decision('none');
    this.fired = true;
    return this.decision(this.maxContacts >= 3 ? 'redo' : 'undo');
  }

  /** A contact was lost (a browser takeover, a window blur): drop it, fire
   *  nothing. A cancelled gesture must never undo — that is the one outcome a
   *  designer cannot predict. */
  cancel(input: ContactInput): void {
    this.contacts.delete(input.pointerId);
    // The *episode* is dead, not just this contact: a gesture the browser took
    // away half-way through must not come back as a tap when the finger that is
    // still down lifts. `fired` is cleared by the next `down` — a fresh gesture
    // is a fresh episode.
    this.fired = true;
  }

  /** Forget the episode entirely (a tool change, a document swap). */
  reset(): void {
    this.contacts.clear();
    this.maxContacts = 0;
    this.moved = false;
    this.fired = false;
    this.startedAt = 0;
  }

  private decision(action: TouchAction): TouchDecision {
    return {
      action,
      swallow: action !== 'none' || this.contacts.size >= 2,
      contacts: this.contacts.size,
    };
  }
}
