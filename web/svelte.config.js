import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";

export default {
  preprocess: vitePreprocess(),
  compilerOptions: {
    // Suppress a11y label warnings — settings-field pattern uses sibling labels.
    // A compiler filter rather than `onwarn`, so svelte-check honors it too.
    warningFilter: (warning) => warning.code !== "a11y_label_has_associated_control",
  },
};
