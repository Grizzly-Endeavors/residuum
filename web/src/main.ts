import "./styles/fonts.css";
import "./styles/tokens.css";
import "./styles/index.css";
import "./styles/ui-base.css";
import { mount } from "svelte";
import App from "./App.svelte";
import { startConfigSync } from "./lib/config-sync";

// Time-aware vein intensity: vein glows are slightly brighter at night,
// dimmer at midday. ±10% range, computed once on mount. The shift is too
// small to chase across the hour boundary — once is enough.
const hour = new Date().getHours();
const distFromNoon = Math.abs(hour - 12);
const veinIntensity = 0.9 + (distFromNoon / 12) * 0.2;
document.documentElement.style.setProperty("--vein-intensity", veinIntensity.toFixed(3));

/** Every primitive control in every state, for development. */
const UI_GALLERY_PATH = "/dev/gallery";

const target = document.getElementById("app");
if (!target) throw new Error("missing #app element");

// Production builds replace the flag with false, which drops this branch and
// the gallery's chunk with it.
if (__UI_GALLERY__ && window.location.pathname === UI_GALLERY_PATH) {
  void import("./lib/ui/gallery/Gallery.svelte").then(({ default: Gallery }) =>
    mount(Gallery, { target }),
  );
} else {
  // Config changes made outside this page reach the views that show them.
  startConfigSync();
  mount(App, { target });
}
