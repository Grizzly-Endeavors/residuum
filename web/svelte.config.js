import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";

export default {
  preprocess: vitePreprocess(),
  onwarn(warning, handler) {
    // Suppress a11y label warnings — settings-field pattern uses sibling labels
    if (warning.code === "a11y_label_has_associated_control") return;
    handler(warning);
  },
};
