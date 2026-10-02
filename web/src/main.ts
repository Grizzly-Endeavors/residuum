import "./styles/fonts.css";
import "./styles/tokens.css";
import "./styles/ui-base.css";
import { mount } from "svelte";
import App from "./App.svelte";
import { startServiceWorker } from "./lib/app-update.svelte";
import { startConfigSync } from "./lib/config-sync";
import { startInstallOffer } from "./shell/install-offer";
import { startPushClient } from "./shell/push-client.svelte";

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
  startInstallOffer();
  // Builds only: the app opens with no network, and a rebuilt app shows Update ready.
  startServiceWorker();
  // This device's notifications, its presence on the hub socket, and notification clicks.
  startPushClient();
  mount(App, { target });
}
