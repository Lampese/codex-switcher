import assert from "node:assert/strict";
import fs from "node:fs";

import { chromium } from "playwright-core";

const baseUrl = process.env.CODEX_SWITCHER_UI_URL ?? "http://127.0.0.1:3210";

function findChromeExecutable() {
  const candidates = [
    process.env.CHROME_PATH,
    "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
    "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe",
    "C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe",
    "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
  ].filter(Boolean);

  for (const candidate of candidates) {
    if (candidate && fs.existsSync(candidate)) {
      return candidate;
    }
  }

  throw new Error("No Chrome/Edge executable found for UI sync-order test");
}

async function waitForRefreshIdle(page) {
  await page.waitForFunction(() => {
    const button = Array.from(document.querySelectorAll("button")).find((element) => {
      const label = element.textContent?.trim();
      return label === "Refresh All" || label === "Refreshing...";
    });

    return button?.textContent?.trim() === "Refresh All" && !button.hasAttribute("disabled");
  }, undefined, { timeout: 60000 });
}

async function runRefreshCycle(page) {
  await waitForRefreshIdle(page);

  const refreshAllButton = page.getByRole("button", { name: "Refresh All" });
  await refreshAllButton.click();

  await page.waitForFunction(() => {
    const button = Array.from(document.querySelectorAll("button")).find((element) => {
      const label = element.textContent?.trim();
      return label === "Refresh All" || label === "Refreshing...";
    });

    return button?.textContent?.trim() === "Refreshing..." || button?.hasAttribute("disabled");
  }, undefined, { timeout: 5000 }).catch(() => {});

  await waitForRefreshIdle(page);
  await page.waitForTimeout(500);
}

async function getOtherAccountOrder(page) {
  const section = page.locator("section").filter({
    has: page.getByText(/Other Accounts \(\d+\)/),
  });

  await section.first().waitFor();

  const cardTitles = section.locator("div.theme-card h3");
  const names = await cardTitles.allTextContents();
  return names.map((name) => name.trim()).filter(Boolean);
}

const browser = await chromium.launch({
  executablePath: findChromeExecutable(),
  headless: true,
});

try {
  const page = await browser.newPage();
  await page.addInitScript(() => {
    const originalSetInterval = window.setInterval.bind(window);

    window.setInterval = (handler, timeout, ...args) => {
      if (timeout === 5000 || timeout === 60000) {
        return 0;
      }

      return originalSetInterval(handler, timeout, ...args);
    };
  });
  await page.goto(baseUrl, { waitUntil: "domcontentloaded" });
  await page.getByRole("button", { name: "Refresh All" }).waitFor();
  const samples = [];

  await runRefreshCycle(page);

  const baselineOrder = await getOtherAccountOrder(page);
  samples.push(`baseline: ${baselineOrder.join(" | ")}`);

  for (let round = 1; round <= 4; round += 1) {
    await runRefreshCycle(page);

    const order = await getOtherAccountOrder(page);
    samples.push(`round ${round}: ${order.join(" | ")}`);
    assert.deepEqual(order, baselineOrder, samples.join("\n"));
  }

  console.log(samples.join("\n"));
} finally {
  await browser.close();
}
