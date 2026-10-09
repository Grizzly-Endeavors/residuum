// A key that must be pressed twice. The first press arms it, and a second
// within the window goes through; the window running out, or `disarm()`,
// puts it back. `armed` is what a hint to the user reads.

/** How long the first press waits for the second. */
const WINDOW_MS = 2000;

export class PressAgain {
  /** The first press has been made and the second is awaited. */
  armed = $state(false);

  private timer: ReturnType<typeof setTimeout> | undefined;

  constructor(private readonly windowMs = WINDOW_MS) {}

  /** Register a press: true when it is the second of a pair, otherwise it arms and gives false. */
  press(): boolean {
    if (this.armed) {
      this.disarm();
      return true;
    }
    this.armed = true;
    this.timer = setTimeout(() => {
      this.disarm();
    }, this.windowMs);
    return false;
  }

  disarm(): void {
    clearTimeout(this.timer);
    this.timer = undefined;
    this.armed = false;
  }
}
