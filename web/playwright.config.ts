import { defineConfig, devices } from "@playwright/test";

const liveBaseURL = process.env.AUDIODOWN_LIVE_BASE_URL;

export default defineConfig({
  testDir: "./tests",
  fullyParallel: false,
  workers: 1,
  outputDir: "test-results",
  reporter: "list",
  expect: {
    toHaveScreenshot: {
      animations: "disabled",
      caret: "hide",
      maxDiffPixelRatio: 0.01,
    },
  },
  use: {
    baseURL: liveBaseURL ?? "http://127.0.0.1:4173",
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
  webServer: liveBaseURL
    ? undefined
    : {
        command: "npm run dev -- --host 0.0.0.0 --port 4173",
        url: "http://127.0.0.1:4173",
        reuseExistingServer: false,
      },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
});
