import "./styles/fonts.css";
import "./styles/tokens.css";
import "./styles/ui-base.css";
import { mount } from "svelte";
import App from "./App.svelte";
import { startServiceWorker } from "./lib/app-update.svelte";
import { captureCloudHandoff } from "./lib/cloud-handoff";
import { startConfigSync } from "./lib/config-sync";
import { PAIRING_PATH } from "./lib/pairing";
import { startInstallOffer } from "./shell/install-offer";
import { startPushClient } from "./shell/push-client.svelte";

const UI_GALLERY_PATH = "/dev/gallery";

const target = document.getElementById("app");
if (!target) throw new Error("missing #app element");

if (__UI_GALLERY__ && window.location.pathname === UI_GALLERY_PATH) {
  void import("./lib/ui/gallery/Gallery.svelte").then(({ default: Gallery }) =>
    mount(Gallery, { target }),
  );
} else if (window.location.pathname === PAIRING_PATH) {
  void import("./pairing/PairingPage.svelte").then(({ default: PairingPage }) =>
    mount(PairingPage, { target }),
  );
} else {
  captureCloudHandoff(window.location.search);
  startConfigSync();
  startInstallOffer();
  startServiceWorker();
  startPushClient();
  mount(App, { target });
}
