/** True in the dev server and mock builds, which serve the primitives gallery. Set in vite.config.ts. */
declare const __UI_GALLERY__: boolean;

/** True in a build, which registers the service worker; false in the dev server and the tests. Set in vite.config.ts. */
declare const __SERVICE_WORKER__: boolean;
