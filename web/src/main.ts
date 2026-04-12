import "./styles/index.css";
import { mount } from "svelte";
import App from "./App.svelte";

// Time-aware vein intensity: vein glows are slightly brighter at night,
// dimmer at midday. ±10% range, computed once on mount. The shift is too
// small to chase across the hour boundary — once is enough.
const hour = new Date().getHours();
const distFromNoon = Math.abs(hour - 12);
const veinIntensity = 0.9 + (distFromNoon / 12) * 0.2;
document.documentElement.style.setProperty("--vein-intensity", veinIntensity.toFixed(3));

const target = document.getElementById("app");
if (!target) throw new Error("missing #app element");
mount(App, { target });
