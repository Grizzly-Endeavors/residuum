// Imports a module that does not exist, so Playwright reports an error of the
// whole run rather than of a test.

import { test } from "@playwright/test";
import "./missing-helper.mjs";

test("never runs", () => {});
