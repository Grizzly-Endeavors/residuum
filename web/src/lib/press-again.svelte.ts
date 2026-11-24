const WINDOW_MS = 2000;

export class PressAgain {
  armed = $state(false);

  private timer: ReturnType<typeof setTimeout> | undefined;

  constructor(private readonly windowMs = WINDOW_MS) {}

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
