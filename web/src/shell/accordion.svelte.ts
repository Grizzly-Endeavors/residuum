export class RailAccordion {
  open = $state<string | null>(null);
  private viewed: string | null = null;

  toggle(name: string): void {
    this.open = this.open === name ? null : name;
  }

  expand(name: string): void {
    this.open = name;
  }

  follow(viewed: string | null): void {
    if (viewed !== null && viewed !== this.viewed) this.open = viewed;
    this.viewed = viewed;
  }
}
